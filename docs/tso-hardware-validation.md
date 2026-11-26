# TSO physical validation procedure (not yet executed)

Everything delivered by the TSO work is host-tested only. This file specifies
the board-side procedure that must run on a physical NanoPi R5S before any
claim of hardware segmentation. No step here has been executed by the
build/test workflow; nothing is flashed or written to eMMC by it.

## Prerequisites

- Two reproducible firmware builds verified byte-identical
  (`python scripts/build` twice or the packaging script), image + deployment
  bundle paths published in the delivery notes.
- A receiving machine on the LAN (or WAN) side with `tcpdump`/Wireshark and
  `iperf3`, cabled to the R5S port under test.
- Recovery Ubuntu stays on eMMC and is not touched. SD deployment follows
  `docs/sd-writing.md`; flashing is an operator decision.

## Test source

The board-side TSO test source (`tso_testgen`, built into the WAN and LAN
driver PDs behind the `tso-test` feature) synthesizes valid oversized TCP
transmit requests — Ethernet+IPv4/IPv6+TCP headers with a declared MSS and a
payload larger than one segment — and submits them through the same TX
interface as any router-originated stack would. It does not depend on NAT
traffic. Generated request counts and outcomes appear in the TSO diagnostic
counters (B51 UDP page for LAN1/LAN2, GMAC status words for WAN).

## GMAC0 (WAN) hardware segmentation

1. Boot the TSO-enabled image with `wan_tso` requested. Confirm diagnostics
   report hardware capability in `wan.dma_capabilities.tso` and verify that
   `tso.wan.active` becomes true after link-up.
   If the feature is absent or enable readback failed, expect software
   segmentation.
2. Run the TSO test source with a known pattern (e.g. 16000-byte payload,
   MSS 1460, TCP seq base 0x10000000, IPv4 ID base 0x2000).
3. Start capture before boot or connecting the WAN cable. The generator sends
   when the driver first reaches link-up. On an isolated wired peer capture
   `tcpdump -i <if> -w tso-wan.pcap 'ether dst 02:00:00:00:00:02 and tcp port 5201'`.
4. Verify in the capture:
   - every emitted frame length ≤ MTU + 14 (+VLAN if any), none oversized;
   - segment payloads are exactly MSS bytes except the final one;
   - TCP sequence numbers advance by MSS per segment;
   - IPv4 identification advances by 1 per segment (hardware behavior may
     differ — record what the silicon does);
   - IPv4 header checksums and TCP checksums verify (Wireshark "good");
   - PSH only on the final segment;
   - the segment count equals ceil(payload_len / MSS).
5. Rebuild the same profile with `R5S_TSO_TEST=1 R5S_TSO_TEST_SOFTWARE=1`.
   The generator submits the same frames through software segmentation on
   all requested interfaces. Require the same wire-level properties and
   confirm that the TSO fallback counters advance.

## RTL8125 (LAN1/LAN2) hardware segmentation

Same procedure per port with the LAN test source, plus:

- Confirm `tso_capability=1` for the B51 identity (diagnostics page).
- The receiver should capture at 2.5G or 1G as negotiated; verify frames
  respect the configured MTU.
- Run the request while ordinary NAT forwarding is active and confirm
  forwarded traffic is unaffected (counters unchanged apart from TSO).
- Exercise the 63-outstanding pipeline: submit several TSO requests back to
  back; confirm no descriptor errors, correct completion counts, and that
  saturation at 63 outstanding returns busy/fallback rather than corruption.

## Negative tests on hardware

- TSO request with invalid MSS → explicit error counter, nothing on the wire.
- Link-down during in-flight TSO → clean stop, no stuck descriptors after
  link restore; counters show completion failure or timeout handling.
- Reset/shutdown during TSO (`wan-shutdown` build) → documented shutdown
  code, no hang.

## Sign-off

The UDP diagnostic reader reports hardware TSO submissions as `hw_frames`
and confirmed request completions as `hw_completed` on firmware that supplies
it. A completed request still needs receiver-side validation of the segments.

Record: board serial, image SHA256, manifest `tso_*` keys, capability bits
read at runtime (GMAC `MAC_HW_FEATURE1`, B51 identity), capture file hashes,
and which of the above checks passed. Only then may the delivery claim
hardware-validated TSO; until then the claim is host-tested with hardware
support implemented and capability-gated.
