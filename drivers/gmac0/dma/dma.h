/* SPDX-License-Identifier: BSD-2-Clause */
#ifndef R5S_DMA_H
#define R5S_DMA_H
#include <stddef.h>
#include <stdint.h>
#ifdef R5S_GMAC_RING_256
#define DMA_SIZE 0x100000u
#define DMA_SLOTS 256u
#else
#define DMA_SIZE 0x40000u
#define DMA_SLOTS 64u
#endif
#define DMA_STRIDE 64u
#define DMA_BUFFER 1536u
#define DMA_RX_DESC (DMA_SLOTS*DMA_STRIDE)
#define DMA_TX_BUFFER (2u*DMA_RX_DESC)
#define DMA_RX_BUFFER (DMA_TX_BUFFER+DMA_SLOTS*DMA_BUFFER)
struct dma_desc {volatile uint32_t word[4];uint32_t padding[12];};
uintptr_t dma_virtual(void);
uint64_t dma_physical(void);
int dma_cache(unsigned operation,uintptr_t address,size_t length);
int dma_start(void);
unsigned dma_tx_channels(void);
unsigned dma_rx_channels(void);
int dma_select_rx(unsigned channel);
int dma_select_tx(unsigned channel);
int dma_stop(void);
int dma_send(const unsigned char *packet,size_t length);
int dma_send_nat_validated(const unsigned char *packet,size_t length);
int dma_receive(unsigned char *packet,size_t capacity);
/* Copy the completed private RX frame exactly like dma_receive, but re-arm
 * the slot at `rearm_physical` (0 = private pool buffer, otherwise a
 * 64-byte-aligned external buffer). This migrates a slot to the shared pool
 * without losing the completed frame. */
int dma_receive_rearm(unsigned char *packet,size_t capacity,uint64_t rearm_physical);
/* Batched RX servicing: between begin and end the descriptor maintenance is
 * windowed/span-cleaned and the RX tail pointer is published once per batch.
 * `dma_end_rx_batch` cleans the re-armed spans then publishes the tail; it
 * returns -20 when a cache operation fails (the caller must fault). With no
 * active batch every receive/re-arm call keeps its per-packet behaviour. */
void dma_begin_rx_batch(void);
int dma_end_rx_batch(void);
int dma_reclaim(void);
/* Zero-copy external buffers (shared Normal non-cacheable pool).
 * Contract: an external buffer is Normal non-cacheable. The driver performs
 * NO cache maintenance and NO memset on external buffers; only descriptor
 * lines are maintained through dma_cache as usual. The caller must have
 * written the buffer contents and issued a DSB (dma_publish ordering) before
 * handing the buffer to dma_arm_ref/dma_send_ref; publication itself ends
 * with the descriptor clean and the ring tail hw_write (which does dsb sy).
 * An external buffer is CPU-accessible through the Rust glue's
 * dma_ref_virtual() translation (never a physical-identity cast); dma_peek
 * verifies the transport protocol by reading the frame header there without
 * invalidating the buffer.
 * External buffers must lie outside the private copy-buffer ranges
 * [bus(buffer(0,0)), +DMA_SLOTS*DMA_BUFFER) and [bus(buffer(1,0)), ...);
 * driver-owned slot metadata survives writeback overwriting word[0]. */
/* Arm RX descriptor slot (0..DMA_SLOTS-1) at an external buffer whose
 * device-visible address is `physical` (64-byte aligned, >= DMA_BUFFER).
 * Fails -21 when the device still owns the slot (nothing is changed). */
int dma_arm_ref(unsigned slot,uint64_t physical);
/* Observe the next completed RX descriptor WITHOUT consuming or re-arming.
 * Returns 1 for a completed, valid external frame (FD|LD, no error bits
 * 0x8000|0x40000000, length 14..1514): *slot the descriptor index,
 * *physical the device-visible buffer address from word[0], *length the
 * frame length, *rx_protocol the verified transport protocol (0/6/17, same
 * decoding as dma_receive) and *external 1. Returns 0 when the descriptor is
 * still owned (no outputs are written), when the completion is a
 * private-buffer slot (outputs written, *external 0, *rx_protocol decoded;
 * dma_receive consumes these) or when a completed descriptor is malformed
 * (outputs written for recovery via dma_take_ref). Returns -26 on error bits
 * (outputs written so the slot can be reclaimed), -20 on cache error,
 * -21 when the driver is not running, -27 on NULL output pointers. Does NOT
 * advance the RX cursor. */
int dma_peek(unsigned *slot,uint64_t *physical,size_t *length,
             uint32_t *rx_protocol,int *external);
/* Consume the completed RX descriptor at `slot` (a slot observed by
 * dma_peek) and re-arm it at `physical`: 0 falls back to the private pool
 * buffer for this slot (legacy memset+clean), any 64-byte-aligned address
 * arms an external buffer (no buffer maintenance). The completed buffer is
 * not touched. Advances the RX cursor past `slot` and updates the RX tail
 * pointer. Returns 0 or negative (-20 cache, -21 running/ownership, -27
 * bad slot or alignment). */
int dma_take_ref(unsigned slot,uint64_t physical);
/* Transmit a frame that already lives in an external buffer at
 * device-visible `physical` (64-byte aligned). `length` is 14..1514; frames
 * shorter than 60 are padded by the device or must already be zero-filled to
 * 60 bytes by the caller. `checksum` requests TX IPv4 checksum insertion as
 * the TDES3 field (0, 1u<<16 or 3u<<16); 0 means the frame's checksums are
 * already correct and must NOT be rewritten. This path never touches the
 * external buffer (no memset, no copy, no cache ops, no checksum zeroing).
 * Returns the padded length, 0 when the ring is full, negative on error
 * (-20 cache, -21 not running, -26 ring corruption, -27 bad input). */
