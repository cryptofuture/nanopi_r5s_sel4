#define _POSIX_C_SOURCE 200809L
#include <assert.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>
#include "dma.h"
/* contract §1 layout: exact field order/widths (Rust mirror compatibility) */
_Static_assert(sizeof(struct dma_tso_frag)==16,"frag layout");
_Static_assert(sizeof(struct dma_tso_request)==152,"request layout");
_Static_assert(offsetof(struct dma_tso_request,tso_id)==16 && offsetof(struct dma_tso_request,frags)==24,
 "request field order");
/* Host test for the GMAC0 hardware TSO path (docs/tso-design.md §1-§3,
 * docs/tso-capability.md). The device model understands context
 * descriptors (CTXT bit 30, TCMSSV bit 26), TSO first descriptors (TSE
 * bit 18, hdr_len bits 22:19, payload bits 17:0), LD framing and
 * multi-descriptor frames, and verifies every fetched word against an
 * expectation queue pushed by the test before each send. */
static _Alignas(4096) unsigned char cpu[DMA_SIZE],device[DMA_SIZE];
static uint32_t regs[2048];
static int refuse_tse;
static uint64_t now;static unsigned reset_stuck,fail_cache,cache_calls;
static unsigned cache_tx_buffer_calls,cache_ext_calls,hw_writes;
static unsigned hold,tx_fault_bits,last_tail,tx_hw;
/* ---- external Normal non-cacheable pool (identity VA == PA) ---- */
#define EXT0 0x24400000u
#define EXT_SIZE (8u*65536u)
static unsigned char *ext;
#define BASE 0x24000000u
static unsigned pslot; /* mirror of the driver TX producer */
uintptr_t dma_virtual(void) {return (uintptr_t)cpu;}
uint64_t dma_physical(void) {return BASE;}
uintptr_t dma_ref_virtual(uint64_t physical) {
 if (physical>=BASE && physical<BASE+DMA_SIZE) return (uintptr_t)cpu+(physical-BASE);
 if (physical>=EXT0 && physical<EXT0+EXT_SIZE) return (uintptr_t)physical;
 return 0;
}
uint64_t hw_us(void) {return ++now;}
void hw_progress(void) {}
uint32_t hw_read(unsigned region,unsigned offset) {assert(region==0);return regs[offset/4];}
int dma_cache(unsigned operation,uintptr_t address,size_t size) {
 cache_calls++;
 if (address>=(uintptr_t)EXT0 && address<(uintptr_t)EXT0+EXT_SIZE) {
  cache_ext_calls++;return 0; /* counted, rejected by the closing assertion */
 }
 assert(address>=(uintptr_t)cpu && address+size<=(uintptr_t)cpu+DMA_SIZE && address%64==0 && size%64==0);
 if (address>=(uintptr_t)cpu+DMA_TX_BUFFER && address<(uintptr_t)cpu+DMA_TX_BUFFER+DMA_SLOTS*DMA_BUFFER)
  cache_tx_buffer_calls++;
 if (fail_cache) return 7;
 size_t offset=address-(uintptr_t)cpu;
 if (operation==0 || operation==2) memcpy(device+offset,cpu+offset,size);
 if (operation==1 || operation==2) memcpy(cpu+offset,device+offset,size);
 return 0;
}
/* ---- expectation queue + device TX model ---- */
struct exp {uint32_t w[4];};
static struct exp expq[2048];static unsigned exp_push,exp_pop;
static struct fetched {unsigned slot;uint32_t w[4];} fl[2048];
static unsigned f_push,f_pop;
static unsigned in_frame,tso_left;
static void expect(uint32_t w0,uint32_t w1,uint32_t w2,uint32_t w3) {
 assert(exp_push<sizeof expq/sizeof expq[0]);
 expq[exp_push].w[0]=w0;expq[exp_push].w[1]=w1;expq[exp_push].w[2]=w2;expq[exp_push].w[3]=w3;
 exp_push++;
}
/* Write back the oldest `n` fetched (device-owned) descriptors. */
static void device_complete(unsigned n) {
 while (n--) {
  assert(f_pop<f_push);
  struct fetched *f=&fl[f_pop++];
  struct dma_desc *d=(void *)(device+f->slot*DMA_STRIDE);
  d->word[0]=0;d->word[3]=tx_fault_bits|(f->w[3]&0x40000000u);tx_fault_bits=0;
 }
}
static void frame_parse(const uint32_t *w) {
 uint32_t w3=w[3];
 assert(w[1]==0);
 if (w3&0x40000000u) { /* context: CTXT|TCMSSV, MSS only */
  assert(!in_frame && w[0]==0 && w3==0xc4000000u && w[2]>=64 && w[2]<=2047);
  return;
 }
 if (w3&0x20000000u) { /* FD: TSO first or ordinary single-descriptor frame */
  assert(!in_frame && (w3&0x80000000u));
  if (w3&0x00040000u) {
   assert(((w3>>19)&0xf)>=5 && ((w3>>19)&0xf)<=15); /* tcp_hdr_len/4 */
   assert(w[2]>=54 && w[2]<=1023);
   assert((w3&0x3ffff)>=1 && !(w3&0x10000000u)); /* payload, not LD */
   assert((w3&~(0x80000000u|0x20000000u|0x00040000u|0x00780000u|0x3ffff))==0);
   tso_left=w3&0x3ffff;in_frame=1;
  } else {
   assert(w[2]>=60 && w[2]<=1514 && (w3&0x7fff)==w[2] && (w3&0x10000000u));
  }
  return;
 }
 assert(in_frame && (w3&0x80000000u)); /* TSO payload chunk */
 assert(w[2]>=1 && w[2]<=DMA_TSO_MAX_CHUNK);
 assert((w3&~(0x80000000u|0x10000000u))==0);
 assert(w[2]<=tso_left);
 tso_left-=w[2];
 if (w3&0x10000000u) {assert(tso_left==0);in_frame=0;}
}
void hw_write(unsigned region,unsigned offset,uint32_t value) {
 assert(dma_allowed(region,offset,value));regs[offset/4]=value;hw_writes++;
 if (offset==0x1104 && refuse_tse) regs[offset/4]&=~(1u<<12);
 if (offset==0x1000 && !reset_stuck) {
  regs[offset/4]=0;regs[0]=0;tx_hw=0;f_pop=f_push;in_frame=0;pslot=0;
  assert(exp_pop==exp_push); /* nothing fetched may survive a reset */
 }
 if (offset==0x1120) {
  last_tail=value;
  assert(value>=BASE && value<BASE+DMA_RX_DESC && (value-BASE)%DMA_STRIDE==0);
  unsigned tail=(value-BASE)/DMA_STRIDE;
  while (tx_hw!=tail) {
   struct dma_desc *d=(void *)(device+tx_hw*DMA_STRIDE);
   assert(d->word[3]&0x80000000u);
   assert(exp_pop<exp_push);
   uint32_t w[4]={d->word[0],d->word[1],d->word[2],d->word[3]};
   const struct exp *e=&expq[exp_pop++];
   assert(w[0]==e->w[0] && w[1]==e->w[1] && w[2]==e->w[2] && w[3]==e->w[3]);
   frame_parse(w);
   assert(f_push<sizeof fl/sizeof fl[0]);
   fl[f_push].slot=tx_hw;memcpy(fl[f_push].w,w,sizeof fl[0].w);f_push++;
   if (!hold) device_complete(1);
   tx_hw=(tx_hw+1)%DMA_SLOTS;
  }
 }
}
static uint64_t got_ext;static uint32_t got_id;
/* complete the held descriptors and reclaim exactly `n` slots */
static void drain(unsigned n) {
 device_complete(n);
 for (unsigned i=0;i<n;i++) assert(dma_reclaim_full(&got_ext,&got_id)==1);
 assert(dma_reclaim_full(&got_ext,&got_id)==0);
}
static struct dma_tso_request valid_request(void) {
 struct dma_tso_request r;
 memset(&r,0,sizeof r);
 r.ip_version=4;r.protocol=6;r.ip_hdr_len=20;r.tcp_hdr_len=20;
 r.mss=500;r.head_len=54;r.payload_len=1000;r.tso_id=0x3333;
 r.nfrags=1;r.frags[0].physical=EXT0;r.frags[0].length=1054;
 return r;
}
static void send_ordinary(void) {
 unsigned char packet[1514];
 memset(packet,0x5a,sizeof packet);
 expect(BASE+DMA_TX_BUFFER+pslot*DMA_BUFFER,0,60,0xb000003cu);
 assert(dma_send(packet,60)==60);
 pslot=(pslot+1)%DMA_SLOTS;
}
static void send_ref(uint64_t physical,unsigned length) {
 expect((uint32_t)physical,0,length,0xb0000000u|length);
 assert(dma_send_ref(physical,length,0)==(int)length);
 pslot=(pslot+1)%DMA_SLOTS;
}
/* push the exact expected descriptor words for `r`, return the slot count
 * (bulk helper; the encoding tests assert literal words instead) */
