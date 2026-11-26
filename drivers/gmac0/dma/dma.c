/* SPDX-License-Identifier: BSD-2-Clause */
#include "dma.h"
#include "../link.h"
#include <string.h>
#define OWN 0x80000000u
#define FD 0x20000000u
#define LD 0x10000000u
#define CTX 0x40000000u
#define TCMSSV 0x04000000u
#define TSE 0x00040000u
#define TX_CHANNEL_TSE (1u<<12)
#define RX_IOC (1u<<30)
#define TX_IOC (1u<<31)
#define IRQ_NORMAL (1u<<15)
#define IRQ_RX (1u<<6)
#define IRQ_TX 1u
struct tx_ring {
 unsigned producer,consumer,pending;
 unsigned batching,unposted,clean_start;
 unsigned desc_arm;
 unsigned char discover[DMA_SLOTS],context[DMA_SLOTS];
 uint32_t tso_last[DMA_SLOTS],external[DMA_SLOTS];
 uint64_t sent_at[DMA_SLOTS],batch_started;
};
#define TX1_SLOTS 32u
#define TX1_DESC (DMA_RX_BUFFER+DMA_SLOTS*DMA_BUFFER)
#define TX1_BUFFER (TX1_DESC+0x1000u)
_Static_assert(TX1_DESC >= DMA_RX_BUFFER + DMA_SLOTS*DMA_BUFFER, "independent TX1 descriptors");
_Static_assert(TX1_BUFFER + TX1_SLOTS*DMA_BUFFER <= DMA_SIZE, "TX1 payload arena");
static struct tx_ring tx_rings[2];
static unsigned selected_tx, tx_channels = 1;
#define tx (tx_rings[selected_tx])
static unsigned tx_slots(void) {return selected_tx ? TX1_SLOTS : DMA_SLOTS;}
static unsigned tx_dma_offset(void) {return selected_tx * 0x80;}
unsigned dma_tx_channels(void) {return tx_channels;}
int dma_select_tx(unsigned channel) {
 if (channel>=tx_channels) return -21;
 selected_tx=channel;
 return 0;
}
static unsigned running,tso_active;
static unsigned selected_rx,rx_channels=1,rx_indices[2];
static unsigned rx_ready_to_take[2];
static int rx_batching;
static unsigned rx_unposted,rx_desc_arm,rx_batch_start;
#define rx_index (rx_indices[selected_rx])
static unsigned rx_slots(void) {return DMA_SLOTS/rx_channels;}
static unsigned rx_first(void) {return selected_rx*rx_slots();}
static unsigned rx_dma_offset(void) {return selected_rx*0x80;}
unsigned dma_rx_channels(void) {return rx_channels;}
int dma_select_rx(unsigned channel) {
 if (channel>=rx_channels) return -21;
 if (rx_batching && dma_end_rx_batch()) return -20;
 selected_rx=channel;
 return 0;
}
uint32_t dma_rx_irq;
static uint32_t rx_external[DMA_SLOTS];
uint32_t dma_evidence;
uint32_t dma_tx_ipv4_checksums, dma_tx_ipv4_checksum_active;
uint32_t dma_rx_checksum_protocol;
static unsigned rx_checksum_active;
static int discover(const unsigned char *p,size_t n) {
 if (n<282 || p[12]!=8 || p[13]!=0 || p[14]!=0x45 || p[23]!=17 ||
     p[34] || p[35]!=68 || p[36] || p[37]!=67 || p[42]!=1 ||
     memcmp(p+278,"\x63\x82\x53\x63",4)) return 0;
 for (size_t i=282;i<n;) {
  unsigned type=p[i++];if (!type) continue;
  if (type==255 || i==n) break;
  unsigned size=p[i++];if (size>n-i) break;
  if (type==53) return size==1 && p[i]==1;
  i+=size;
 }
 return 0;
}
uint64_t dma_rx_packets,dma_tx_packets,dma_drops;
uint32_t dma_rx_retired[2],dma_tx_retired[2];
_Static_assert(sizeof(struct dma_desc)==DMA_STRIDE,"cache-line exclusive descriptor");
static struct dma_desc *desc(unsigned rx,unsigned i) {return (struct dma_desc *)(dma_virtual()+(rx?DMA_RX_DESC+rx_first()*DMA_STRIDE:selected_tx?TX1_DESC:0)+i*DMA_STRIDE);}
static uintptr_t buffer(unsigned rx,unsigned i) {return dma_virtual()+(rx?DMA_RX_BUFFER+rx_first()*DMA_BUFFER:selected_tx?TX1_BUFFER:DMA_TX_BUFFER)+i*DMA_BUFFER;}
static uint32_t bus(uintptr_t address) {return (uint32_t)(dma_physical()+address-dma_virtual());}
static int cache(unsigned op,uintptr_t address,size_t size) {return dma_cache(op,address,size)?-20:0;}
static int desc_cache(unsigned op,struct dma_desc *d,size_t size) {
#ifdef R5S_GMAC_DMA_NC
 (void)op;(void)d;(void)size;
 return 0;
#else
 return cache(op,(uintptr_t)d,size);
#endif
}
static uint32_t rx_status(const struct dma_desc *d) {
 uint32_t status=d->word[3];
#ifdef R5S_GMAC_DMA_NC
 if (!(status&OWN)) __asm__ volatile("dmb oshld" ::: "memory");
#endif
 return status;
}
static void desc_own(struct dma_desc *d,uint32_t status) {
#ifdef R5S_GMAC_DMA_NC
 __asm__ volatile("dmb oshst" ::: "memory");
#endif
 d->word[3]=status;
}
static void ring_doorbell(unsigned offset,struct dma_desc *d) {
#ifdef R5S_GMAC_DMA_NC
 __asm__ volatile("dsb oshst" ::: "memory");
#endif
 hw_write(0,offset,bus((uintptr_t)d));
}
#define MAINT_BATCH 16u
_Static_assert(MAINT_BATCH < DMA_SLOTS/2, "RX batch must not lap the ring");

