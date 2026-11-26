#define _POSIX_C_SOURCE 200809L
#include <assert.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>
#include "dma.h"
static _Alignas(4096) unsigned char cpu[DMA_SIZE],device[DMA_SIZE];
static uint32_t regs[2048];
#ifdef R5S_WAN_TX_IPV4_CHECKSUM
static unsigned expect_ipv4_checksum;
extern uint32_t dma_tx_ipv4_checksums, dma_tx_ipv4_checksum_active;
#endif
static uint64_t now;static unsigned reset_stuck,tx_stuck,fail_cache,tx_hw,rx_hw,cache_calls;
#ifdef R5S_GMAC_RK3568_AXI
static unsigned drop_axi_mixed;
#endif
#ifdef R5S_GMAC_EAME
static unsigned drop_axi_eame;
#endif
/* ---- external Normal non-cacheable pool (identity VA == PA, single view) ---- */
#define EXT0 0x24400000u
#define EXT_BUFFERS 8u
#define EXT_BUFFER 2048u
static unsigned char *ext;
static unsigned char *ext_at(uint64_t physical) {
 assert(physical>=EXT0 && physical+EXT_BUFFER<=EXT0+EXT_BUFFERS*EXT_BUFFER);
 return ext+(physical-EXT0);
}
/* TX expectations for external sends: device-visible address + TDES3
 * checksum field per descriptor, consumed by the device model on fetch. */
static uint32_t tx_ext_pa[DMA_SLOTS],tx_ext_cs[DMA_SLOTS];
static unsigned tx_ext_push,tx_ext_pop,tx_fault_bits;
uintptr_t dma_virtual(void) {return (uintptr_t)cpu;}
static uint64_t base=0x24000000;
uint64_t dma_physical(void) {return base;}
uintptr_t dma_ref_virtual(uint64_t physical) {
 if (physical>=base && physical<base+DMA_SIZE) return (uintptr_t)cpu+(physical-base);
 if (physical>=EXT0 && physical<EXT0+EXT_BUFFERS*EXT_BUFFER) return (uintptr_t)physical;
 return 0;
}
#ifdef R5S_ZC_WRITEBACK
static unsigned ref_invalidations, fail_ref_invalidation;
int dma_ref_invalidate(uint64_t physical) {
 assert(dma_ref_virtual(physical));
 ref_invalidations++;
 return fail_ref_invalidation;
}
#endif
extern uint32_t dma_evidence;
#ifdef R5S_WAN_RX_CHECKSUM
extern uint32_t dma_rx_checksum_protocol;
#endif
uint64_t hw_us(void) {return ++now;}
void hw_progress(void) {}
uint32_t hw_read(unsigned region,unsigned offset) {assert(region==0);return regs[offset/4];}
static size_t tx_payload_clean;
int dma_cache(unsigned operation,uintptr_t address,size_t size) {
 assert(address>=(uintptr_t)cpu && address+size<=(uintptr_t)cpu+DMA_SIZE && address%64==0 && size%64==0);
 cache_calls++;if (fail_cache) return 7;
 size_t offset=address-(uintptr_t)cpu;
 if (operation==2 && offset>=DMA_TX_BUFFER && offset<DMA_TX_BUFFER+DMA_SLOTS*DMA_BUFFER)
  tx_payload_clean=size;
 if (operation==0 || operation==2) memcpy(device+offset,cpu+offset,size);
 if (operation==1 || operation==2) memcpy(cpu+offset,device+offset,size);
 return 0;
}
void hw_write(unsigned region,unsigned offset,uint32_t value) {
 assert(dma_allowed(region,offset,value));
 if (offset==0x1160) {regs[offset/4]&=~value;return;}
 regs[offset/4]=value;
#ifdef R5S_GMAC_RK3568_AXI
 if (offset==0x1004 && drop_axi_mixed) regs[offset/4]&=~(1u<<14);
#endif
#ifdef R5S_GMAC_EAME
 if (offset==0x1004 && drop_axi_eame) regs[offset/4]&=~(1u<<11);
#endif
 if (offset==0x1000 && !reset_stuck) {regs[offset/4]=0;regs[0]=0;tx_hw=rx_hw=0;}
 if (offset==0x1120 && !tx_stuck) {
  unsigned tail=(value-0x24000000)/DMA_STRIDE;
  while (tx_hw!=tail) {
   struct dma_desc *d=(void *)(device+tx_hw*DMA_STRIDE);
   assert(d->word[3]&0x80000000);
   unsigned length=d->word[2];assert(length>=60 && length<=1514);
   uint32_t start=0x24000000+DMA_TX_BUFFER;
   if (d->word[0]>=start && d->word[0]<start+DMA_SLOTS*DMA_BUFFER) {
    assert(d->word[0]==0x24000000+DMA_TX_BUFFER+tx_hw*DMA_BUFFER);
    assert(device[DMA_TX_BUFFER+tx_hw*DMA_BUFFER]==0x5a);
#ifdef R5S_WAN_TX_IPV4_CHECKSUM
    assert(((d->word[3]>>16)&3)==expect_ipv4_checksum);
    if (expect_ipv4_checksum) {
     const unsigned char *frame=device+DMA_TX_BUFFER+tx_hw*DMA_BUFFER;
     assert(frame[24]==0 && frame[25]==0);
     if (expect_ipv4_checksum==3) assert(frame[40]==0 && frame[41]==0);
    }
#endif
   } else {
    /* external (Normal non-cacheable) TX: exact device-visible address and
     * checksum request from the expectation table; the frame must arrive
     * untouched (no memset, no CPU checksum zeroing). */
    assert(tx_ext_pop!=tx_ext_push);
    assert(d->word[0]==tx_ext_pa[tx_ext_pop]);
    assert(((d->word[3]>>16)&3)==(tx_ext_cs[tx_ext_pop]>>16));
    tx_ext_pop++;
    const unsigned char *frame=(const unsigned char *)(uintptr_t)d->word[0];
    assert(frame[0]==0x5a && frame[24]==0xba && frame[25]==0xda);
   }
   d->word[0]=0;d->word[3]=tx_fault_bits;tx_fault_bits=0;tx_hw=(tx_hw+1)%DMA_SLOTS;
  }
 }
}
static void inject(unsigned length,uint32_t flags) {
 struct dma_desc *d=(void *)(device+DMA_RX_DESC+rx_hw*DMA_STRIDE);assert(d->word[3]&0x80000000);
 memset(device+DMA_RX_BUFFER+rx_hw*DMA_BUFFER,0xa5,DMA_BUFFER);d->word[0]=0x123;d->word[3]=flags|length;rx_hw=(rx_hw+1)%DMA_SLOTS;
}
/* Device completion of a privately-armed RX slot. The frame is an IPv4/UDP
 * datagram so rx_protocol decoding is exercisable. */
