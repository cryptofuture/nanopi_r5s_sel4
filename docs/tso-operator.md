# NanoPi R5S hardware TCP Segmentation Offload

This delivery adds hardware TCP Segmentation Offload (TSO) to the GMAC0 WAN
and both RTL8125 B51 LAN interfaces of the native seL4/Microkit router,
integrated with the existing checksum-offload, multi-descriptor TX,
zero-copy forwarding and multicore NAT architecture. Ordinary NAT
forwarding is unchanged: forwarded TCP packets are never reassembled to
feed TSO. TSO activates only for valid oversized TCP transmit requests —
router-originated traffic or the board-side test source.

## What is host-tested vs hardware-validated

**Host-tested (all passing in this delivery):**

- GMAC0 TSO descriptor encoding (context descriptor with MSS, TSE first
  descriptor with TCP-header length and 18-bit payload count, ≤16383-byte
  payload chunks, OWN/LD placement) — `tests/gmac_tso_test.c`, 11 groups.
- RTL8125 B51 TSO descriptor encoding (GiantSendv4/v6, GTTCPHO, MSS in
  opts2, FIRST/LAST/RingEnd, 32-byte no-close format) — 46 Rust tests in
  `drivers/rtl8125/`.
- MSS, MTU and header-offset validation (`head_len + mss <= 1514`, MSS
  64..2047, TCP header 20..60 bytes) and length-sum checks —
  `pds/gmac_dma_corrected/src/tso.rs`.
- IPv4 TCP software segmentation with fully verified IP and TCP checksums
  (independent recompute + one's-complement property), odd-length payloads,
  IPv6 segmentation with pseudo-header checksums — `tso.rs`,
  `tests/tso_testgen_test.rs`.
- Scatter-gather buffer lifetime: buffers stay owned until every
  descriptor of their TSO request has completed
  (`dma_reclaim_full` request reporting; `Ring::retired_tso`).
- RTL8125 63-outstanding reservation: a TSO request reserves all its
  descriptors atomically, wraparound across slot 63→0, saturation returns
  busy/fallback without touching the ring.
- Partial TX completion and DMA reclamation; link-down/reset/shutdown with
  in-flight TSO slots (GMAC stop/restart record clearing).
- Invalid oversized requests return explicit errors; fallback never emits
  frames above 1514 bytes and never silently drops.
- Normal NAT forwarding with TSO enabled but unused: the complete existing
  suite (router-core, ethernet, zero-copy, GMAC DMA, RTL8125, B51
  diagnostics, image rules) passes unchanged.

**Not hardware-validated (pending physical R5S run):** actual wire-level
segmentation. The procedure — with the built-in test source and packet
capture checks — is `docs/tso-hardware-validation.md`. Until that run, the
delivery claim is: host-tested, hardware support implemented and
capability-gated.

## Runtime capability (auto-disable)

| NIC | Probe | Behaviour when absent |
|---|---|---|
| GMAC0 | `MAC_HW_FEATURE1` bit 18 (`GMAC_HW_TSOEN`) at DMA start | TSO off; requests take software fallback |
| LAN1/LAN2 | B51 chip identity (`10ec:8125` rev 05, `Method::B51`) | TSO off; same fallback |

Build-time controls `wan_tso`, `lan1_tso`, `lan2_tso` (env
`R5S_WAN_TSO=1`, `R5S_LAN1_TSO=1`, `R5S_LAN2_TSO=1`) are independent;
`R5S_TSO_TEST=1` embeds the board-side test source; `R5S_TSO_IPV6=0`
disables IPv6 TSO. The manifest records `tso_requested`, `tso_ipv6`,
`tso_test`. Runtime capability is reported in diagnostics.

## Diagnostics (B51 UDP schema 7, 304 words)

Per port (WAN / LAN1 / LAN2, 12 words each):
`requests`, `hw_frames`, `hw_segments` (estimated),
`fallback_frames`, `fallback_segments`, `invalid`, `desc_errors`,
`completion_failures`, `queue_drops`, `active`, `capability`, `ipv6`.
`scripts/read-b51-udp` probes 304/268/248-word schemas; `b51_status.py`
renders the `tso` block.

## Multicore safety and the forwarding constraint

DMA descriptor rings stay owned by their driver PDs. TSO-capable requests
reach the drivers through the existing bounded zero-copy TX queues as
contiguous reference groups (`FLAG_TSO_FIRST/CONT/LAST` in the reference
flags; first reference carries MSS/IPv6/TCP-header metadata), preserving
SPSC backpressure semantics; the driver PD is the sole ring consumer, so
no concurrent descriptor corruption is possible. Ordinary forwarded
packets keep the direct zero-copy path with in-place NAT and checksum
offload — no reassembly, no added copies, no added latency.

The software fallback segments oversized requests into MTU-sized frames
with fully computed checksums and sends them through the ordinary TX path;
DMA buffer ownership is held until the hardware TSO request's final
descriptor completes (`dma_reclaim_full` / `Ring::retired_tso`).

## Board-side test source

`tso_testgen` (built into the driver PDs with `tso-test`) synthesizes three
16000-byte IPv4 TCP requests (MSS 1460, known sequence/ID bases, pattern
payload) once the link is up and DMA is running, and submits them through
the same TX interface as router-originated traffic. Diagnostics counters
show requests/offload/fallback; `docs/tso-hardware-validation.md` describes
the capture checks (MTU-sized frames, sequence advance, checksums, PSH on
the final segment).

## Rebuild

Source `scripts/env`, then run
`python3 scripts/build-lan1-bringup NEW_DIRECTORY --lan-repair=both` with
`R5S_DUAL_LAN_STAGE=dual-lan-router`, `R5S_LAN_B51_UDP=1`,
`R5S_ROUTER_SCHEDULE=multicore`, `R5S_WAN_TSO=1`, `R5S_LAN1_TSO=1`,
`R5S_LAN2_TSO=1`, `R5S_TSO_TEST=1` twice into independent (nonexistent)
directories and package with
`scripts/package-b51-matrix FIRST REPEAT docs/tso-operator.md --dual-lan
--checkpoint dual-lan-router`. The two images must hash identically;
`scripts/verify-image` validates the manifest.