static int rx_flush(void) {
 rx_desc_arm=0;
 if (!rx_unposted) return 0;
 unsigned first=rx_batch_start;
 unsigned count=rx_unposted;
 unsigned tail=rx_slots()-first;
 if (count<tail) tail=count;
 if (desc_cache(0,desc(1,first),tail*DMA_STRIDE)) return -20;
 if (count>tail && desc_cache(0,desc(1,0),(count-tail)*DMA_STRIDE)) return -20;
 ring_doorbell(0x1128+rx_dma_offset(),desc(1,(rx_index+rx_slots()-1)%rx_slots()));
 rx_unposted=0;
 rx_batch_start=rx_index;
 return 0;
}
static int rx_desc_ready(void) {
 if (rx_unposted==MAINT_BATCH && rx_flush()) return -20;
 if (rx_batching && rx_desc_arm) return 0;
 unsigned span=rx_batching ? MAINT_BATCH-rx_unposted : 1;
 unsigned left=rx_slots()-rx_index;
 if (span>left) span=left;
 if (desc_cache(1,desc(1,rx_index),span*DMA_STRIDE)) return -20;
 rx_desc_arm=span;
 return 0;
}
static void rx_advance(void) {
 rx_index=(rx_index+1)%rx_slots();
 if (rx_desc_arm) rx_desc_arm--;
}
void dma_begin_rx_batch(void) {
 rx_batching=1;
 rx_batch_start=rx_index;
 rx_desc_arm=0;
}
int dma_end_rx_batch(void) {
 if (!rx_batching) return 0;
 if (rx_flush()) return -20;
 rx_batching=0;
 return 0;
}
static void rx_publish(unsigned slot) {
 if (rx_batching) {rx_unposted++;return;}
 ring_doorbell(0x1128+rx_dma_offset(),desc(1,slot));
}
/* External-buffer classification: the descriptor word[0] is the record; a
 * descriptor is private when it points into the copy-buffer range of its
 * direction and external otherwise (shared pool buffers live outside). */
static int tx_private(uint32_t address) {
 uint32_t start=bus(buffer(0,0));return address>=start && address<start+tx_slots()*DMA_BUFFER;
}
/* Same rx_protocol decoding as dma_receive: checksum-offload writeback
 * verified against the frame's IPv4 header. */
static uint32_t rx_protocol_of(const unsigned char *frame,size_t length,uint32_t status,uint32_t writeback) {
 if (rx_checksum_active && (status&(1u<<26)) && (writeback&0xf8)==0x10
     && length>=34 && frame[12]==8 && !frame[13] && frame[14]==0x45) {
  unsigned type=writeback&7;
  if (type==1 && frame[23]==17) return 17;
  if (type==2 && frame[23]==6) return 6;
 }
 return 0;
}
unsigned dma_tx_space(void) {return running ? tx_slots()-1-tx.pending : 0;}
int dma_tx_available(void) {return dma_tx_space()!=0;}
unsigned dma_tx_pending(void) {return tx.pending;}
unsigned dma_tx_capacity(void) {return tx_slots()-1;}
int dma_enabled(void) {return running;}
void dma_begin_tx_batch(void) {
 /* Idempotent: a repeated begin inside one batch must not reset the span
  * start, or descriptors prepared earlier in the batch lose their clean. */
 if (!tx.batching) tx.clean_start=tx.producer;
 tx.batching=1;
}
/* Inside a TX batch the span clean at dma_end_tx_batch covers every prepared
 * descriptor; outside one the per-descriptor clean stays synchronous. */
static int defer_tx_desc_clean(struct dma_desc *d) {
 return tx.batching ? 0 : desc_cache(2,d,DMA_STRIDE);
}
int dma_end_tx_batch(void) {
 int batching=tx.batching;
 tx.batching=0;
 if (!tx.unposted) return 0;
 if (batching) {
  /* One wrap-aware span clean for every descriptor prepared in the batch. */
  unsigned first=tx.clean_start;
  unsigned count=(tx.producer+tx_slots()-first)%tx_slots();
  unsigned tail=tx_slots()-first;
  if (count<tail) tail=count;
  if (desc_cache(0,desc(0,first),tail*DMA_STRIDE)) return -20;
  if (count>tail && desc_cache(0,desc(0,0),(count-tail)*DMA_STRIDE)) return -20;
 }
 tx.clean_start=tx.producer;
 /* One publication barrier orders every payload and descriptor write of the
  * batch before the doorbell may expose the descriptors to the device. */
 __asm__ volatile("dsb sy" ::: "memory");
 ring_doorbell(0x1120+tx_dma_offset(),desc(0,tx.producer));
 tx.unposted=0;
 return 0;
}
static uint64_t tx_submit_time(void) {
 if (!tx.unposted) tx.batch_started=hw_us();
 return tx.batch_started;
}
static void post_tx(unsigned slots) {
 tx.unposted+=slots;
 if (!tx.batching) dma_end_tx_batch();
}
static uint32_t channel_pbl = 8u << 16;
static uint32_t tx_channel = 8u << 16;
static uint32_t channel_control = 6u << 18;
#define AXI_BASELINE ((2u<<16)|14u)
#define AXI_RK3568 ((4u<<24)|(8u<<16)|(1u<<14)|14u)
/* DMA_SysBus_Mode bit 11 (EAME), programmed by the pinned U-Boot EQoS init
 * (dwc_eth_qos.c "DMA performance configuration") on this IP. */