static void complete_private(unsigned slot,unsigned length) {
 struct dma_desc *d=(void *)(device+DMA_RX_DESC+slot*DMA_STRIDE);assert(d->word[3]&0x80000000);
 assert(d->word[0]==0x24000000u+DMA_RX_BUFFER+slot*DMA_BUFFER);
 unsigned char *frame=device+DMA_RX_BUFFER+slot*DMA_BUFFER;
 memset(frame,0xa5,DMA_BUFFER);frame[12]=8;frame[13]=0;frame[14]=0x45;frame[23]=17;
 d->word[0]=0x123;d->word[1]=0x11;d->word[3]=0x30000000u|(1u<<26)|length;rx_hw=(slot+1)%DMA_SLOTS;
}
/* Device completion of an externally-armed RX slot: the frame lands in the
 * Normal non-cacheable buffer (same CPU/device view, no cache model). */
static void complete_ref(unsigned slot,uint64_t physical,unsigned length) {
 struct dma_desc *d=(void *)(device+DMA_RX_DESC+slot*DMA_STRIDE);assert(d->word[3]&0x80000000);
 assert(d->word[0]==(uint32_t)physical);
 unsigned char *frame=ext_at(physical);
 memset(frame,0xa5,EXT_BUFFER);frame[12]=8;frame[13]=0;frame[14]=0x45;frame[23]=17;
 d->word[0]=0x123;d->word[1]=0x11;d->word[3]=0x30000000u|(1u<<26)|length;rx_hw=(slot+1)%DMA_SLOTS;
}
int main(void) {
 regs[0x120/4]=5|(5<<6);
 base=UINT64_MAX-4095;assert(dma_start()==-21);base=0x24000000;
 reset_stuck=1;assert(dma_start()==-28 && !dma_enabled());reset_stuck=0;
 regs[0x110/4]=0x51;
 regs[0]=1u<<13;assert(dma_start()==0);
#ifdef R5S_GMAC_HYBRID_IRQ
 const unsigned irq_mask=(1u<<15)|(1u<<6);
 struct dma_desc *first=(void *)(device+DMA_RX_DESC);
 assert(first->word[3]&(1u<<30));
 assert(regs[0x1134/4]==0);
 dma_irq_end();
 assert(regs[0x1134/4]==irq_mask);
 regs[0x1160/4]=irq_mask|1;
 dma_irq_begin();
 assert(regs[0x1134/4]==0 && regs[0x1160/4]==1);
 regs[0x1160/4]|=irq_mask;
 dma_irq_end();
 assert(regs[0x1134/4]==irq_mask && regs[0x1160/4]==(irq_mask|1));
 assert(!dma_allowed(0,0x1160,1));
 assert(dma_stop()==0 && regs[0x1134/4]==0);
 regs[0x110/4]=0x41;
 assert(dma_start()==0);
 dma_irq_end();
 assert(regs[0x1134/4]==0 && !(first->word[3]&(1u<<30)));
 assert(dma_stop()==0);
 regs[0x110/4]=0x51;
 regs[0]=1u<<13;assert(dma_start()==0);
#endif
#ifdef R5S_GMAC_RK3568_AXI
 assert((regs[0x1004/4]&~(1u<<11))==0x0408400e);
 assert(dma_stop()==0);
 drop_axi_mixed=1;
 assert(dma_start()==0 && (regs[0x1004/4]&~(1u<<11))==0x0002000e);
 assert(dma_stop()==0);
 drop_axi_mixed=0;
 regs[0x110/4]=0x41;
 assert(dma_start()==0 && (regs[0x1004/4]&~(1u<<11))==0x0002000e);
 assert(dma_stop()==0);
 regs[0x110/4]=0x51;
 regs[0]=1u<<13;assert(dma_start()==0);
#else
 assert((regs[0x1004/4]&~(1u<<11))==0x0002000e);
#endif
#ifdef R5S_GMAC_EAME
 /* EAME (DMA_SysBus_Mode bit 11) is requested on recognized revisions; a
  * rejected setting falls back to the same mode without it, and revisions
  * outside 0x40-0x52 never request it. */
 assert(regs[0x1004/4]&(1u<<11));
 assert(dma_stop()==0);
 drop_axi_eame=1;
 assert(dma_start()==0 && !(regs[0x1004/4]&(1u<<11)));
 assert(dma_stop()==0);
 drop_axi_eame=0;
 regs[0x110/4]=0x3f;
 assert(dma_start()==0 && !(regs[0x1004/4]&(1u<<11)));
 assert(dma_stop()==0);
 regs[0x110/4]=0x51;
 regs[0]=1u<<13;
 assert(dma_start()==0 && (regs[0x1004/4]&(1u<<11)));
#endif
#ifdef R5S_GMAC_PBL
 assert(regs[0x1104/4]>>16==R5S_GMAC_PBL);
 assert(regs[0x1108/4]>>16==R5S_GMAC_PBL);
#endif
#ifdef R5S_GMAC_PBLX8
 assert(regs[0x1100/4]&(1u<<16));
 assert(dma_stop()==0);
 regs[0x120/4]=1|(1<<6);
 assert(dma_start()==0);
 assert(!(regs[0x1100/4]&(1u<<16)));
 assert(dma_stop()==0);
 regs[0x120/4]=5|(5<<6);
 regs[0]=1u<<13;
 assert(dma_start()==0);
#endif
assert(cache_calls>64 && (regs[0]&(1u<<13)));
 unsigned char packet[1514],output[1514];memset(packet,0x5a,sizeof(packet));
 dma_begin_tx_batch();
 assert(dma_send(packet,60)==60 && dma_send(packet,60)==60);
 assert(tx_hw==0 && dma_reclaim()==0);
 dma_end_tx_batch();
 assert(tx_hw==2 && dma_reclaim()==1 && dma_reclaim()==1 && dma_reclaim()==0);
 unsigned short_offset=DMA_TX_BUFFER+tx_hw*DMA_BUFFER;
 memset(cpu+short_offset,0xa9,DMA_BUFFER);
 assert(dma_send(packet,14)==60 && tx_payload_clean==64);
 for (unsigned i=14;i<60;i++) assert(device[short_offset+i]==0);
 assert(device[short_offset+60]==0xa9 && dma_reclaim()==1);
 assert(dma_send(packet,sizeof(packet))==1514 && tx_payload_clean==1536);
 assert(dma_reclaim()==1);
 for (unsigned round=0;round<100;round++) {
  for (unsigned i=0;i<63;i++) assert(dma_send(packet,14+i%100)==(int)(14+i%100<60?60:14+i%100));
  assert(!dma_tx_available() && dma_send(packet,60)==0);
  for (unsigned i=0;i<63;i++) assert(dma_reclaim()==1);
  assert(dma_reclaim()==0);
 }
 memset(packet,0,300);packet[0]=0x5a;packet[12]=8;packet[14]=0x45;packet[23]=17;
 packet[35]=68;packet[37]=67;packet[42]=1;
 memcpy(packet+278,"\x63\x82\x53\x63\x35\x01\x01\xff",8);
 assert(dma_tx_available() && dma_send(packet,300)==300 && !(dma_evidence&16));
 assert(dma_reclaim()==1 && (dma_evidence&19)==19);
 for (unsigned i=0;i<2000;i++) {
  inject(1514,0x30000000);assert(dma_receive(output,sizeof(output))==1514);
  for (unsigned j=0;j<sizeof(output);j++) assert(output[j]==0xa5);
 }
 inject(32767,0x30000000);assert(dma_receive(output,sizeof(output))==-1);
 inject(100,0x30008000);assert(dma_receive(output,sizeof(output))==-1);
 inject(100,0x10000000);assert(dma_receive(output,sizeof(output))==-1);
 assert(dma_stop()==0);assert(!dma_enabled());
 assert(regs[0x1114/4]==0 && regs[0x111c/4]==0);
#ifdef R5S_WAN_TX_IPV4_CHECKSUM
 regs[0x11c/4]=1<<14;
#endif
 #ifdef R5S_WAN_RX_CHECKSUM
 regs[0x11c/4]|=1<<16;
 #endif
 assert(dma_start()==0);
 #ifdef R5S_WAN_RX_CHECKSUM
 for (unsigned i=0;i<4;i++) {
  unsigned slot=rx_hw;
  inject(64,0x30000000|(i==0?0:1u<<26));
  struct dma_desc *r=(void *)(device+DMA_RX_DESC+slot*DMA_STRIDE);
  unsigned char *frame=device+DMA_RX_BUFFER+slot*DMA_BUFFER;
  frame[12]=8;frame[13]=0;frame[14]=0x45;frame[23]=17;
  r->word[1]=0x11|(i==1?1u<<6:0)|(i==2?1u<<7:0);
  assert(dma_receive(output,sizeof(output))==64);
  assert(dma_rx_checksum_protocol==(i==3?17:0));
 }
 #endif
#ifdef R5S_WAN_TX_IPV4_CHECKSUM
 assert(dma_tx_ipv4_checksum_active==1);
 packet[16]=0;packet[17]=20;packet[24]=0xba;packet[25]=0xda;
 expect_ipv4_checksum=1;
 assert(dma_send(packet,34)==60 && dma_reclaim()==1);
 assert(packet[24]==0xba && packet[25]==0xda && dma_tx_ipv4_checksums==1);
 expect_ipv4_checksum=0;
 packet[24]^=1;
 assert(dma_send(packet,60)==60 && dma_reclaim()==1);
 packet[24]^=1;
 packet[20]=0x20;
 assert(dma_send(packet,60)==60 && dma_reclaim()==1);
 assert(dma_tx_ipv4_checksums==1);
 packet[20]=0;packet[17]=28;packet[25]=0xd2;
 packet[39]=8;packet[40]=0xff;packet[41]=0x57;
 expect_ipv4_checksum=3;
 assert(dma_send(packet,42)==60 && dma_reclaim()==1);
 assert(dma_tx_ipv4_checksums==2);
 expect_ipv4_checksum=0;
 unsigned validated_offset=DMA_TX_BUFFER+tx_hw*DMA_BUFFER;
 assert(dma_send_nat_validated(packet,42)==60 && dma_reclaim()==1);
 assert(!memcmp(device+validated_offset,packet,42));
 assert(dma_tx_ipv4_checksums==2);
 packet[40]=packet[41]=0;
 validated_offset=DMA_TX_BUFFER+tx_hw*DMA_BUFFER;
 assert(dma_send_nat_validated(packet,42)==60 && dma_reclaim()==1);
 assert(!memcmp(device+validated_offset,packet,42));
#endif
 regs[0x1160/4]=1<<12;assert(dma_reclaim()==-24);regs[0x1160/4]=0;
 tx_stuck=1;assert(dma_send(packet,60)==60);now+=2000001;assert(dma_reclaim()==-25);
 fail_cache=1;assert(dma_send(packet,100)==-20);fail_cache=0;
 regs[0xd08/4]=1u<<1;
 uint64_t stop_start=now;
 assert(dma_stop()==-23 && dma_enabled());
 assert(now-stop_start<=100002);
 assert(regs[0x1114/4]!=0 && regs[0x111c/4]!=0);
 regs[0xd08/4]=0;
 assert(dma_stop()==0);regs[0x120/4]=0;assert(dma_start()==-22);
 /* ---------------- external zero-copy buffer API ---------------- */
 tx_stuck=0;fail_cache=0;
 ext=mmap((void *)(uintptr_t)EXT0,EXT_BUFFERS*EXT_BUFFER,PROT_READ|PROT_WRITE,
          MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED,-1,0);
 assert(ext==(unsigned char *)(uintptr_t)EXT0);
 unsigned s=0,calls;uint64_t p=0,got=0;size_t l=0;uint32_t pr=0;int external=0;
 /* running violations: -21 while stopped */
 assert(dma_arm_ref(0,EXT0)==-21);
 assert(dma_peek(&s,&p,&l,&pr,&external)==-21);
 assert(dma_take_ref(0,EXT0)==-21);
 assert(dma_send_ref(EXT0,60,0)==-21);
 got=123;assert(dma_reclaim()==0);got=123;assert(dma_reclaim_ex(&got)==0 && got==0);
 regs[0x120/4]=5|(5<<6);assert(dma_start()==0);
 /* input validation while running: -27 */
 assert(dma_arm_ref(DMA_SLOTS,EXT0)==-27);
 assert(dma_arm_ref(0,0)==-27 && dma_send_ref(0,60,0)==-27);
 assert(dma_arm_ref(0,EXT0+1)==-27);
 assert(dma_arm_ref(0,EXT0+32)==-27);
 assert(dma_arm_ref(0,0x100000000ull)==-27);
 assert(dma_send_ref(EXT0,13,0)==-27);
 assert(dma_send_ref(EXT0,1515,0)==-27);
 assert(dma_send_ref(EXT0+1,60,0)==-27);
 assert(dma_send_ref(EXT0,60,2u<<16)==-27);
 assert(dma_take_ref(DMA_SLOTS,EXT0)==-27);
 assert(dma_take_ref(0,EXT0+3)==-27);
 assert(dma_peek(NULL,&p,&l,&pr,&external)==-27);
 assert(dma_peek(&s,&p,&l,&pr,NULL)==-27);
 /* ownership violation: dma_arm_ref on a device-owned slot changes nothing */
 struct dma_desc *d0=(void *)(device+DMA_RX_DESC);uint32_t keep=d0->word[0];
 assert(d0->word[3]&0x80000000u);
 calls=cache_calls;assert(dma_arm_ref(0,EXT0)==-21);
 assert(cache_calls==calls+1 && (d0->word[3]&0x80000000u) && d0->word[0]==keep);
 /* test 1: arm_ref publishes the external address in descriptor word[0]
    (32-bit identity) and touches neither the buffer nor its cache state */
 complete_private(0,100);
 memset(ext_at(EXT0),0x5a,EXT_BUFFER);
 calls=cache_calls;assert(dma_arm_ref(0,EXT0)==0);
 assert(cache_calls==calls+2);        /* observe + descriptor clean, no buffer ops */
 assert(d0->word[0]==(uint32_t)EXT0);
 assert(ext_at(EXT0)[0]==0x5a && ext_at(EXT0)[100]==0x5a);
 /* test 2: peek reports an external completion without copying or consuming */
 complete_ref(0,EXT0,100);
#ifdef R5S_ZC_WRITEBACK
 unsigned invalidations=ref_invalidations;
 fail_ref_invalidation=1;
 assert(dma_peek(&s,&p,&l,&pr,&external)==-20);
 fail_ref_invalidation=0;
#endif
 calls=cache_calls;
 assert(dma_peek(&s,&p,&l,&pr,&external)==1);
#ifdef R5S_ZC_WRITEBACK
 assert(ref_invalidations==invalidations+2);
#endif
 assert(s==0 && p==EXT0 && l==100 && external==1);
#ifdef R5S_WAN_RX_CHECKSUM
 assert(pr==17);
#else
 assert(pr==0);
#endif
 assert(cache_calls==calls+1);        /* descriptor line only */
 assert(ext_at(EXT0)[12]==8 && ext_at(EXT0)[23]==17);  /* buffer untouched */
 assert(dma_peek(&s,&p,&l,&pr,&external)==1 && s==0);  /* cursor not advanced */
 assert(dma_take_ref(0,EXT0+EXT_BUFFER)==0);
 assert(d0->word[0]==(uint32_t)(EXT0+EXT_BUFFER) && (d0->word[3]&0x80000000u));
 assert(ext_at(EXT0)[12]==8);         /* completed buffer not touched */
 assert(dma_receive(output,sizeof(output))==0);  /* cursor advanced past slot 0 */
 /* take_ref(...,0) falls back to the private pool buffer (legacy memset) */
 complete_private(1,70);
 assert(dma_arm_ref(1,EXT0+2*EXT_BUFFER)==0);
 complete_ref(1,EXT0+2*EXT_BUFFER,70);
 assert(dma_peek(&s,&p,&l,&pr,&external)==1);
 assert(s==1 && p==EXT0+2*EXT_BUFFER && l==70 && external==1);
 assert(dma_take_ref(1,0)==0);
 {
  struct dma_desc *d1=(void *)(device+DMA_RX_DESC+DMA_STRIDE);
  assert(d1->word[0]==0x24000000u+DMA_RX_BUFFER+DMA_BUFFER);
  assert(device[DMA_RX_BUFFER+DMA_BUFFER]==0);  /* memset+clean restored */
 }
 assert(ext_at(EXT0+2*EXT_BUFFER)[12]==8);  /* completed buffer not touched */
 /* test 3: mixed-mode cursor discipline at a private-then-external pair */
 complete_private(2,60);
 complete_private(3,80);
 assert(dma_arm_ref(3,EXT0+3*EXT_BUFFER)==0);
 complete_ref(3,EXT0+3*EXT_BUFFER,80);
 s=99;external=-1;
 assert(dma_peek(&s,&p,&l,&pr,&external)==0);  /* private slot: not peek's */
 assert(s==2 && external==0 && l==60);         /* *external=0 is reported */
#ifdef R5S_WAN_RX_CHECKSUM
 assert(pr==17);
#else
 assert(pr==0);
#endif
 assert(dma_receive(output,sizeof(output))==60);  /* dma_receive consumes it */
 assert(output[12]==8 && output[23]==17);
 assert(dma_receive(output,sizeof(output))==0);   /* never steals external */
 assert(dma_peek(&s,&p,&l,&pr,&external)==1 && s==3 && external==1 && l==80);
 assert(dma_take_ref(3,0)==0);
 complete_private(4,60);
 assert(dma_arm_ref(4,EXT0+3*EXT_BUFFER)==0);
 complete_ref(4,EXT0+3*EXT_BUFFER,13);
 assert(dma_peek(&s,&p,&l,&pr,&external)==-26 && s==4 && external==1);
 assert(dma_take_ref(s,p)==0);
 assert(dma_peek(&s,&p,&l,&pr,&external)==0);
 /* test 4: dma_send_ref publishes the external address and never touches
    the frame; dma_reclaim_ex reports the completed external address */
 unsigned char *frame=ext_at(EXT0+4*EXT_BUFFER);
 memset(frame,0x5a,EXT_BUFFER);frame[24]=0xba;frame[25]=0xda;
 tx_ext_pa[tx_ext_push]=(uint32_t)(EXT0+4*EXT_BUFFER);tx_ext_cs[tx_ext_push]=0;tx_ext_push++;
 assert(dma_send_ref(EXT0+4*EXT_BUFFER,60,0)==60);
 assert(frame[0]==0x5a && frame[24]==0xba && frame[25]==0xda);
 got=0;assert(dma_reclaim_ex(&got)==1 && got==EXT0+4*EXT_BUFFER);
 assert(frame[24]==0xba && frame[25]==0xda);
 frame=ext_at(EXT0+5*EXT_BUFFER);
 memset(frame,0x5a,EXT_BUFFER);frame[24]=0xba;frame[25]=0xda;
 tx_ext_pa[tx_ext_push]=(uint32_t)(EXT0+5*EXT_BUFFER);tx_ext_cs[tx_ext_push]=1u<<16;tx_ext_push++;
 assert(dma_send_ref(EXT0+5*EXT_BUFFER,64,1u<<16)==64);
 assert(frame[24]==0xba && frame[25]==0xda);  /* checksum fields never zeroed */
 got=0;assert(dma_reclaim_ex(&got)==1 && got==EXT0+5*EXT_BUFFER);
 tx_ext_pa[tx_ext_push]=(uint32_t)(EXT0+5*EXT_BUFFER);tx_ext_cs[tx_ext_push]=3u<<16;tx_ext_push++;
 assert(dma_send_ref(EXT0+5*EXT_BUFFER,64,3u<<16)==64);
 got=0;assert(dma_reclaim_ex(&got)==1 && got==EXT0+5*EXT_BUFFER);
 assert(frame[24]==0xba && frame[25]==0xda);
 memset(packet,0x5a,60);
 assert(dma_send(packet,60)==60);
 got=123;assert(dma_reclaim_ex(&got)==1 && got==0);
 assert(dma_send(packet,60)==60 && dma_reclaim()==1);
 uint32_t evidence=dma_evidence;
 tx_ext_pa[tx_ext_push]=(uint32_t)(EXT0+4*EXT_BUFFER);tx_ext_cs[tx_ext_push]=0;tx_ext_push++;
 assert(dma_send_ref(EXT0+4*EXT_BUFFER,60,0)==60);
 got=0;assert(dma_reclaim_ex(&got)==1 && got==EXT0+4*EXT_BUFFER);
 assert(((dma_evidence^evidence)&16)==0);  /* no discover flag on external sends */
 /* test 6: -26 writeback error then -25 completion timeout */
 frame=ext_at(EXT0+6*EXT_BUFFER);
 memset(frame,0x5a,EXT_BUFFER);frame[24]=0xba;frame[25]=0xda;
 tx_ext_pa[tx_ext_push]=(uint32_t)(EXT0+6*EXT_BUFFER);tx_ext_cs[tx_ext_push]=0;tx_ext_push++;
 tx_fault_bits=0x8000;
 assert(dma_send_ref(EXT0+6*EXT_BUFFER,60,0)==60);
 tx_fault_bits=0;
 got=123;assert(dma_reclaim_ex(&got)==-26 && got==0);
 assert(dma_reclaim()==-26);
 {
  unsigned fixed=0;
  for (unsigned i=0;i<DMA_SLOTS;i++) {
   struct dma_desc *d=(void *)(device+i*DMA_STRIDE);
   if (d->word[3]==0x8000u) {d->word[3]=0;fixed++;}
  }
  assert(fixed==1);
 }
 got=0;assert(dma_reclaim_ex(&got)==1 && got==EXT0+6*EXT_BUFFER);
 frame=ext_at(EXT0+7*EXT_BUFFER);
 memset(frame,0x5a,EXT_BUFFER);frame[24]=0xba;frame[25]=0xda;
 tx_ext_pa[tx_ext_push]=(uint32_t)(EXT0+7*EXT_BUFFER);tx_ext_cs[tx_ext_push]=0;tx_ext_push++;
 tx_stuck=1;
 assert(dma_send_ref(EXT0+7*EXT_BUFFER,60,0)==60);
 now+=2000001;
 got=123;assert(dma_reclaim_ex(&got)==-25 && got==0);
 assert(dma_reclaim()==-25);
 uint64_t stopped_rx=1,stopped_tx=1;
 assert(dma_take_stopped_refs(0,&stopped_rx,&stopped_tx)==0 && !stopped_rx && !stopped_tx);
 regs[0xd08/4]=16;
 assert(dma_stop()==-23);
 assert(dma_take_stopped_refs(0,&stopped_rx,&stopped_tx)==0 && !stopped_rx && !stopped_tx);
 regs[0xd08/4]=0;
 assert(dma_stop()==0);
 assert(dma_start()==-21);
 unsigned nrx=0,ntx=0;
 for (unsigned i=0;i<DMA_SLOTS;i++) {
  assert(dma_take_stopped_refs(i,&stopped_rx,&stopped_tx)==1);
  if (stopped_rx) {assert(stopped_rx==EXT0+EXT_BUFFER || stopped_rx==EXT0+3*EXT_BUFFER);nrx++;}
  if (stopped_tx) {assert(stopped_tx==EXT0+7*EXT_BUFFER);ntx++;}
  assert(dma_take_stopped_refs(i,&stopped_rx,&stopped_tx)==1 && !stopped_rx && !stopped_tx);
 }
 assert(nrx==2 && ntx==1);
 tx_stuck=0;
 assert(dma_start()==0 && dma_reclaim()==0);
#ifdef R5S_GMAC_TWO_TX
 assert(dma_tx_channels()==1 && dma_select_tx(1)<0);
 assert(dma_stop()==0);
 regs[0x110/4]=0x51;
 regs[0x120/4]=(6u<<6)|6u;
 regs[0x124/4]=(1u<<18)|(1u<<6);
 assert(dma_start()==0 && dma_tx_channels()==2);
 assert(regs[0x1194/4]==base+0x32000 && regs[0x11ac/4]==31);
 assert(dma_select_tx(1)==0);
 for (unsigned i=0;i<33;i++) {
  unsigned offset=0x32000+(i%32)*DMA_STRIDE;
  struct dma_desc *secondary=(void *)(device+offset);
  assert(dma_send_ref(EXT0,60,0)==60);
  assert(regs[0x11a0/4]==base+0x32000+((i+1)%32)*DMA_STRIDE);
  assert((secondary->word[3]&0x80000000) && secondary->word[0]==EXT0);
  assert(dma_select_tx(0)==0 && dma_reclaim()==0);
  assert(dma_select_tx(1)==0);
  secondary->word[3]=0;
  assert(dma_reclaim_ex(&got)==1 && got==EXT0);
 }
 unsigned char copied[60];
 memset(copied,0x5a,sizeof copied);
 assert(dma_send(copied,sizeof copied)==60);
 struct dma_desc *second=(void *)(device+0x32000+DMA_STRIDE);
 assert(second->word[0]==base+0x33000+DMA_BUFFER);
 assert(memcmp(device+0x33000+DMA_BUFFER,copied,sizeof copied)==0);
 second->word[3]=0;
 assert(dma_reclaim_ex(&got)==1 && got==0);
 assert(dma_send_ref(EXT0,60,0)==60);
 regs[0xd48/4]=16;
 assert(dma_stop()==-23);
 assert(!dma_take_stopped_refs(DMA_SLOTS+2,&stopped_rx,&stopped_tx));
 regs[0xd48/4]=0;
 assert(dma_stop()==0);
 assert(dma_take_stopped_refs(DMA_SLOTS+2,&stopped_rx,&stopped_tx)==1 && stopped_tx==EXT0);
 assert(dma_start()==0 && dma_tx_channels()==2);
#endif
#ifdef R5S_GMAC_TWO_RX
 assert(dma_rx_channels()==1 && dma_select_rx(1)<0);
 assert(dma_stop()==0);
 regs[0x110/4]=0x51;
 regs[0x120/4]=(6u<<6)|6u;
 regs[0x124/4]|=(1u<<12)|1u;
 assert(dma_start()==0 && dma_rx_channels()==2);
 assert(regs[0xa0/4]==0xa && regs[0xa4/4]==0x110000 && regs[0xc30/4]==0x100);
 assert(regs[0x1130/4]==31 && regs[0x11b0/4]==31 && regs[0x119c/4]==base+0x1800);
 assert(dma_select_rx(1)==0);
 for (unsigned i=0;i<33;i++) {
  unsigned slot=i%32, index=32+slot;
  struct dma_desc *receive=(void *)(device+DMA_RX_DESC+index*DMA_STRIDE);
  assert((receive->word[3]&0x80000000) && receive->word[0]==base+DMA_RX_BUFFER+index*DMA_BUFFER);
  memset(device+DMA_RX_BUFFER+index*DMA_BUFFER,0x3c,60);
  receive->word[3]=0x30000000u|60;
  assert(dma_select_rx(0)==0 && dma_peek(&s,&p,&l,&pr,&external)==0);
  assert(dma_select_rx(1)==0 && dma_peek(&s,&p,&l,&pr,&external)==0 && s==slot);
  unsigned char packet[60];
  assert(dma_receive(packet,sizeof packet)==60 && packet[0]==0x3c);
  assert(regs[0x11a8/4]==base+DMA_RX_DESC+index*DMA_STRIDE);
 }
 struct dma_desc *q1_next=(void *)(device+DMA_RX_DESC+33*DMA_STRIDE);
 q1_next->word[3]=0x30000000u|60;
 assert(dma_arm_ref(1,EXT0)==0);
 regs[0xd78/4]=1u<<16;
 assert(dma_stop()==-23);
 assert(!dma_take_stopped_refs(33,&stopped_rx,&stopped_tx));
 regs[0xd78/4]=0;
 assert(dma_stop()==0);
 assert(dma_take_stopped_refs(33,&stopped_rx,&stopped_tx)==1 && stopped_rx==EXT0);
 assert(dma_start()==0 && dma_rx_channels()==2);
#endif
 return 0;
}
