# TSO transmit interface — implementation contract

Driver-level TCP Segmentation Offload for GMAC0 (C driver) and RTL8125
(Rust driver). Hardware facts and limits: `docs/tso-capability.md`. This
file is the binding contract between the three implementation surfaces:

- `drivers/gmac0/dma/dma.{h,c}` — GMAC0 descriptors + `dma_send_tso`
- `drivers/rtl8125/{ring,dma}.rs` — RTL8125 descriptors + `Ring::send_tso`
- `pds/gmac_dma_corrected/src/tso.rs` — shared validation, software
  segmentation fallback, TSO request queue (used by both PDs)

Ordinary frames keep the existing TX paths byte-identical. A TSO request is
an *oversized TCP packet representation* (router-originated traffic or a
test generator). NAT-forwarded packets are never reassembled into TSO
requests.

## 1. Request shape

```c
#define DMA_TSO_MAX_FRAGS    8u
#define DMA_TSO_MAX_PAYLOAD  64000u   /* LSO_64K: max head_len+payload_len */
#define DMA_TSO_MIN_MSS      64u      /* dwmac ETH_DMACxCR recommendation */
#define DMA_TSO_MAX_MSS      2047u    /* RTL8125 MSS_MAX */
#define DMA_TSO_MAX_HDR      1023u    /* dwmac TDES2 documented limit */
#define DMA_TSO_MAX_CHUNK    16383u   /* dwmac TDES2_BUFFER1_SIZE_MASK */

struct dma_tso_frag { uint64_t physical; uint32_t length; };

struct dma_tso_request {
    uint8_t  ip_version;    /* 4 or 6 */
    uint8_t  protocol;      /* 6 (TCP) */
    uint8_t  ip_hdr_len;    /* 20 (v4) or 40 (v6) */
    uint8_t  tcp_hdr_len;   /* 20..60, multiple of 4 */
    uint16_t mss;           /* DMA_TSO_MIN_MSS..DMA_TSO_MAX_MSS */
    uint16_t flags;         /* bit 0: request TX checksum offload where
                               available (hw TSO always inserts checksums) */
    uint32_t head_len;      /* == 14 + ip_hdr_len + tcp_hdr_len */
    uint32_t payload_len;   /* total TCP payload bytes, >= 1 */
    uint32_t tso_id;        /* caller-chosen nonzero request id */
    uint8_t  nfrags;        /* 1..DMA_TSO_MAX_FRAGS */
    uint8_t  reserved[3];
    struct dma_tso_frag frags[DMA_TSO_MAX_FRAGS];
};
```

Rust mirrors this exactly in `tso.rs` (`TsoRequest` + `TsoFrag`, same field
widths/order; `physical` is the device-visible bus address of the fragment).

Semantics:

- `frags[0]` starts with the Ethernet+IP+TCP headers; `frags[0].length >=
  head_len`; payload bytes after `head_len` in `frags[0]` are inline payload.
- `frags[i>0]` are pure payload continuation, in order.
- `sum(frags[i].length) == head_len + payload_len` exactly.
- `payload_len >= mss` is recommended; smaller payloads are accepted and
  produce a single segment on the fallback path, but hardware TSO is only
  issued when `payload_len > mss` (otherwise the request is sent as one
  ordinary frame through the normal path — it cannot exceed 1514 when
  `head_len + payload_len <= 1514`; if it is oversized yet single-segment it
  goes through fallback segmentation which emits one valid segment).
- A fragment buffer must be device-reachable and 64-byte aligned. Private
  (copy) pool buffers and external Normal-NC zero-copy pool buffers are both
  legal; the GMAC driver classifies by address exactly as
  `tx_private()` does today (cache maintenance only for private buffers,
  never for external ones). RTL8125 fragments are always external
  (2048-aligned, outside the private arena) or private single-slot buffers
  used only by tests.

## 2. Validation rules (shared, identical in all three surfaces)

Reject (`TsoError::Invalid…` / negative errno) when any of:

1. `ip_version ∉ {4, 6}`; `protocol != 6`;
   `ip_hdr_len != (ip_version == 4 ? 20 : 40)`;
   `tcp_hdr_len < 20 || tcp_hdr_len > 60 || tcp_hdr_len % 4 != 0`.
2. `head_len != 14 + ip_hdr_len + tcp_hdr_len`.
3. `mss < DMA_TSO_MIN_MSS || mss > DMA_TSO_MAX_MSS` (both NICs; GMAC also
   requires `head_len + mss < 16383`, satisfied by `mss <= 2047`).
   Additionally `head_len + mss <= 1514`: every emitted segment must fit
   the 1500-byte MTU plus the Ethernet header (IPv6 with a 20-byte TCP
   header therefore caps MSS at 1440).