#define AXI_EAME (1u<<11)
#define AXI_CONFIG_MASK ((15u<<24)|(15u<<16)|(1u<<14)|AXI_EAME|0xfeu)
static uint32_t axi_mode = AXI_BASELINE;
static uint32_t tx_mtl;
static uint32_t irq_mask(unsigned channel) {
 uint32_t events=(channel<rx_channels ? IRQ_RX : 0) | (channel<tx_channels ? IRQ_TX : 0);
 return dma_rx_irq && events ? IRQ_NORMAL | events : 0;
}
static uint32_t tx_ioc(void) {
 return dma_rx_irq && (!tx.pending || tx.producer % 8 == 0) ? TX_IOC : 0;
}
static int allow_secondary_tx(unsigned offset,uint32_t value,uint32_t base) {
 if (tx_channels!=2) return 0;
 switch (offset) {
 case 0xd40:return value==tx_mtl;
 case 0xd18:case 0xd58:return value==16;
 case 0x1180:return value==channel_control;
 case 0x1184:return value==channel_pbl || value==(channel_pbl|1u);
 case 0x1190:return value==0;
 case 0x1194:return value==0 || value==base+TX1_DESC;
 case 0x11ac:return value==TX1_SLOTS-1;
 case 0x11a0:return value>=base+TX1_DESC && value<base+TX1_DESC+TX1_SLOTS*DMA_STRIDE
                       && (value-base-TX1_DESC)%DMA_STRIDE==0;
 default:return 0;
 }
}
static int allow_secondary_rx(unsigned offset,uint32_t value,uint32_t base) {
 if (rx_channels!=2) return 0;
 const uint32_t first=base+DMA_RX_DESC+(DMA_SLOTS/2)*DMA_STRIDE;
 switch (offset) {
 case 0xc30:return value==0x100;
 case 0xd70:return (value&~0x3ff00020u)==0 && (value&32);
 case 0x1180:return value==channel_control;
 case 0x1188:return value==(channel_pbl|(DMA_BUFFER<<1)) || value==(channel_pbl|(DMA_BUFFER<<1)|1u);
 case 0x1198:return value==0;
 case 0x119c:return value==0 || value==first;
 case 0x11a8:return value>=first && value<base+2*DMA_RX_DESC && (value-first)%DMA_STRIDE==0;
 case 0x11b0:return value==DMA_SLOTS/2-1;
 default:return 0;
 }
}
int dma_allowed(unsigned region,unsigned offset,uint32_t value) {
 if (region!=0) return 0;
 uint32_t base=(uint32_t)dma_physical();
 switch (offset) {
 case 0: return (value&~((rx_checksum_active ? 1u<<27 : 0)|3u|(1u<<13)|(1u<<14)|(1u<<15)|(1u<<20)|(1u<<21)))==0;
 case 0x8:case 0x70:case 0x90:case 0xb4:case 0x1110:case 0x1118:return value==0;
 case 0x1134:return value==0 || value==irq_mask(0);
 case 0x1160:return value!=0 && (value&~irq_mask(0))==0;
 case 0x11b4:return (tx_channels==2 || rx_channels==2) && (value==0 || value==irq_mask(1));
 case 0x11e0:return value!=0 && (value&~irq_mask(1))==0;
 case 0xa0:return value==(rx_channels==2?0xau:2u);
 case 0xa4:return value==(rx_channels==2?0x110000u:0x100000u);
 case 0x300:return value==0x00000100;
 case 0x304:return value==0x53355202;
 case 0xd00:return (value&~0x01ff000au)==0 && (value&0xa)==0xa;
 case 0xd18:return value==16;
 case 0xd30:return (value&~0x3ff00020u)==0 && (value&32);
 case 0x1000:return value==1;
 case 0x1004:return value==axi_mode || value==(axi_mode|AXI_EAME);
 case 0x1100:return value==channel_control;
 case 0x1104:return value==tx_channel || value==(tx_channel|1u);
 case 0x1108:return value==(channel_pbl|(DMA_BUFFER<<1)) || value==(channel_pbl|(DMA_BUFFER<<1)|1u);
 case 0x1114:return value==0 || value==base;
 case 0x111c:return value==0 || value==base+DMA_RX_DESC;
 case 0x112c:return value==DMA_SLOTS-1;
 case 0x1130:return value==rx_slots()-1;
 case 0x1120:return value>=base && value<base+DMA_RX_DESC && (value-base)%DMA_STRIDE==0;
 case 0x1128:return value>=base+DMA_RX_DESC && value<base+DMA_RX_DESC+rx_slots()*DMA_STRIDE && (value-base-DMA_RX_DESC)%DMA_STRIDE==0;
 default:return allow_secondary_tx(offset,value,base) || allow_secondary_rx(offset,value,base);
 }
}
uint32_t dma_irq_begin(void) {
 if (!running || !dma_rx_irq) return 0;
 uint32_t observed=0;
 for (unsigned channel=0;channel<2;channel++) {
  uint32_t mask=irq_mask(channel);
  if (!mask) continue;
  unsigned offset=channel*0x80;
  hw_write(0,0x1134+offset,0);
  uint32_t status=hw_read(0,0x1160+offset)&mask;
  observed|=status;
  if (status) hw_write(0,0x1160+offset,status);
 }
 return observed;
}
void dma_irq_end(void) {
 if (running && dma_rx_irq)
  for (unsigned channel=0;channel<2;channel++) {
   uint32_t mask=irq_mask(channel);
   if (mask) hw_write(0,0x1134+channel*0x80,mask);
  }
}
static int arm_rx(unsigned i) {
 memset((void *)buffer(1,i),0,DMA_BUFFER);
 int defer = rx_batching && i==rx_index;
 if (cache(2,buffer(1,i),DMA_BUFFER)) return -20;
 struct dma_desc *d=desc(1,i);
 rx_external[rx_first()+i]=0;
 d->word[0]=bus(buffer(1,i));d->word[1]=0;d->word[2]=0;desc_own(d,OWN|0x01000000|dma_rx_irq);
 return (!defer && desc_cache(2,d,DMA_STRIDE)) ? -20 : 0;
}
#ifndef R5S_GMAC_PBL
#define R5S_GMAC_PBL 8
#endif
_Static_assert(R5S_GMAC_PBL == 8 || R5S_GMAC_PBL == 16 || R5S_GMAC_PBL == 32, "DMA PBL");
uint32_t dma_hw_features[4], dma_hw_features_valid, dma_pbl, dma_pbl_requested, dma_axi_config;