static unsigned expect_tso(const struct dma_tso_request *r) {
 unsigned slots=2,fi=0;
 expect(0,0,r->mss,0xc4000000u);
 expect((uint32_t)r->frags[0].physical,0,r->head_len,
   0x80000000u|0x20000000u|0x00040000u|((uint32_t)r->tcp_hdr_len/4<<19)|(r->payload_len&0x3ffff));
 uint32_t address=(uint32_t)r->frags[0].physical+r->head_len,left=r->frags[0].length-r->head_len;
 for (;;) {
  if (!left) {
   if (++fi>=r->nfrags) break;
   address=(uint32_t)r->frags[fi].physical;left=r->frags[fi].length;continue;
  }
  uint32_t chunk=left>DMA_TSO_MAX_CHUNK?DMA_TSO_MAX_CHUNK:left;
  left-=chunk;
  unsigned last=left==0 && fi+1==r->nfrags;
  expect(address,0,chunk,0x80000000u|(last?0x10000000u:0));
  address+=chunk;slots++;
 }
 return slots;
}
static void send_tso_ok(const struct dma_tso_request *r,unsigned slots) {
 unsigned before=hw_writes;
 unsigned s=expect_tso(r);
 assert(s==slots);
 assert(dma_send_tso(r)==(int)s);
 assert(hw_writes==before+1); /* exactly one tail write, no new registers */
 pslot=(pslot+s)%DMA_SLOTS;
}
int main(void) {
 struct dma_tso_request r;
 struct dma_tso_capability cap;
 hold=1;
 regs[0x110/4]=0x51;
 regs[0x120/4]=5|(5<<6)|(1u<<18);
 ext=mmap((void *)(uintptr_t)EXT0,EXT_SIZE,PROT_READ|PROT_WRITE,
          MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED,-1,0);
 assert(ext==(unsigned char *)(uintptr_t)EXT0);
 memset(ext,0x5a,EXT_SIZE);
 assert(dma_start()==0);
 /* ---- capability report ---- */
 assert(dma_tso_available()==1);
 assert(regs[0x1104/4]&(1u<<12));
 memset(&cap,0xff,sizeof cap);
 dma_tso_capability_read(&cap);
 assert(cap.supported==1 && cap.ip_version_mask==3);
 assert(cap.max_payload==DMA_TSO_MAX_PAYLOAD && cap.max_mss==DMA_TSO_MAX_MSS);
 assert(cap.min_mss==DMA_TSO_MIN_MSS && cap.max_header==DMA_TSO_MAX_HDR);
 assert(cap.max_frags==DMA_TSO_MAX_FRAGS);
 /* ---- test 1: descriptor encoding, literal contract bit patterns ---- */
 r=valid_request();
 r.ip_version=4;r.tcp_hdr_len=20;r.mss=1460;r.head_len=54;
 r.payload_len=40000;r.tso_id=0x12345678;r.nfrags=2;
 r.frags[0].physical=EXT0;r.frags[0].length=1054; /* 54 hdr + 1000 inline */
 r.frags[1].physical=EXT0+0x10000;r.frags[1].length=39000;
 assert(r.frags[0].length+r.frags[1].length==r.head_len+r.payload_len);
 expect(0,0,1460,0xc4000000u);
 expect(0x24400000u,0,54,0xa02c9c40u); /* OWN|FD|TSE|(5<<19)|40000 */
 expect(0x24400036u,0,1000,0x80000000u);
 expect(0x24410000u,0,16383,0x80000000u);
 expect(0x24413fffu,0,16383,0x80000000u);
 expect(0x24417ffeu,0,6234,0x90000000u); /* OWN|LD */
 {
  unsigned before=hw_writes;
  assert(dma_send_tso(&r)==6);
  assert(hw_writes==before+1 && last_tail==BASE+6*DMA_STRIDE);
  pslot=(pslot+6)%DMA_SLOTS;
 }
 assert(exp_pop==exp_push);
 device_complete(6);
 /* per-slot completion report: tso_id only on the LD slot */
 assert(dma_reclaim_full(&got_ext,&got_id)==1 && got_ext==0 && got_id==0);
 for (unsigned i=0;i<4;i++) assert(dma_reclaim_full(&got_ext,&got_id)==1 && got_ext==0 && got_id==0);
 assert(dma_reclaim_full(&got_ext,&got_id)==1 && got_ext==0 && got_id==0x12345678);
 assert(dma_reclaim_full(&got_ext,&got_id)==0);
 /* ---- test 2: chunk splitting (inline payload + large later frag) ---- */
 r=valid_request();
 r.mss=1000;r.payload_len=21000;r.tso_id=0x2222;r.nfrags=2;
 r.frags[0].physical=EXT0;r.frags[0].length=1054;
 r.frags[1].physical=EXT0+0x20000;r.frags[1].length=20000;
 expect(0,0,1000,0xc4000000u);
 expect(0x24400000u,0,54,0xa02c0000u|(5u<<19)|21000u);
 expect(0x24400036u,0,1000,0x80000000u);
 expect(0x24420000u,0,16383,0x80000000u);
 expect(0x24423fffu,0,3617,0x90000000u);
 assert(dma_send_tso(&r)==5);
 pslot=(pslot+5)%DMA_SLOTS;
 device_complete(5);
 for (unsigned i=0;i<5;i++) {
  assert(dma_reclaim_full(&got_ext,&got_id)==1);
  assert(got_id==(i==4?0x2222u:0));
 }
 assert(dma_reclaim_full(&got_ext,&got_id)==0);
 /* inline payload alone crossing the 16383 chunk boundary */
 r=valid_request();
 r.mss=1000;r.payload_len=20000;r.tso_id=0x2223;r.nfrags=1;
 r.frags[0].physical=EXT0;r.frags[0].length=20054;
 send_tso_ok(&r,4);
 drain(4);
 /* ---- test 3: validation (§2) ---- */
 r=valid_request();
 #define BAD(field,value) do {struct dma_tso_request b=r;b.field=(value);\
  assert(dma_send_tso(&b)==-27);} while (0)
 BAD(ip_version,5);BAD(ip_version,3);BAD(protocol,17);BAD(protocol,0);
 BAD(ip_hdr_len,24);BAD(ip_hdr_len,19);
 BAD(tcp_hdr_len,19);BAD(tcp_hdr_len,61);BAD(tcp_hdr_len,22);BAD(tcp_hdr_len,0);
 BAD(head_len,55);BAD(head_len,53);BAD(head_len,2000);BAD(head_len,1024);
 BAD(mss,63);BAD(mss,2048);BAD(mss,0);
 BAD(payload_len,0);
 BAD(nfrags,0);BAD(nfrags,9);
 BAD(tso_id,0);
 r=valid_request();r.payload_len=63947; /* head+payload 64001 */
 assert(dma_send_tso(&r)==-27);
 r=valid_request();r.frags[0].physical=EXT0+32;
 assert(dma_send_tso(&r)==-27);
 r=valid_request();r.frags[0].physical=EXT0+63;
 assert(dma_send_tso(&r)==-27);
 r=valid_request();r.frags[0].physical=0x100000000ull;
 assert(dma_send_tso(&r)==-27);
 r=valid_request();r.frags[0].length=1055; /* sum mismatch */
 assert(dma_send_tso(&r)==-27);
 r=valid_request();r.frags[0].length=53; /* < head_len */
 assert(dma_send_tso(&r)==-27);
 r=valid_request();r.nfrags=2;r.frags[0].length=1054;
 r.frags[1].physical=EXT0+0x10000;r.frags[1].length=0;
 assert(dma_send_tso(&r)==-27);
 r=valid_request();r.nfrags=2;r.frags[0].length=1054;
 r.frags[1].physical=EXT0+0x10001;r.frags[1].length=1000; /* misaligned frag 1 */
 r.payload_len=2000;
 assert(dma_send_tso(&r)==-27);
 r=valid_request();r.nfrags=2;r.frags[0].length=1054;
 r.frags[1].physical=0x1ffffffffull;r.frags[1].length=1000;
 r.payload_len=2000;
 assert(dma_send_tso(&r)==-27);
 assert(exp_pop==exp_push && dma_reclaim_full(&got_ext,&got_id)==0);
 /* boundary-valid requests (accepted, exact descriptor words verified) */
 r=valid_request();r.mss=64;r.tso_id=1;send_tso_ok(&r,3);drain(3);
 r=valid_request();r.mss=2047;r.tso_id=2;send_tso_ok(&r,3);drain(3); /* payload<mss fine */
 r=valid_request();r.tcp_hdr_len=60;r.head_len=94;r.payload_len=100;r.tso_id=3;
 r.frags[0].length=94+100;send_tso_ok(&r,3);drain(3);
 r=valid_request();r.tcp_hdr_len=40;r.head_len=74;r.payload_len=100;r.tso_id=4;
 r.frags[0].length=74+100;send_tso_ok(&r,3);drain(3);
 r=valid_request();r.payload_len=63946;r.tso_id=5; /* exactly 64000 */
 r.frags[0].length=54+63946;send_tso_ok(&r,6);drain(6);
 r=valid_request();r.ip_version=6;r.ip_hdr_len=40;r.head_len=74;
 r.payload_len=500;r.tso_id=6;r.frags[0].length=574;
 send_tso_ok(&r,3);drain(3); /* IPv6 explicitly supported */
 r=valid_request();r.tso_id=7;r.nfrags=8;r.payload_len=800;
 r.frags[0].physical=EXT0;r.frags[0].length=154; /* 54+100 inline */
 for (unsigned i=1;i<8;i++) {r.frags[i].physical=EXT0+0x10000*i;r.frags[i].length=100;}
 send_tso_ok(&r,10);drain(10); /* 8 payload chunks + ctx + first */
 /* ---- error paths while running: NULL request, cache failure ---- */
 assert(dma_send_tso(NULL)==-27);
 r=valid_request();r.tso_id=8;
 fail_cache=1;{unsigned before=hw_writes;
  assert(dma_send_tso(&r)==-20 && hw_writes==before);}
 fail_cache=0;
 assert(exp_pop==exp_push);
 /* ---- test 7: cache ownership (private maintained, external never) ---- */
 {
  uint32_t priv0=BASE+DMA_TX_BUFFER+32*DMA_BUFFER,priv1=BASE+DMA_TX_BUFFER+40*DMA_BUFFER;
  unsigned ct0=cache_tx_buffer_calls,cc0=cache_calls;
  r=valid_request();r.mss=1000;r.payload_len=21000;r.tso_id=9;r.nfrags=2;
  r.frags[0].physical=EXT0;r.frags[0].length=1054;
  r.frags[1].physical=EXT0+0x20000;r.frags[1].length=20000;
  send_tso_ok(&r,5);
  assert(cache_tx_buffer_calls==ct0); /* all-external: no buffer maintenance */
  assert(cache_calls==cc0+10); /* observe+clean per descriptor only */
  assert(ext[0]==0x5a && ext[0x20000]==0x5a); /* never memset */
  drain(5);
  ct0=cache_tx_buffer_calls;cc0=cache_calls;
  r=valid_request();r.mss=1000;r.payload_len=21000;r.tso_id=10;r.nfrags=2;
  r.frags[0].physical=priv0;r.frags[0].length=1054;
  r.frags[1].physical=priv1;r.frags[1].length=20000;
  send_tso_ok(&r,5);
  /* first desc + inline chunk + 2 payload chunks: 4 private ranges cleaned */
  assert(cache_tx_buffer_calls==ct0+4);
  assert(cache_calls==cc0+10+4);
  drain(5);
  assert(cache_ext_calls==0);
 }
 /* ---- test 4: ring saturation (DMA_SLOTS-1 cap, atomic request) ---- */
 for (unsigned i=0;i<61;i++) send_ordinary();
 r=valid_request();r.mss=1000;r.payload_len=16383;r.tso_id=11;
 r.frags[0].length=54+16383; /* 3 slots exactly */
 {unsigned before=hw_writes,f0=exp_push;
  assert(dma_send_tso(&r)==0); /* 61+3 > 63 */
  assert(hw_writes==before && exp_push==f0);} /* ring untouched */
 device_complete(1);assert(dma_reclaim_full(&got_ext,&got_id)==1 && got_id==0);
 send_tso_ok(&r,3); /* 60+3 == 63: fits exactly */
 {unsigned before=hw_writes,f0=exp_push;
  assert(dma_send_tso(&r)==0 && hw_writes==before && exp_push==f0);}
 device_complete(63);
 for (unsigned i=0;i<60;i++) assert(dma_reclaim_full(&got_ext,&got_id)==1 && got_id==0);
 for (unsigned i=0;i<3;i++) {
  assert(dma_reclaim_full(&got_ext,&got_id)==1);
  assert(got_id==(i==2?11u:0));
 }
 assert(dma_reclaim_full(&got_ext,&got_id)==0);
 /* ---- test 5: wraparound mid-request (normalize producer to slot 62) ---- */
 while (pslot!=62) {send_ordinary();device_complete(1);
  assert(dma_reclaim_full(&got_ext,&got_id)==1);}
 r=valid_request();r.mss=1000;r.payload_len=4000;r.tso_id=12;r.nfrags=2;
 r.frags[0].length=1054;r.frags[1].physical=EXT0+0x20000;r.frags[1].length=3000;
 {unsigned f0=f_push;
  send_tso_ok(&r,4);
  assert(last_tail==BASE+2*DMA_STRIDE); /* producer wrapped to slot 2 */
  assert(fl[f0].slot==62 && fl[f0+1].slot==63 && fl[f0+2].slot==0 && fl[f0+3].slot==1);
  assert(fl[f0+2].w[0]==EXT0+54 && fl[f0+3].w[0]==EXT0+0x20000);}
 device_complete(4);
 for (unsigned i=0;i<4;i++) {
  assert(dma_reclaim_full(&got_ext,&got_id)==1);
  assert(got_id==(i==3?12u:0));
 }
 assert(dma_reclaim_full(&got_ext,&got_id)==0);
 /* ---- test 6: partial completion ---- */
 r=valid_request();r.mss=1000;r.payload_len=4000;r.tso_id=13;r.nfrags=2;
 r.frags[0].length=1054;r.frags[1].physical=EXT0+0x20000;r.frags[1].length=3000;
 send_tso_ok(&r,4);
 device_complete(2);
 assert(dma_reclaim_full(&got_ext,&got_id)==1 && got_id==0);
 assert(dma_reclaim_full(&got_ext,&got_id)==1 && got_id==0);
 assert(dma_reclaim_full(&got_ext,&got_id)==0); /* remainder still owned */
 device_complete(1);
 assert(dma_reclaim_full(&got_ext,&got_id)==1 && got_id==0);
 assert(dma_reclaim_full(&got_ext,&got_id)==0);
 device_complete(1);
 assert(dma_reclaim_full(&got_ext,&got_id)==1 && got_id==13);
 assert(dma_reclaim_full(&got_ext,&got_id)==0);
 /* ---- test 11: ordinary + external + TSO interleaved on one ring ---- */
 send_ordinary();
 send_ref(EXT0+0x30000,100);
 r=valid_request();r.tso_id=14;r.payload_len=20000;r.frags[0].length=20054;
 send_tso_ok(&r,4);
 send_ordinary();
 send_ref(EXT0+0x31000,64);
 device_complete(8);
 got_ext=123;got_id=456;
 assert(dma_reclaim_ex(&got_ext)==1 && got_ext==0); /* private ordinary */
 assert(dma_reclaim_ex(&got_ext)==1 && got_ext==EXT0+0x30000); /* external */
 /* TSO ctx slot: reclaim_ex behavior unchanged (external=0, no tso report) */
 assert(dma_reclaim_ex(&got_ext)==1 && got_ext==0);
 assert(dma_reclaim_ex(&got_ext)==1 && got_ext==0); /* TSO first */
 assert(dma_reclaim_full(&got_ext,&got_id)==1 && got_ext==0 && got_id==0);
 assert(dma_reclaim_full(&got_ext,&got_id)==1 && got_ext==0 && got_id==14);
 assert(dma_reclaim_ex(&got_ext)==1 && got_ext==0);
 assert(dma_reclaim_ex(&got_ext)==1 && got_ext==EXT0+0x31000);
 assert(dma_reclaim_ex(&got_ext)==0);
 /* ---- test 9: link-down/reset/shutdown during TSO ---- */
 r=valid_request();r.tso_id=15;r.payload_len=4000;r.nfrags=2;
 r.frags[0].length=1054;r.frags[1].physical=EXT0+0x20000;r.frags[1].length=3000;
 send_tso_ok(&r,4); /* left in flight */
 assert(dma_stop()==0 && !dma_enabled());
 assert(regs[0x1114/4]==0 && regs[0x111c/4]==0);
 assert(dma_reclaim_full(&got_ext,&got_id)==0); /* reclaim after stop */
 assert(dma_start()==0);
 for (unsigned i=0;i<4;i++) { /* per-slot tso records were cleared */
  send_ordinary();device_complete(1);
  assert(dma_reclaim_full(&got_ext,&got_id)==1 && got_id==0);
 }
 r=valid_request();r.tso_id=16;send_tso_ok(&r,3); /* fresh ids work */
 device_complete(3);
 for (unsigned i=0;i<3;i++) {
  assert(dma_reclaim_full(&got_ext,&got_id)==1);
  assert(got_id==(i==2?16u:0));
 }
 assert(dma_reclaim_full(&got_ext,&got_id)==0);
 /* ---- test 8: capability probe (bit 18) and -21 while stopped ---- */
 assert(dma_stop()==0);
 assert(dma_send_tso(&r)==-21);
 regs[0x120/4]=5|(5<<6); /* GMAC_HW_TSOEN clear */
 assert(dma_start()==0);
 assert(dma_tso_available()==0);
 assert(!(regs[0x1104/4]&(1u<<12)));
 r=valid_request();r.tso_id=17;
 assert(dma_send_tso(&r)==-30 && dma_send_tso(NULL)==-27);
 memset(&cap,0xff,sizeof cap);
 dma_tso_capability_read(&cap);
 assert(cap.supported==0 && cap.ip_version_mask==0 && cap.max_payload==0);
 assert(dma_stop()==0);
 regs[0x120/4]=5|(5<<6)|(1u<<18);
 refuse_tse=1;
 assert(dma_start()==0 && !dma_tso_available() && dma_send_tso(&r)==-30);
 assert(!(regs[0x1104/4]&(1u<<12)));
 send_ordinary();drain(1);
 assert(dma_stop()==0);
 refuse_tse=0;
 assert(dma_start()==0);
 assert(dma_tso_available()==1);
 r=valid_request();r.tso_id=18;send_tso_ok(&r,3);drain(3);
 /* ring corruption: stale device ownership at the next slot is -26 */
 {
  struct dma_desc *d=(void *)(device+pslot*DMA_STRIDE);
  d->word[3]=0x80000000u;
  unsigned before=hw_writes,f0=exp_push;
  r=valid_request();r.tso_id=19;
  assert(dma_send_tso(&r)==-26 && hw_writes==before && exp_push==f0);
  d->word[3]=0;
 }
 r=valid_request();r.tso_id=20;send_tso_ok(&r,3);drain(3);
 assert(exp_pop==exp_push && f_pop==f_push);
 assert(cache_ext_calls==0);
 return 0;
}