4. `head_len > DMA_TSO_MAX_HDR`.
5. `payload_len < 1`; `head_len + payload_len > DMA_TSO_MAX_PAYLOAD`.
6. `nfrags == 0 || nfrags > DMA_TSO_MAX_FRAGS`.
7. `sum(frags[i].length) != head_len + payload_len` or
   `frags[0].length < head_len` or any `frags[i].length == 0`.
8. any fragment `physical` not 64-byte aligned or not representable in the
   device address width (GMAC: 32-bit identity; RTL8125: inside the inbound
   window).
9. `tso_id == 0` is invalid for `dma_send_tso` (0 marks "no request" in
   completion reporting).
10. IPv6 is accepted only where the device advertises TSOv6 (both do) —
    tests cover IPv6 as "explicitly supported"; a build that defines
    `R5S_TSO_IPV6=0` rejects IPv6 with `TsoError::Unsupported`.

Valid requests are **never silently dropped**: invalid → explicit error;
unsupported/unavailable → software segmentation fallback; fallback failure →
explicit error.

## 3. GMAC0 hardware path (`dma_send_tso`)

Descriptor sequence (ring slots consumed in order; return value = number of
slots consumed):

1. **Context descriptor** (always, one slot):
   `word[0]=0, word[1]=0, word[2]=mss,
   word[3]=CTX(0x40000000)|TCMSSV(0x04000000)`, OWN (0x80000000) added with
   the final publish of the request.
2. **First data descriptor** (headers; covers exactly `head_len` bytes of
   `frags[0]`):
   `word[0]=bus(frags[0].physical), word[1]=0, word[2]=head_len,
   word[3]=OWN|FD(0x20000000)|TSE(1<<18)|((tcp_hdr_len/4)<<19)
   |((payload_len & 0x3ffff))`.
3. **Payload descriptors**: the remainder of `frags[0]` (inline payload at
   `+head_len`), then each `frags[i>0]`, each split into chunks of at most
   16383 bytes (stmmac `stmmac_tso_allocator`): `word[0]=chunk address,
   word[2]=chunk length, word[3]=OWN` and `LD (0x10000000)` on the
   descriptor holding the final byte.

Publish order: all descriptor words written and cache-cleaned (private
buffers only — external buffers are never cache-touched, matching
`dma_send_ref`), then every `word[3]` OWN-bearing descriptor is cleaned, and
the request is visible at the single tail write (`0x1120`) advanced by the
slot count. `sent_at[slot]` is stamped per slot for the existing completion
timeout. Ring capacity: `pending + slots <= DMA_SLOTS-1` is checked **before**
any write; otherwise return 0 (ring full, caller retries or falls back).

Completion: `dma_reclaim_full(uint64_t *external, uint32_t *tso_id)` walks
the same consumer cursor as `dma_reclaim_ex`. For each retired slot it
returns 1 with `*external` = the slot's recorded buffer address (0 for
private). `*tso_id` is nonzero **only for the final slot** of a TSO request
(the slot carrying LD), carrying that request's `tso_id`. All buffers of a
TSO request remain owned until that report; the Rust glue releases them
then (single `zc::complete` per buffer regardless of how many descriptors
touched it). `dma_reclaim_ex(external)` stays behaviorally unchanged
(it becomes `dma_reclaim_full(external, NULL)`).

## 4. RTL8125 hardware path (`Ring::send_tso`)

One descriptor per fragment (no chunk splitting; fragment length ≤ 65535,
validated). Descriptor i of nfrags:

- `opts1 = OWN | FIRST?(i==0) | LAST?(i==nfrags-1) | RingEnd?(slot==N-1)
  | GiantSendv4(1<<26)|GiantSendv6(1<<25) | (l4_offset<<18) | frag_len`
  where `l4_offset = 14 + ip_hdr_len` (must be ≤ 0x70 and even — the
  shared validator enforces this via the header rules),
- `opts2 = (mss & 0x7ff) << 18`,
- address = fragment bus address, reserved words zero.

Reservation: all `nfrags` slots are checked free first
(`tx_inflight() + nfrags <= N-1` in no-close mode; OWN-cleared in classic
mode) and published consecutively from the TX cursor with a single
`kick_tx_count(nfrags)` doorbell (tail advances by nfrags). A failure
before the first publish leaves the ring untouched; the single-producer
driver PD guarantees no interleaved submitter.