int dma_start(void) {
 uint64_t physical=dma_physical();uintptr_t virt=dma_virtual();
 if (running || (physical&4095) || (virt&4095) || physical<0x20000000 || physical>0x40000000-DMA_SIZE ||
     (hw_read(0,0)&3) || (hw_read(0,0x1104)&1) || (hw_read(0,0x1108)&1)) return -21;
 for (unsigned slot=0;slot<DMA_SLOTS;slot++)
  if (rx_external[slot] || tx_rings[0].external[slot] || tx_rings[1].external[slot]) return -21;
 unsigned feature=hw_read(0,0x120),rx_bits=feature&31,tx_bits=(feature>>6)&31,width=(feature>>14)&3;
 if (rx_bits<1 || rx_bits>10 || tx_bits<1 || tx_bits>10 || width==3) return -22;
 unsigned revision=hw_read(0,0x110)&0xff;
 dma_hw_features[0]=hw_read(0,0x11c);
 dma_hw_features[1]=feature;
 dma_hw_features[2]=hw_read(0,0x124);
 dma_hw_features_valid=3;
 tx_channels=1;
 selected_tx=0;
#ifdef R5S_GMAC_TWO_TX
 if ((revision==0x42 || (revision>=0x50 && revision<=0x52))
     && tx_bits>=5 && (dma_hw_features[2]&(15u<<18)) && (dma_hw_features[2]&(15u<<6))) {
  if (hw_read(0,0x1184)&1) return -21;
  tx_channels=2;
 }
#endif
 rx_channels=1;
 selected_rx=0;
#ifdef R5S_GMAC_TWO_RX
 if ((revision==0x42 || (revision>=0x50 && revision<=0x52))
     && rx_bits>=5 && (dma_hw_features[2]&(15u<<12)) && (dma_hw_features[2]&15u)) {
  if (hw_read(0,0x1188)&1) return -21;
  rx_channels=2;
 }
#endif
 unsigned rx_fifo=(128u<<rx_bits)/rx_channels, tx_fifo=(128u<<tx_bits)/tx_channels;
 unsigned fifo_bytes=rx_fifo<tx_fifo?rx_fifo:tx_fifo;
 unsigned pbl=revision>=0x40 && revision<=0x52 && fifo_bytes>=16u*R5S_GMAC_PBL
              ? R5S_GMAC_PBL : 8u;
 unsigned pblx8=0;
#ifdef R5S_GMAC_PBLX8
 /* RK3568 EQoS uses a 64-bit AXI bus. Keep the burst within half of either FIFO. */
 pblx8=revision>=0x50 && revision<=0x52 && fifo_bytes>=128u*pbl;
#endif
 channel_pbl=pbl<<16;
 channel_control=(6u<<18)|(pblx8<<16);
 if (revision>=0x50 && revision<=0x52) {
  dma_hw_features[3]=hw_read(0,0x128);
  dma_hw_features_valid=4;
 }
 uint32_t negotiated=hw_read(0,0)&(7u<<13);
 hw_write(0,0x1000,1);
 uint64_t reset_start=hw_us();
 while (hw_read(0,0x1000)&1) {
  if (hw_us()-reset_start>=50000) return -28;
  hw_progress();
 }
 memset((void *)virt,0,DMA_SIZE);
 if (cache(2,virt,DMA_SIZE)) return -20;
 rx_ready_to_take[0]=rx_ready_to_take[1]=0;
 rx_indices[0]=rx_indices[1]=dma_evidence=0;
 memset(tx_rings,0,sizeof tx_rings);
#ifdef R5S_GMAC_HYBRID_IRQ
 dma_rx_irq=(revision==0x42 || (revision>=0x50 && revision<=0x52)) ? RX_IOC : 0;
#endif
 for (selected_rx=0;selected_rx<rx_channels;selected_rx++)
  for (unsigned i=0;i<rx_slots();i++) if (arm_rx(i)) {selected_rx=0;return -20;}
 selected_rx=0;
 hw_write(0,0x300,0x00000100);hw_write(0,0x304,0x53355202);
 hw_write(0,8,0);hw_write(0,0x70,0);hw_write(0,0x90,0);hw_write(0,0xb4,0);hw_write(0,0x1134,0);
 hw_write(0,0xa0,rx_channels==2?0xau:2u);
 hw_write(0,0xa4,rx_channels==2?0x110000u:0x100000u);
 tx_mtl=(((128u<<tx_bits)/(256*tx_channels)-1)<<16)|0xa;
 hw_write(0,0xd00,tx_mtl);
 if (tx_channels==2) {
  hw_write(0,0xd40,tx_mtl);hw_write(0,0xd58,16);
  hw_write(0,0x1180,channel_control);hw_write(0,0x1184,channel_pbl);
  hw_write(0,0x1190,0);hw_write(0,0x1194,(uint32_t)physical+TX1_DESC);
  hw_write(0,0x11ac,TX1_SLOTS-1);hw_write(0,0x11b4,0);
  if ((hw_read(0,0xd40)&0x01ff000a)!=tx_mtl
      || hw_read(0,0x1194)!=(uint32_t)physical+TX1_DESC
      || (hw_read(0,0x11ac)&1023)!=TX1_SLOTS-1) return -22;
 }
 uint32_t rx_mtl=((rx_fifo/256-1)<<20)|32;
 hw_write(0,0xd18,16);hw_write(0,0xd30,rx_mtl);
 if (rx_channels==2) {
  uint32_t rx1_base=(uint32_t)physical+DMA_RX_DESC+(DMA_SLOTS/2)*DMA_STRIDE;
  hw_write(0,0xc30,0x100);hw_write(0,0xd70,rx_mtl);
  hw_write(0,0x1180,channel_control);hw_write(0,0x1188,channel_pbl|(DMA_BUFFER<<1));
  hw_write(0,0x1198,0);hw_write(0,0x119c,rx1_base);
  hw_write(0,0x11b0,rx_slots()-1);hw_write(0,0x11b4,0);
  if ((hw_read(0,0xa0)&15)!=0xa || (hw_read(0,0xa4)&0x170000)!=0x110000
      || (hw_read(0,0xc30)&0xf0f)!=0x100 || hw_read(0,0x119c)!=rx1_base
      || (hw_read(0,0x11b0)&1023)!=rx_slots()-1) return -22;
 }
 axi_mode=AXI_BASELINE;
 dma_axi_config=0;
#ifdef R5S_GMAC_RK3568_AXI
 if (revision==0x42 || (revision>=0x50 && revision<=0x52)) {
  axi_mode=AXI_RK3568;
  hw_write(0,0x1004,axi_mode);
  if ((hw_read(0,0x1004)&AXI_CONFIG_MASK)!=axi_mode) {
   axi_mode=AXI_RK3568 & ~(1u<<14);
   hw_write(0,0x1004,axi_mode);
   if ((hw_read(0,0x1004)&AXI_CONFIG_MASK)!=axi_mode) axi_mode=AXI_BASELINE;
  }
 }
#endif
#ifdef R5S_GMAC_EAME
 if (revision>=0x40 && revision<=0x52) {
  uint32_t with_eame=axi_mode|AXI_EAME;
  hw_write(0,0x1004,with_eame);
  if ((hw_read(0,0x1004)&AXI_CONFIG_MASK)==with_eame) axi_mode=with_eame;
 }
#endif
 if (axi_mode==AXI_BASELINE) hw_write(0,0x1004,axi_mode);
 dma_axi_config=hw_read(0,0x1004)&AXI_CONFIG_MASK;
#ifdef R5S_GMAC_RK3568_AXI
 if (dma_axi_config!=axi_mode) return -22;
#endif
 hw_write(0,0x1100,channel_control);
 if ((hw_read(0,0x1100)&(1u<<16))!=(channel_control&(1u<<16))) {
  /* Safe fallback: readback rejected the ×8 multiplier — reprogram the bus
   * mode without it; dma_pbl below reports what actually took effect. */
  channel_control&=~(1u<<16);
  hw_write(0,0x1100,channel_control);
 }
 dma_pbl_requested=pbl*(pblx8 ? 8u : 1u);
 unsigned tx_osp=0;
#ifdef R5S_GMAC_RK3568_AXI
 if (revision>=0x50 && revision<=0x52) tx_osp=1u<<4;
#endif
 tx_channel=channel_pbl|tx_osp;
 tso_active=0;
#ifdef R5S_WAN_TSO
 if (revision>=0x40 && revision<=0x52 && (feature&(1u<<18))) tx_channel|=TX_CHANNEL_TSE;
#endif
 hw_write(0,0x1104,tx_channel);
 uint32_t tx_readback=hw_read(0,0x1104);
 tx_osp&=tx_readback;
 tx_channel=(tx_channel&~(1u<<4))|tx_osp;
 if (tx_channel&TX_CHANNEL_TSE) {
  tso_active=(tx_readback&TX_CHANNEL_TSE)!=0;
  if (!tso_active) tx_channel=channel_pbl|tx_osp;
 }
 hw_write(0,0x1108,channel_pbl|(DMA_BUFFER<<1));
 dma_pbl=((hw_read(0,0x1108)>>16)&0x7f)*((hw_read(0,0x1100)&(1u<<16)) ? 8u : 1u);
 hw_write(0,0x1110,0);hw_write(0,0x1114,(uint32_t)physical);hw_write(0,0x1118,0);hw_write(0,0x111c,(uint32_t)physical+DMA_RX_DESC);
 hw_write(0,0x112c,DMA_SLOTS-1);hw_write(0,0x1130,rx_slots()-1);
 if ((hw_read(0,0x112c)&1023)!=DMA_SLOTS-1 || (hw_read(0,0x1130)&1023)!=rx_slots()-1) return -22;
#ifdef R5S_WAN_TX_IPV4_CHECKSUM
 dma_tx_ipv4_checksum_active=(dma_hw_features[0]&(1u<<14))!=0;
#endif
#ifdef R5S_WAN_RX_CHECKSUM
 rx_checksum_active=(dma_hw_features[0]&(1u<<16))!=0;
#endif
 running=1;
 if (tx_channels==2) {
  hw_write(0,0x1184,channel_pbl|1);
  if (!(hw_read(0,0x1184)&1)) return -22;
 }
 hw_write(0,0x1104,tx_channel|1);hw_write(0,0x1108,channel_pbl|(DMA_BUFFER<<1)|1);
 if (rx_channels==2) {
  hw_write(0,0x1188,channel_pbl|(DMA_BUFFER<<1)|1);
  if (!(hw_read(0,0x1188)&1)) return -22;
 }
 hw_write(0,0,negotiated|(1u<<20)|(1u<<21)|3|(rx_checksum_active ? 1u<<27 : 0));
 if (!(hw_read(0,0)&(1u<<27))) rx_checksum_active=0;
 for (selected_rx=0;selected_rx<rx_channels;selected_rx++)
  ring_doorbell(0x1128+rx_dma_offset(),desc(1,rx_slots()-1));
 selected_rx=0;
 return 0;
}
int dma_stop(void) {
 if (!running) return 0;
 dma_irq_begin();
 hw_write(0,0x1104,tx_channel);
 if (tx_channels==2) hw_write(0,0x1184,channel_pbl);
 uint64_t start=hw_us();
 while (((hw_read(0,0xd08)>>1)&3)==1 || (hw_read(0,0xd08)&16)
     || (tx_channels==2 && (((hw_read(0,0xd48)>>1)&3)==1 || (hw_read(0,0xd48)&16)))) {if (hw_us()-start>100000) return -23;hw_progress();}
 hw_write(0,0,hw_read(0,0)&~3u);
 start=hw_us();
 while ((hw_read(0,0xd38)&((0x7fffu<<16)|(3u<<4)))
     || (rx_channels==2 && (hw_read(0,0xd78)&((0x7fffu<<16)|(3u<<4))))) {if (hw_us()-start>100000) return -23;hw_progress();}
 hw_write(0,0x1108,channel_pbl|(DMA_BUFFER<<1));
 if (rx_channels==2) hw_write(0,0x1188,channel_pbl|(DMA_BUFFER<<1));
 if ((hw_read(0,0x1104)|hw_read(0,0x1108))&1
     || (tx_channels==2 && (hw_read(0,0x1184)&1))
     || (rx_channels==2 && (hw_read(0,0x1188)&1))) return -23;
 start=hw_us();
 while (hw_read(0,0x100c)&0xff00u) {if (hw_us()-start>100000) return -23;hw_progress();}
 hw_write(0,0x1114,0);
 hw_write(0,0x111c,0);
 if (tx_channels==2) {
  hw_write(0,0x1194,0);
  if (hw_read(0,0x1194)) return -23;
 }
 if (rx_channels==2) {
  hw_write(0,0x119c,0);
  if (hw_read(0,0x119c)) return -23;
 }
 if (hw_read(0,0x1114) || hw_read(0,0x111c)) return -23;
 running=0;tx.pending=0;dma_rx_irq=0;tx.batching=tx.unposted=0;
 rx_batching=rx_unposted=0;rx_desc_arm=0;
 memset(tx.tso_last,0,sizeof tx.tso_last);
 return 0;
}
int dma_prepare_link(unsigned speed,unsigned duplex) {
 static unsigned last_speed,last_duplex;
 if (running && (speed!=last_speed || duplex!=last_duplex)) {int error=dma_stop();if (error) return error;}
 last_speed=speed;last_duplex=duplex;return 0;
}
int dma_take_stopped_refs(unsigned slot,uint64_t *rx,uint64_t *tx_ref) {
 *rx=*tx_ref=0;
 if (running || slot>=DMA_SLOTS+(tx_channels==2?TX1_SLOTS:0)) return 0;
 if (slot>=DMA_SLOTS) {
  *tx_ref=tx_rings[1].external[slot-DMA_SLOTS];
  tx_rings[1].external[slot-DMA_SLOTS]=0;
 } else {
  *rx=rx_external[slot];*tx_ref=tx_rings[0].external[slot];
  rx_external[slot]=tx_rings[0].external[slot]=0;
 }
 return 1;
}
int dma_reclaim_check(void) {
 if (!running) return 0;
 return hw_read(0,0x1160+tx_dma_offset())&((1u<<12)|(1u<<13)) ? -24 : 0;
}
int dma_reclaim_next(uint64_t *external,uint32_t *tso_id) {
 if (external) *external=0;
 if (tso_id) *tso_id=0;
 if (!running) return 0;
 if (tx.pending==tx.unposted) return 0;
 struct dma_desc *d=desc(0,tx.consumer);
 /* Windowed invalidate: one cache op covers up to MAINT_BATCH in-flight
  * descriptor lines (never past tx.pending, so freshly prepared slots whose
  * clean is still pending are not dropped). The window shrinks as the
  * consumer advances and is dropped on a stall or ring wrap. */
 if (!tx.desc_arm) {
  unsigned span=MAINT_BATCH;
  unsigned published=tx.pending-tx.unposted;
  if (span>published) span=published;
  unsigned left=tx_slots()-tx.consumer;
  if (span>left) span=left;
  if (desc_cache(1,desc(0,tx.consumer),span*DMA_STRIDE)) return -20;
  tx.desc_arm=span;
 }
 if (d->word[3]&OWN) {
  tx.desc_arm=0;
  return hw_us()-tx.sent_at[tx.consumer]>2000000?-25:0;
 }
 if (!tx.context[tx.consumer] && (d->word[3]&(0x8000u|CTX))) return -26;
 tx.context[tx.consumer]=0;
 if (external) *external=tx.external[tx.consumer];
 tx.external[tx.consumer]=0;
 if (tso_id) *tso_id=tx.tso_last[tx.consumer];
 tx.tso_last[tx.consumer]=0;
 dma_evidence|=2;if (tx.discover[tx.consumer]) dma_evidence|=16;
 tx.pending--;tx.consumer=(tx.consumer+1)%tx_slots();dma_tx_packets++;dma_tx_retired[selected_tx]++;
 if (tx.desc_arm) tx.desc_arm--;
 if (!tx.consumer) tx.desc_arm=0;
 return 1;
}
int dma_reclaim_full(uint64_t *external,uint32_t *tso_id) {
 int result=dma_reclaim_check();
 if (!result) return dma_reclaim_next(external,tso_id);
 if (external) *external=0;
 if (tso_id) *tso_id=0;
 return result;
}
int dma_reclaim_ex(uint64_t *external) {return dma_reclaim_full(external,0);}
int dma_reclaim(void) {return dma_reclaim_ex(NULL);}
static unsigned tx_checksum(const unsigned char *packet,size_t length) {
#ifdef R5S_WAN_TX_IPV4_CHECKSUM
 if (length >= 34 && packet[12] == 8 && !packet[13] && packet[14] == 0x45
     && !(packet[20] & 0xbf) && !packet[21]) {
  size_t ipv4_length=((size_t)packet[16]<<8)|packet[17];
  if (ipv4_length>=20 && ipv4_length<=length-14 && dma_tx_ipv4_checksum_active) {
   unsigned sum=0;
   for (size_t i=14;i<34;i+=2) sum+=((unsigned)packet[i]<<8)|packet[i+1];
   sum=(sum&65535)+(sum>>16);
   sum=(sum&65535)+(sum>>16);
   if (sum==65535) {
    unsigned protocol=packet[23];
    size_t transport=ipv4_length-20;
    unsigned offset=0;
    if (protocol==6 && transport>=20 && (packet[46]>>4)>=5
        && (size_t)(packet[46]>>4)*4<=transport) offset=50;
    if (protocol==17 && transport>=8 && (((size_t)packet[38]<<8)|packet[39])==transport
        && (packet[40] || packet[41])) offset=40;
    if (offset) {
     sum=protocol+(unsigned)transport;
     for (size_t i=26;i<34;i+=2) sum+=((unsigned)packet[i]<<8)|packet[i+1];
     for (size_t i=0;i<transport;i+=2)
      sum+=((unsigned)packet[34+i]<<8)|(i+1<transport?packet[35+i]:0);
     sum=(sum&65535)+(sum>>16);
     sum=(sum&65535)+(sum>>16);
     if (sum==65535) return 3u<<16;
    }
    return 1u<<16;
   }
  }
 }
#else
 (void)packet;
 (void)length;
#endif
 return 0;
}
/* Producer slots are CPU-owned after reset or successful reclamation. */
static int send_copy(const unsigned char *packet,size_t length,int checksum_validated) {
 if (!running) return 0;
 if (!packet || length<14 || length>1514) return -27;
 if (tx.pending>=tx_slots()-1) return 0;
 struct dma_desc *d=desc(0,tx.producer);
 if (d->word[3]&OWN) return -26;
 unsigned char *data=(unsigned char *)buffer(0,tx.producer);
 memcpy(data,packet,length);
 tx.discover[tx.producer]=discover(packet,length);
 unsigned checksum_control=checksum_validated?0:tx_checksum(packet,length);
 if (checksum_control) {
  data[24]=data[25]=0;
  if (checksum_control==(3u<<16)) {
   unsigned offset=packet[23]==6?50:40;
   data[offset]=data[offset+1]=0;
  }
 }
 if (length<60) {memset(data+length,0,60-length);length=60;}
 if (cache(2,(uintptr_t)data,(length+63)&~(size_t)63)) return -20;
 d->word[0]=bus(buffer(0,tx.producer));d->word[1]=0;d->word[2]=(uint32_t)length|tx_ioc();
 desc_own(d,OWN|FD|LD|checksum_control|(uint32_t)length);
 if (defer_tx_desc_clean(d)) return -20;
 dma_tx_ipv4_checksums+=checksum_control!=0;
 dma_evidence|=1;
 tx.sent_at[tx.producer]=tx_submit_time();tx.producer=(tx.producer+1)%tx_slots();tx.pending++;
 post_tx(1);
 return (int)length;
}
int dma_send(const unsigned char *packet,size_t length) {
 return send_copy(packet,length,0);
}
int dma_send_nat_validated(const unsigned char *packet,size_t length) {
 return send_copy(packet,length,1);
}
int dma_receive_rearm(unsigned char *packet,size_t capacity,uint64_t rearm_physical) {
 dma_rx_checksum_protocol=0;
 rx_ready_to_take[selected_rx]=0;
 if (!running) return 0;
 if (hw_read(0,0x1160+rx_dma_offset())&(1u<<12)) return -24;
 if (rx_desc_ready()) return -20;
 struct dma_desc *d=desc(1,rx_index);
 uint32_t status=rx_status(d);if (status&OWN) {rx_desc_arm=0;return 0;}
 if (rx_external[rx_first()+rx_index]) return 0;
 size_t length=status&0x7fff;int result=-1;
 if ((status&(FD|LD))==(FD|LD) && !(status&(0x8000u|0x40000000u)) && length>=14 && length<=1514 && length<=capacity) {
  if (cache(1,buffer(1,rx_index),(length+63)&~63u)) return -20;
  memcpy(packet,(void *)buffer(1,rx_index),length);
  dma_rx_checksum_protocol=rx_protocol_of(packet,length,status,d->word[1]);
  result=(int)length;dma_rx_packets++;dma_evidence|=4;
 } else {dma_drops++;dma_evidence|=8;}
 unsigned i=rx_index;
 if (rearm_physical) {
  if (dma_arm_ref(i,rearm_physical)) return -20;
 } else if (arm_rx(i)) return -20;
 rx_publish(i);rx_advance();dma_rx_retired[selected_rx]++;
 return result;
}
int dma_receive(unsigned char *packet,size_t capacity) {
 return dma_receive_rearm(packet,capacity,0);
}
/* External-buffer (Normal non-cacheable) API: see dma.h for the contract.
 * No cache maintenance or memset ever touches an external buffer; only
 * descriptor lines are maintained and publication ends at the hw_write tail
 * (dsb sy), with payload ordering the caller's dma_publish DSB. */