int dma_send_ref(uint64_t physical,size_t length,uint32_t checksum);
/* Reclaim ONE completed TX descriptor (shared TX ring with the copy path).
 * When the reclaimed slot used an external buffer and `external` is
 * non-NULL, its device-visible address (the word[0] record) is stored there;
 * otherwise *external is set to 0. Returns 1 when a slot was reclaimed, 0
 * when the next slot is still owned or the driver is not running, -24 fatal
 * DMA error, -25 completion timeout, -26 writeback error (same as
 * dma_reclaim). */
int dma_reclaim_ex(uint64_t *external);
/* Hardware TCP segmentation offload (docs/tso-design.md, docs/tso-capability.md).
 * A dma_tso_request is one oversized TCP packet: frags[0] holds the
 * Ethernet+IP+TCP headers followed by inline payload, frags[1..] continue
 * the payload; sum(frags[i].length) == head_len + payload_len exactly.
 * Fragments carry the device-visible bus address (32-bit identity, 64-byte
 * aligned) of a private copy-pool buffer or an external Normal-NC buffer;
 * the driver classifies each written descriptor chunk with tx_private() and
 * performs cache maintenance only on private ranges (never on external ones,
 * matching dma_send_ref). */
#define DMA_TSO_MAX_FRAGS 8u
#define DMA_TSO_MAX_PAYLOAD 64000u /* LSO_64K cap on head_len+payload_len */
#define DMA_TSO_MIN_MSS 64u
#define DMA_TSO_MAX_MSS 2047u
#define DMA_TSO_MAX_HDR 1023u /* dwmac TDES2 documented limit */
#define DMA_TSO_MAX_CHUNK 16383u /* TDES2_BUFFER1_SIZE_MASK */
struct dma_tso_frag {uint64_t physical;uint32_t length;};
struct dma_tso_request {
 uint8_t ip_version; /* 4 or 6 */
 uint8_t protocol; /* 6 (TCP) */
 uint8_t ip_hdr_len; /* 20 (v4) or 40 (v6) */
 uint8_t tcp_hdr_len; /* 20..60, multiple of 4 */
 uint16_t mss;
 uint16_t flags; /* bit 0: request TX checksum offload where available */
 uint32_t head_len; /* == 14 + ip_hdr_len + tcp_hdr_len */
 uint32_t payload_len; /* total TCP payload bytes, >= 1 */
 uint32_t tso_id; /* caller-chosen nonzero request id */
 uint8_t nfrags; /* 1..DMA_TSO_MAX_FRAGS */
 uint8_t reserved[3];
 struct dma_tso_frag frags[DMA_TSO_MAX_FRAGS];
};
/* Capability report: limits are zeroed when TSO is not supported.
 * ip_version_mask: bit 0 = IPv4, bit 1 = IPv6. */
struct dma_tso_capability {
 uint32_t supported;
 uint32_t ip_version_mask;
 uint32_t max_payload;
 uint32_t max_mss;
 uint32_t min_mss;
 uint32_t max_header;
 uint32_t max_frags;
};
/* Runtime TSO capability (build gate R5S_WAN_TSO AND MAC_HW_FEATURE1 bit 18
 * probed at dma_start). 1 when hardware TSO requests may be submitted. */
int dma_tso_available(void);
void dma_tso_capability_read(struct dma_tso_capability *out);
/* Submit one TSO request through the hardware path (context descriptor,
 * first data descriptor, payload chunks <= DMA_TSO_MAX_CHUNK). Validates
 * first (docs/tso-design.md §2) and returns -27 on any violation, then
 * checks ring capacity pending+slots <= DMA_SLOTS-1 BEFORE writing any
 * descriptor (returns 0 when the whole request does not fit). Publication
 * ends with one tail write at 0x1120; no other register is touched.
 * Returns the number of ring slots consumed (>0), -20 cache error, -21 not
 * running, -26 ring corruption, -27 bad input, -30 TSO unavailable. */
int dma_send_tso(const struct dma_tso_request *request);
/* Reclaim ONE completed TX descriptor exactly like dma_reclaim_ex and in
 * addition report the owning TSO request: *tso_id is the request's tso_id
 * only on the request's final (LD-carrying) slot, 0 otherwise. Either
 * output pointer may be NULL. */
/* TSO ownership returns through the final request ID. */
int dma_reclaim_full(uint64_t *external,uint32_t *tso_id);
/* Check channel status before a bounded sweep of published TX descriptors. */
int dma_reclaim_check(void);
int dma_reclaim_next(uint64_t *external,uint32_t *tso_id);
/* Implemented by the Rust glue: CPU virtual address of a device-visible
 * buffer address known to this driver (a private-pool offset or a shared
 * zero-copy pool buffer). Returns 0 when the address is not CPU-visible. */
uintptr_t dma_ref_virtual(uint64_t physical);
int dma_enabled(void);
void dma_begin_tx_batch(void);
int dma_end_tx_batch(void);
uint32_t dma_irq_begin(void);
void dma_irq_end(void);
int dma_take_stopped_refs(unsigned slot,uint64_t *rx,uint64_t *tx);
int dma_tx_available(void);
unsigned dma_tx_space(void);
unsigned dma_tx_pending(void);
unsigned dma_tx_capacity(void);
int dma_prepare_link(unsigned speed,unsigned duplex);
int dma_allowed(unsigned region,unsigned offset,uint32_t value);
extern uint64_t dma_rx_packets,dma_tx_packets,dma_drops;
#endif