Completion: count-based `reclaim_tx` as today. The ring records
`tso_last[slot] = tso_id` on the LAST descriptor of each request; when the
close pointer retires that slot the request is complete and the caller may
release its buffers (`Ring::take_completed_tso() -> Option<u32>` returns
retired request ids in completion order). Ordinary frames are unaffected.

## 5. Software fallback (shared `tso.rs`)

Used when: hardware TSO unavailable (capability probe failed / build flag
off), the request fails hardware-specific limits (e.g. `head_len > 1023`),
or hardware submission returns "not available". Never used to paper over
malformed requests — validation errors stay errors.

`segment(request, &mut emit) -> Result<u32, TsoError>` produces
MTU-respecting TCP segments through `emit(frame: &[u8])`:

- segment k payload = payload bytes `[k*mss, min((k+1)*mss, payload_len))`;
- frame = Ethernet(14, copied) + IP(v4: copied with new total length /
  `id = base_id + k` / checksum recomputed; v6: new payload length) +
  TCP (copied, `seq = base_seq + k*mss`, PSH only on the final segment,
  checksum recomputed over the pseudo-header + segment) + segment payload;
- checksums are computed in software to completion (requirement: "correct
  software TCP segmentation"); frames are then sent through the ordinary TX
  path with checksum offload disabled (checksum field already correct).
- returns the number of segments emitted. `emit` errors propagate.

The emitter feeds the normal copy/zero-copy TX path of whichever PD is
falling back; oversized frames are never handed to the driver.

## 6. Bounded TSO submission queue (multicore)

`tso.rs` also provides `TsoQueue`: a bounded SPSC ring of compact tickets
(≤ 64 entries, 32-byte stride) referencing zero-copy pool buffer ids
(`id`, `length`) instead of raw addresses, plus the request metadata.
Forwarding workers (or the test generator) push tickets
(`push() -> Result<bool, _>`, `false` = backpressure); the NIC driver PD is
the sole consumer (`pop()`), translates ids via the pool's device address
map, and submits through `dma_send_tso`/`Ring::send_tso`. DMA descriptor
rings remain owned by the driver PD; tickets are the only cross-core
object. ARM release/acquire ordering mirrors `RefQueue`.

## 7. Diagnostics counters (per NIC)

`tso_requests`, `tso_hw_frames`, `tso_hw_segments_est` (ceil(payload/mss)),
`tso_fallback_frames`, `tso_fallback_segments`, `tso_invalid`, `tso_desc_errors`,
`tso_completion_failures`, `tso_queue_drops`, `tso_active` (runtime
capability ∧ build flag), `tso_capability`. Exposed through the B51 UDP
diagnostics page for LAN1/LAN2 and the GMAC status page for WAN.

## 8. Build configuration

`R5S_WAN_TSO=1`, `R5S_LAN1_TSO=1`, `R5S_LAN2_TSO=1` (independent) at image
build time; recorded in `manifest.json` as `wan_tso`/`lan1_tso`/`lan2_tso`
and verified by `scripts/image.py`. Unsupported hardware auto-disables at
runtime and reports capability in diagnostics. `R5S_TSO_IPV6=0` disables
IPv6 TSO acceptance (default: enabled on both NICs).

## 9. Host test matrix

- GMAC TSO descriptor encoding (context + first + payload chunks, OWN/LD
  placement, TSE/HDRL/PAYLOAD fields) — C tests against the mocked hw.
- RTL8125 TSO descriptor encoding (GiantSend bits, GTTCPHO, MSS, FIRST/LAST/
  RingEnd, opts2) — Rust tests against `mac::tests::Model`.
- MSS / header-offset / length-sum validation (both drivers + tso.rs).
- IPv4 TCP segmentation and checksum correctness of the fallback (vectors
  verified against full 16-bit ones-complement recomputation).
- IPv6 TSO covered when `R5S_TSO_IPV6` enabled (explicitly supported).
- Scatter-gather / buffer lifetime: no buffer released before its request's
  final descriptor completes (GMAC `dma_reclaim_full` reports).
- Descriptor wraparound and ring saturation (RTL8125 63-outstanding;
  GMAC `DMA_SLOTS-1` cap; TSO spanning the wrap point).
- Partial TX completion and DMA reclamation (close pointer retires a prefix
  of a TSO request → no buffer release until the last slot).
- Invalid oversized requests → explicit errors; fallback never emits > 1514.
- Link-down / reset / shutdown during TSO (ring stop with in-flight TSO
  slots; `dma_stop`/`clear_after_stop` behavior).
- Normal NAT forwarding unchanged with TSO enabled but unused.