int dma_arm_ref(unsigned slot,uint64_t physical) {
 if (!running) return -21;
 if (slot>=rx_slots() || !physical || (physical&63) || physical>0xffffffffull) return -27;
 struct dma_desc *d=desc(1,slot);
 int defer = rx_batching && slot==rx_index;
 if (!(defer && rx_desc_arm) && desc_cache(1,d,DMA_STRIDE)) return -20;
 if (d->word[3]&OWN) return -21;
 rx_external[rx_first()+slot]=(uint32_t)physical;
 d->word[0]=(uint32_t)physical;d->word[1]=0;d->word[2]=0;desc_own(d,OWN|0x01000000|dma_rx_irq);
 return (!defer && desc_cache(2,d,DMA_STRIDE)) ? -20 : 0;
}
#ifdef R5S_ZC_WRITEBACK
extern int dma_ref_invalidate(uint64_t physical);
#endif
int dma_peek(unsigned *slot,uint64_t *physical,size_t *length,uint32_t *rx_protocol,int *external) {
 if (!slot || !physical || !length || !rx_protocol || !external) return -27;
 rx_ready_to_take[selected_rx]=0;
 if (!running) return -21;
 if (hw_read(0,0x1160+rx_dma_offset())&(1u<<12)) return -24;
 if (rx_desc_ready()) return -20;
 struct dma_desc *d=desc(1,rx_index);
 uint32_t status=rx_status(d);
 if (status&OWN) {rx_desc_arm=0;return 0;}
 uint32_t address=rx_external[rx_first()+rx_index];size_t n=status&0x7fff;int is_external=address!=0;
 if (!is_external) address=bus(buffer(1,rx_index));
 *slot=rx_index;*physical=address;*length=n;*rx_protocol=0;*external=is_external;
 if (status&(0x8000u|0x40000000u)) {rx_ready_to_take[selected_rx]=1;return -26;}
 if ((status&(FD|LD))==(FD|LD) && n>=14 && n<=1514) {
  if (is_external) {
   /* Normal non-cacheable: read the header through the CPU mapping of the
    * shared pool (never a physical-identity cast) and copy nothing. */
   uintptr_t va=dma_ref_virtual(address);
   if (!va) return -27;
#ifdef R5S_ZC_WRITEBACK
   if (dma_ref_invalidate(address)) return -20;
#endif
   *rx_protocol=rx_protocol_of((const unsigned char *)va,n,status,d->word[1]);
   rx_ready_to_take[selected_rx]=1;
   return 1;
  }
 }
 rx_ready_to_take[selected_rx]=is_external;
 return is_external ? -26 : 0;
}
int dma_take_ref(unsigned slot,uint64_t physical) {
 if (!running) return -21;
 if (slot!=rx_index || !rx_ready_to_take[selected_rx] || slot>=rx_slots() || (physical && ((physical&63) || physical>0xffffffffull))) return -27;
 rx_ready_to_take[selected_rx]=0;
 struct dma_desc *d=desc(1,slot);
 if (physical) {
  rx_external[rx_first()+slot]=(uint32_t)physical;
  d->word[0]=(uint32_t)physical;d->word[1]=0;d->word[2]=0;desc_own(d,OWN|0x01000000|dma_rx_irq);
  if (!(rx_batching && slot==rx_index) && desc_cache(2,d,DMA_STRIDE)) return -20;
 } else if (arm_rx(slot)) return -20;
 rx_publish(slot);rx_advance();dma_rx_retired[selected_rx]++;
 return 0;
}
int dma_send_ref(uint64_t physical,size_t length,uint32_t checksum) {
 if (!running) return -21;
 if (!physical || (physical&63) || physical>0xffffffffull || length<14 || length>1514) return -27;
 if (checksum!=0 && checksum!=1u<<16 && checksum!=3u<<16) return -27;
 if (tx.pending>=tx_slots()-1) return 0;
 struct dma_desc *d=desc(0,tx.producer);
 if (d->word[3]&OWN) return -26;
 if (length<60) length=60;
 d->word[0]=(uint32_t)physical;d->word[1]=0;d->word[2]=(uint32_t)length|tx_ioc();
 desc_own(d,OWN|FD|LD|checksum|(uint32_t)length);
 if (defer_tx_desc_clean(d)) return -20;
 tx.external[tx.producer]=(uint32_t)physical;
 tx.discover[tx.producer]=0;
 dma_tx_ipv4_checksums+=checksum!=0;
 dma_evidence|=1;
 tx.sent_at[tx.producer]=tx_submit_time();tx.producer=(tx.producer+1)%tx_slots();tx.pending++;
 post_tx(1);
 return (int)length;
}
int dma_tso_available(void) {
#ifdef R5S_WAN_TSO
 return running && !selected_tx && tso_active;
#else
 return 0;
#endif
}
void dma_tso_capability_read(struct dma_tso_capability *out) {
 if (!out) return;
 memset(out,0,sizeof *out);
 if (!dma_tso_available()) return;
 out->supported=1;
 out->ip_version_mask=1u;
#if !defined(R5S_TSO_IPV6) || R5S_TSO_IPV6!=0
 out->ip_version_mask=3u;
#endif
 out->max_payload=DMA_TSO_MAX_PAYLOAD;out->max_mss=DMA_TSO_MAX_MSS;
 out->min_mss=DMA_TSO_MIN_MSS;out->max_header=DMA_TSO_MAX_HDR;
 out->max_frags=DMA_TSO_MAX_FRAGS;
}
/* One TSO ring descriptor: observe, reject device ownership, write the
 * words with OWN set, then clean the used private buffer range (external
 * ranges are never cache-touched, matching dma_send_ref) and the
 * descriptor line. */
