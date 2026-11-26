# Zero-copy DMA-to-DMA forwarding

Status: **source/configuration qualified, host-tested, hardware-untested.** No
physical zero-copy operation, DMA reachability or throughput improvement is
claimed until the same image passes staged acceptance on an R5S board. This
document describes the implemented design and its precise limits.

## Scope

The multicore NAT firmware forwards frames between GMAC0 (WAN) and the two
RTL8125 B51 controllers (LAN1/LAN2) by passing 16-byte buffer references
instead of copying payloads. Payload bytes live only in a shared, fixed-size
DMA pool; the four NAT workers rewrite headers in place in that pool. The
copy-based path is preserved verbatim and is used whenever the zero-copy path
is unavailable. Existing profiles without `R5S_ROUTER_SCHEDULE=multicore` are
byte-identical to their previous outputs.

## DMA mapping

Never assume a CPU physical address equals a device DMA address.

| Device | Addressing | Shared pool buffer (PA 0x24400000 + id*2048) |
|---|---|---|
| GMAC0 (on-chip EQoS) | 32-bit low physical identity, high words zeroed | `0x24400000 + id*2048` |
| RTL8125 LAN1 | PCIe inbound iATU region 0: PA 0x24200000 → bus 0x80000000, size widened to 0x300000 | bus `0x82000000 + id*2048` |
| RTL8125 LAN2 | PCIe inbound iATU region 0: PA 0x24300000 → bus 0x81000000, size widened to 0x200000 | bus `0x81100000 + id*2048` |

For 256 or 512 descriptors with `R5S_ZC_CACHE=writeback`, the shared pool starts at PA `0x24600000`. LAN1 maps 5 MiB starting at PA `0x24200000` to bus `0x80000000`, so its shared address starts at `0x80400000`. LAN2 maps 5 MiB starting at PA `0x24600000` to bus `0x81000000`, so its shared address starts at `0x81000000` and its private ring starts at `0x81200000`. The aligned kernel direct-map spans keep both private arenas non-cacheable and the shared payload write-back. Board tests must confirm reachability and coherency.

The GMAC identity contract follows the locked Rockchip EQoS programming
(`drivers/gmac0/dma/dma.c::bus`); it is a computed physical address, never a
pointer cast. The RTL8125 windows are programmed with field readback and
bounded enable polling (`drivers/rtl8125/pcie.rs::map_inbound`), checked
4 KiB-aligned and wholly below 4 GiB, and translated by
`InboundWindow::translate` (`drivers/rtl8125/dma.rs`) which enforces bounds and
overflow. `zc::translate` (`pds/gmac_dma_corrected/src/zc.rs`) applies the same
rule per device and counts mapping failures. Board reachability of the widened
windows remains a hardware gate: the captured root has no `dma-ranges`,
`iommus` or `dma-coherent` property, which is not proof of identity
translation, coherency or confinement.

Cache coherency: the whole block [0x24200000, 0x24600000) is reserved RAM
mapped Normal non-cacheable in every participating protection domain
(`cached="false"`). The kernel patch `patches/router/sel4-rtl8125-normal-nc.patch`
extends the NC direct-map window from 2 MiB to 4 MiB (both 2 MiB blocks at
0x24200000 and 0x24400000) and keeps rejecting cached, device or large-page
mappings in the range, so no cached alias of the pool can exist. Both MiB of
the second block are reserved (`zc_pool` + `zc_padding`) so no kernel object
can be allocated inside NC memory. Because all CPU access is uncached, payload
transfers need no cache maintenance — only the publish/consume barriers
(`zc::barrier::dma_publish` / `dma_consume`, DSB SY on AArch64) that order CPU
writes before descriptor/queue publication and completion reads after device
writes. GMAC0 descriptor lines stay in the private cached pool and keep the
existing cache maintenance; RTL8125 descriptors stay in their Normal-NC pool.
ARM_SMMU is disabled; a malfunctioning or hostile PCIe endpoint could still
read or corrupt the pool — seL4 mappings limit CPU access only.

## Shared packet pool

