/* SPDX-License-Identifier: BSD-2-Clause */
#include "ring.h"
#include <string.h>
#define FIRST UINT32_C(0x20000000)
#define LAST UINT32_C(0x10000000)
#define CONTEXT UINT32_C(0x40000000)
#define ERROR UINT32_C(0x8000)
_Static_assert(sizeof(struct gmac_desc)==16,"DWMAC normal descriptor size");
int gmac_ring_init(struct gmac_ring *ring,struct gmac_desc *desc,unsigned char *buffers,uint64_t bus,
                   void (*release)(void),void (*acquire)(void)) {
    if (!ring || !desc || !buffers || !release || !acquire || (uintptr_t)desc%64 ||
        (uintptr_t)buffers%64 || bus%64 || bus>UINT32_MAX-(GMAC_SLOTS*GMAC_BUFFER-1)) return -1;
    *ring=(struct gmac_ring){.descriptors=desc,.buffers=buffers,.buffer_bus=(uint32_t)bus,
                            .release_barrier=release,.acquire_barrier=acquire};
    memset(desc,0,GMAC_SLOTS*sizeof(*desc));
    memset(buffers,0,GMAC_SLOTS*GMAC_BUFFER);
    return 0;
}
int gmac_tx_submit(struct gmac_ring *ring,const unsigned char *packet,size_t length) {
    if (!packet || length<14 || length>1514) return -2;
    if (ring->pending==GMAC_SLOTS) return -1;
    unsigned index=ring->producer;
    struct gmac_desc *desc=&ring->descriptors[index];
    if (desc->word[3]&GMAC_OWN) {ring->errors++;return -2;}
    ring->acquire_barrier();
    unsigned char *buffer=ring->buffers+index*GMAC_BUFFER;
    memset(buffer,0,GMAC_BUFFER);
    memcpy(buffer,packet,length);
    if (length<60) length=60;
    desc->word[0]=ring->buffer_bus+index*GMAC_BUFFER;
    desc->word[1]=0;desc->word[2]=(uint32_t)length;
    ring->release_barrier();
    desc->word[3]=GMAC_OWN|FIRST|LAST|(uint32_t)length;
    ring->release_barrier();
    ring->producer=(index+1)%GMAC_SLOTS;ring->pending++;
    return (int)length;
}
int gmac_tx_reclaim(struct gmac_ring *ring) {
    if (!ring->pending) return 0;
    uint32_t status=ring->descriptors[ring->consumer].word[3];
    if (status&GMAC_OWN) return 0;
    ring->acquire_barrier();
    ring->consumer=(ring->consumer+1)%GMAC_SLOTS;ring->pending--;
    if (status&(ERROR|CONTEXT)) {ring->errors++;return -2;}
    return 1;
}
static void rx_release(struct gmac_ring *ring,unsigned index) {
    struct gmac_desc *desc=&ring->descriptors[index];
    memset(ring->buffers+index*GMAC_BUFFER,0,GMAC_BUFFER);
    desc->word[0]=ring->buffer_bus+index*GMAC_BUFFER;
    desc->word[1]=0;desc->word[2]=0;
    ring->release_barrier();
    desc->word[3]=GMAC_OWN|UINT32_C(0x01000000);
    ring->release_barrier();
}
void gmac_rx_prepare(struct gmac_ring *ring) {
    for (unsigned i=0;i<GMAC_SLOTS;i++) rx_release(ring,i);
}
int gmac_rx_copy(struct gmac_ring *ring,unsigned char *output,size_t capacity,int fcs_in_length) {
    unsigned index=ring->consumer;
    uint32_t status=ring->descriptors[index].word[3];
    if (status&GMAC_OWN) return 0;
    ring->acquire_barrier();
    size_t length=status&0x7fff;
    int result=-2;
    if (length>=((fcs_in_length)?18u:14u) && length<=((fcs_in_length)?1518u:1514u) &&
        (status&(FIRST|LAST))==(FIRST|LAST) && !(status&(ERROR|CONTEXT))) {
        if (fcs_in_length) length-=4;
        if (output && length<=capacity) {memcpy(output,ring->buffers+index*GMAC_BUFFER,length);result=(int)length;}
    }
    if (result<0) ring->drops++;
    rx_release(ring,index);
    ring->consumer=(index+1)%GMAC_SLOTS;
    return result;
}