static int tso_emit(unsigned slot,uint32_t address,uint32_t length,uint32_t status) {
 struct dma_desc *d=desc(0,slot);
 if (d->word[3]&OWN) return -26;
 d->word[0]=address;d->word[1]=0;d->word[2]=length|((status&LD) && dma_rx_irq ? TX_IOC : 0);
 if (tx_private(address)) {
  uintptr_t va=dma_virtual()+(uintptr_t)(address-(uint32_t)dma_physical());
  uintptr_t first=va&~(uintptr_t)63;
  if (cache(2,first,((va+length+63)&~(uintptr_t)63)-first)) return -20;
 }
 desc_own(d,status);
 return defer_tx_desc_clean(d);
}
int dma_send_tso(const struct dma_tso_request *request) {
 if (!running) return -21;
 if (!request) return -27;
 if (!dma_tso_available()) return -30;
 unsigned n=request->nfrags;
 if ((request->ip_version!=4 && request->ip_version!=6) || request->protocol!=6 ||
     request->ip_hdr_len!=(request->ip_version==4?20:40) ||
     request->tcp_hdr_len<20 || request->tcp_hdr_len>60 || (request->tcp_hdr_len&3) ||
     request->head_len!=14u+request->ip_hdr_len+request->tcp_hdr_len ||
     request->head_len>DMA_TSO_MAX_HDR ||
     (unsigned)request->mss<DMA_TSO_MIN_MSS || (unsigned)request->mss>DMA_TSO_MAX_MSS ||
     request->payload_len<1 ||
     (uint64_t)request->head_len+request->payload_len>DMA_TSO_MAX_PAYLOAD ||
     !n || n>DMA_TSO_MAX_FRAGS || !request->tso_id || request->frags[0].length<request->head_len)
  return -27;
#if defined(R5S_TSO_IPV6) && R5S_TSO_IPV6==0
 if (request->ip_version==6) return -27;
#endif
 uint64_t sum=0;
 for (unsigned i=0;i<n;i++) {
  const struct dma_tso_frag *f=&request->frags[i];
  if (!f->length || (f->physical&63) || f->physical>0xffffffffull) return -27;
  sum+=f->length;
 }
 if (sum!=(uint64_t)request->head_len+request->payload_len) return -27;
 unsigned slots=2; /* context + first */
 uint32_t left=request->frags[0].length-request->head_len;
 if (left) slots+=(left+DMA_TSO_MAX_CHUNK-1)/DMA_TSO_MAX_CHUNK;
 for (unsigned i=1;i<n;i++)
  slots+=(request->frags[i].length+DMA_TSO_MAX_CHUNK-1)/DMA_TSO_MAX_CHUNK;
 if (tx.pending+slots>DMA_SLOTS-1) return 0;
 uint64_t stamp=tx_submit_time();uint32_t id=request->tso_id;unsigned s=tx.producer;int error;
 /* TDES3: CTX|TCMSSV on the context; FD|TSE|(tcp_hdr_len/4)<<19|payload on
  * the first data descriptor; OWN set as each descriptor is written. */
 if ((error=tso_emit(s,0,request->mss,OWN|CTX|TCMSSV))) return error;
 tx.context[s]=1;
 tx.discover[s]=0;tx.tso_last[s]=0;tx.sent_at[s]=stamp;s=(s+1)%DMA_SLOTS;
 if ((error=tso_emit(s,(uint32_t)request->frags[0].physical,request->head_len,
     OWN|FD|TSE|((uint32_t)request->tcp_hdr_len/4<<19)|(request->payload_len&0x3ffff)))) return error;
 tx.discover[s]=0;tx.tso_last[s]=0;tx.sent_at[s]=stamp;s=(s+1)%DMA_SLOTS;
 unsigned fi=0;uint32_t address=(uint32_t)request->frags[0].physical+request->head_len;
 left=request->frags[0].length-request->head_len;
 for (;;) {
  if (!left) {
   if (++fi>=n) break;
   address=(uint32_t)request->frags[fi].physical;left=request->frags[fi].length;continue;
  }
  uint32_t chunk=left>DMA_TSO_MAX_CHUNK?DMA_TSO_MAX_CHUNK:left;
  left-=chunk;
  unsigned last=left==0 && fi+1==n;
  if ((error=tso_emit(s,address,chunk,OWN|(last?LD:0)))) return error;
  tx.discover[s]=0;tx.tso_last[s]=last?id:0;tx.sent_at[s]=stamp;
  address+=chunk;s=(s+1)%DMA_SLOTS;
 }
 tx.producer=s;tx.pending+=slots;dma_evidence|=1;
 post_tx(slots);
 return (int)slots;
}
