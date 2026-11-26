/* SPDX-License-Identifier: BSD-2-Clause */
#ifndef R5S_GMAC_RING_H
#define R5S_GMAC_RING_H
#include <stddef.h>
#include <stdint.h>
#define GMAC_SLOTS 16
#define GMAC_BUFFER 1536
#define GMAC_OWN UINT32_C(0x80000000)
struct gmac_desc {volatile uint32_t word[4];};
struct gmac_ring {
    struct gmac_desc *descriptors;
    unsigned char *buffers;
    uint32_t buffer_bus;
    unsigned producer,consumer,pending;
    uint64_t drops,errors;
    void (*release_barrier)(void);
    void (*acquire_barrier)(void);
};
int gmac_ring_init(struct gmac_ring *,struct gmac_desc *,unsigned char *,uint64_t bus,
                   void (*release)(void),void (*acquire)(void));
int gmac_tx_submit(struct gmac_ring *,const unsigned char *,size_t);
int gmac_tx_reclaim(struct gmac_ring *);
void gmac_rx_prepare(struct gmac_ring *);
int gmac_rx_copy(struct gmac_ring *,unsigned char *,size_t,int fcs_in_length);
#endif