`zc_pool` at PA 0x24400000 (VA 0x66000000) holds generation-tagged 2048-byte
buffers. 2048 is the RTL8125 buffer size and is 2048-aligned, so every buffer
lies inside one page. The default (64/128-slot ring) layout holds **1024
buffers (2 MiB)** and absorbs the former `zc_padding` MiB; both iATU inbound
windows are widened to cover it (LAN1 4 MiB from PA 0x24200000, LAN2 3 MiB
from PA 0x24300000). The 256/512-slot ring layouts have no room after their
1 MiB pool and keep **512 buffers** plus a reserved padding MiB. Free-list
quotas scale with the pool (default: WAN 320, LAN1 352, LAN2 352). `zc_meta`
(VA 0x66800000) holds the ownership words, three free lists, counters, the
download-fallback return-token tallies and the one-time initialiser state.
`zc_links` (VA 0x66900000) holds eight bounded SPSC reference queues (64
entries of 16 bytes each):

```
Q_GMAC_RX  gmac driver  -> wan app     Q_APP_FWD  wan app      -> logical_lan
Q_APP_TX   wan app      -> gmac driver Q_WAN_EG   logical_lan  -> wan app
Q_LAN1_RX  lan1 driver  -> logical_lan Q_LAN1_TX  logical_lan  -> lan1 driver
Q_LAN2_RX  lan2 driver  -> logical_lan Q_LAN2_TX  logical_lan  -> lan2 driver
```

Worker request/result reference queues live beside the frame queues in the
existing per-worker regions. Each queue is single-producer single-consumer with
wrapping `u32` cursors, release/acquire publication and a full-queue `Ok(false)`
backpressure answer; a full queue drops the reference and frees the buffer,
exactly like the copy path's drop counters. Batched notifications are unchanged
(one Microkit notification per worker activation, up to 8 frames per pass).

Protection-domain isolation is preserved: the pool is mapped writable only in
the DMA drivers and the forwarding workers, read-only in the WAN application
(it classifies and relays references without ever writing payloads), and the
metadata is mapped writable only where ownership transitions happen (drivers,
logical LAN, workers).

## Ownership model

```
FREE -> RX_DMA -> CPU -> TX_DMA -> FREE
```

Each buffer has one ownership word `(generation << 8) | state` in `zc_meta`.
Every transition is a compare-and-swap from the expected `(state, generation)`;
a mismatching transition is refused and counted (`ownership_errors`), never
forced. Generations advance on allocation, so a reference to a previous life of
the buffer can never release the new one. The four transitions and their owners:

| Transition | Owner | Meaning |
|---|---|---|
| `FREE -> RX_DMA` (`alloc`) | RX driver | an RX descriptor is armed at the buffer |
| `RX_DMA -> CPU` (`publish`) | RX driver | the completion was verified and a reference is queued |
| `RX_DMA -> FREE` (`recycle`) | RX driver | a completion is discarded without forwarding |
| `CPU -> TX_DMA` (`submit`) | TX driver | a TX descriptor is armed at the buffer |
| `TX_DMA -> FREE` (`complete`) | TX driver | the TX completion was verified |
| `CPU -> FREE` (`reject`) | any CPU owner | drop instead of transmit (policy, TTL ICMP rewrite path, backpressure) |

Buffers are reclaimed only after verified hardware TX completion (descriptor
OWN cleared and error bits clear, or the RTL8125 TX-close watermark), matching
the existing copy-path completion checks and two-second GMAC deadline.
`zc::quiesce` force-frees every buffer only after a verified DMA stop; a failed
quiescence keeps the existing terminal-fault behaviour rather than reusing a
possibly owned pool. Free lists are partitioned per RX driver (WAN 160, LAN1
176, LAN2 176 buffers) so one port cannot exhaust another; a buffer keeps its
partition after forwarding and any driver may release it.

## Forwarding path and copy elimination

LAN/WAN ingress classification, bridge learning, DHCP, ARP, status replies and
ICMP error construction keep their current owners. The zero-copy fast path is:

1. RX driver arms pool buffers; on completion it publishes the reference
   (with the RX-verified transport protocol in `Ref.meta`, the existing
   checksum-offload provenance) instead of copying the frame.
2. The WAN application relays forwardable unicast IPv4 by reference; ARP, DHCP
   and B51 status requests are copied to the local stack (unchanged behaviour).
3. `logical_lan` dispatches references to the four flow-affine NAT workers.
   The worker rewrites IPv4/TCP/UDP/ICMP headers **in place** in the pool
   buffer using the existing incremental checksum updates and passes the
   checksum-offload metadata unchanged to `forward_with_rx_checksum`.
4. The dispatcher finishes neighbour resolution and L2 rewriting in place and
   hands the reference to the egress driver.
5. The TX driver submits the same buffer to its ring; after verified completion
   the buffer returns to `FREE`.

Per forwarded frame this removes every payload copy of the previous path
(RX DMA buffer -> CPU queue -> worker queue -> TX queue -> TX DMA buffer), so a
forwarded frame is written once by the ingress DMA and read once by the egress
DMA. The 16-byte references are the only queue traffic.

Fallback (copied) path — used and counted whenever zero-copy is unavailable:

* the shared pool has no free buffer (`fallback.pool_exhausted`),
* DMA translation fails (`fallback.mapping`),
* the frame has more than one consumer: broadcast/multicast flooding and
  broadcast delivery to the local stack (`fallback.layout`),
* a reference queue is full (`fallback.queue_full`),
* local delivery (DHCP, ARP, ICMP to the router, B51 status) and the ARP
  pending buffer, which must hold a stable frame across polls,
* any profile built without the `zc` feature.

`copied_packets` / `copied_bytes` count exactly the frames and payload bytes
that take those copies. The copy path is the complete previous implementation,
including inline NAT for copy-path frames when workers are not used.

## Ordered return from the download fallback

With direct worker LAN egress (`R5S_WORKER_LAN_EGRESS=1`) and two B51 TX
queues, a worker's direct lane is TX queue 1 while the dispatcher fallback
rides TX queue 0 — independent hardware rings that can interleave on the wire.
To let a client return to the direct lane without a newer frame overtaking an
older fallback frame, every fallback send of a tracked TCP/UDP download client
arms a **return token** (`zc::FLAG_LAN_TOKEN`, stamp `worker << 8 | client` in
`Ref.meta`) and the client is moved back to the direct lane only when its armed
tally equals its retired tally:

* the worker arms a token on each fallback send (including re-arms while
  waiting), and flips the client to the direct lane only when
  `zc::lan_token_settled` holds and the route resolves to an open lane;
* a single-port handoff transfers the token to the egress reference or copy
  marker and the NIC driver retires it exactly once — on verified TX
  completion or a confirmed software discard (including the drained DMA stop);
* a flood extends the armed tally by the copy count and each copy retires one;
* a dispatch-time drop or WAN egress retires the token at the decision.

Traffic for other clients is never gated. The direct-to-fallback direction
still drains the old lane before switching (`worker_lan_tx_drained`), and
during that drain the client's frames are dropped, as before. Scope: the
token tracks TCP/UDP unicast download frames that pass `lan_egress`; ICMP and
non-client frames take the ordinary result path without tokens. A driver
fault leaves the affected clients on fallback (never worse than the previous
behaviour of keeping them there permanently).

Source learning is coalesced across workers: a worker suppresses identical
`(MAC, LAN, revision)` learn records inside a 250 ms window
(`zc::LearnCoalescer`), and the dispatcher applies consecutive learn records
in one batch, one `learn_source` per MAC (`zc::LearnBatch`), flushing before
any frame result so apply order is unchanged.

## Diagnostics

B51 UDP status schema 6 (268 words; schema 5 images keep 248 words and are
byte-compatible) appends: zero-copy packets received/sent, buffers completed,
packets forwarded/dropped, copied packets and bytes, queue drops, mapping
failures, ownership errors, RX recycled, allocation failures, per-reason
fallback counters, peak occupancy and current pool in-use/free. The host
reader probes both schema sizes; `b51_status.py` exposes the `zc` object and
keeps every existing field.

## Validation performed (host only)

* `rustc --test pds/gmac_dma_corrected/src/zc.rs`: ownership transitions,
  double-free/stale references, partition quotas and reuse, reference-queue
  wraparound and backpressure, DMA window translation and 32-bit identity
  bounds, handle packing, the barrier/publish ordering model with a concurrent
  device-model thread, three concurrent allocator/completer owners, quiesce
  after stop, and the one-time initialiser. 15 tests.
* `rustc --test drivers/rtl8125/lib.rs`: 36 tests including the external
  arming, mixed cursor discipline between copy and zero-copy receive, invalid
  completion re-arm, in-order no-close external TX, 64-bit descriptor address
  readback and window translation bounds.
* `cc -fsanitize=undefined,address tests/gmac_dma_test.c drivers/gmac0/dma/dma.c`:
  the existing 6300 TX / 2000 RX protocol tests plus external arming,
  peek/take cursor discipline, external TX with untouched payload and
  reclaim reporting, validation and timeout/error cases.
* `python3 -m unittest tests.test_zc_forwarding`: schema 6 JSON, dual-schema
  reader, and the generated system layout (pool reserved, mappings per role,
  non-cacheable attribute, manifest DMA addresses).
* The existing router-core, PCIe, network-health, GMAC DMA, RTL8125, NAT,
  bridge, DHCP, supervisor and checksum-offload regressions run through
  `scripts/test-router-integrated` and `python3 -m unittest discover -s tests`.

## Remaining hardware validation

1. iATU inbound window reachability at the widened sizes (readback is
   verified in-software; device visibility is not).
2. GMAC0 32-bit identity reach of PA 0x24400000 and RTL8125 bus reach of the
   pool, with a peer capture on each port.
3. Cache/NC visibility: CPU NC writes visible to both devices; device writes
   visible to NC reads on all four CPUs (RK3568 interconnect behaviour).
4. Ring wrap and backpressure with the mixed private/external descriptor
   streams on both drivers.
5. `iperf3` throughput comparison against the copy-path image on the same
   board, per direction and dual-LAN concurrent; only then report gains.

Do not flash hardware or modify the recovery eMMC from this workflow. See
`docs/router-operator.md` for guarded SD deployment and `docs/hardware-test-plan.md`
for staging; SD writing procedure is in `docs/sd-writing.md`.

## Guarded SD deployment (operator procedure)

Only run these steps on a board whose acceptance stage allows the multicore
router image, after a byte-verified build bundle (below) exists. The workflow
never writes block devices automatically.

1. Verify the bundle hashes before touching media:
   `sha256sum -c r5s-dual-lan-router.tar.gz.sha256` and
   `python3 scripts/verify-image out/<build>/rtl8125-lan-repair/r5s-rtl8125-lan-repair.img`.
2. Confirm the target device node with the operator's own inspection (for
   example `lsblk`) and that it is the SD reader, **never** the eMMC that holds
   the recovery Ubuntu. The eMMC must remain untouched: no partition table, no
   byte of it is written.
3. Follow `docs/sd-writing.md` for the exact writer invocation, using the
   verified image path from the bundle. Re-read and compare the first and last
   MiB after writing.
4. Insert the SD, keep the recovery eMMC selection unchanged, and power-cycle
   into the SD path only. If the board does not reach the router image, stop
   and use the existing recovery procedure — do not retry blindly.
5. Pull diagnostics before any traffic test:
   `python3 scripts/read-b51-udp <lan-host>` and confirm `zc.enabled` is true,
   `ownership_errors` is 0 and `pool.in_use` moves with traffic.

## iperf3 benchmarking (board required)

Measure the copy path and the zero-copy path on the same board, same cabling,
same client machines. The firmware does not estimate throughput; only the
board measurement counts.

1. Baseline: build the same stage without `R5S_ROUTER_SCHEDULE=multicore`
   (or with the previous `r5s-parallel` image) and record `iperf3` results.
2. Zero-copy image: from a LAN client to a WAN-side server and back:
   `iperf3 -c <server> -t 30 -P 4` per direction, single-LAN first, then both
   LANs concurrently. Repeat 3 runs each.
3. During each run capture B51 status twice (start and end) and record
   `zc.packets_forwarded`, `zc.copied_bytes`, `pool.in_use`, `queue_drops`
   and `ownership_errors`. A run with non-zero ownership errors is invalid.
4. Report throughput only after these paired measurements, and state the
   board revision, link speed and CPU placement with every number.
