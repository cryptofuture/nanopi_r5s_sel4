## §1 Task identity
- task_id: T1
- short summary: Additive zero-copy external-buffer support in RTL8125 ring.rs + dma.rs (publish_external, descriptor_address, arm_rx_external, receive_external, send_external, tx_external_address, translate_external, is_private_buffer) with host tests, all green.

## §2 Subagent intent
Implement part of a zero-copy DMA forwarding feature in the audited seL4/Microkit router repo at /workspaces/mounted/projects/r5s_custom: extend the Dma trait and Ring (drivers/rtl8125/ring.rs) and PackedDma (drivers/rtl8125/dma.rs) so RX/TX descriptors can reference the shared packet pool through each device's PCIe inbound window instead of copying payloads through the private 2048-byte buffer arena, while preserving all existing behavior, signatures and tests and adding Model/PackedDma-based host tests. Mid-task inbox refinements were incorporated: arm_rx_external takes bus: Option<u64> (None re-arms the private buffer for pool-exhaustion fallback), invalid external completions re-arm via arm_rx_external(Some(previous_bus)), and the copy-path receive skips externally-armed slots so the two receive paths never consume each other's slots.

## §3 Files and code sections
- drivers/rtl8125/ring.rs: Extended the Dma trait with publish_external and descriptor_address; added Rx::External(usize, u64, usize); Ring gained rx_external: [u64; N] (0 = private, reset in initialize); added arm_rx_external(&mut self, io, slot, bus: Option<u64>), receive_external(io, error_mask) plus private receive_external_frame, send_external(io, bus, length, options) plus private send_external_frame, and tx_external_address; receive_frame now returns Rx::Pending without consuming when the cursor slot is externally armed. Test Model gained addresses/external_publishes tracking and publish_external/descriptor_address implementations. Added 5 host tests (external receive without payload copies, mixed private/external cursor discipline, invalid external completions re-armed at the same address, send_external validation, tx_external_address readback, copy/external path interleaving and the None fallback).
- drivers/rtl8125/dma.rs: PackedDma now stores window: InboundWindow (set in from_verified_memory); added translate_external(physical, size) returning Layout on failure and is_private_buffer(bus) with exact [buffer_bus, buffer_bus + N*BUFFER*2) edges; implemented Dma::publish_external (flags shape exactly as publish accepts, options == 0 only, slot not device-owned, address 2048-aligned and outside the private arena, no payload byte access, address/options words then ordered() then flags; TX no-close in-order slot == tail % N -> Layout and tx_inflight() >= N-1 -> Busy checks mirroring write_padded_payload) and Dma::descriptor_address (two 32-bit words with ordered() before the loads). Added 6 host tests (send_external address/length/options, no-close out-of-order rejection, no-close capacity Busy, publish_external validation, two-half 64-bit address readback, translate_external bounds and private-arena edges).

## §4 Verbatim commands
Exact commands run from the repo root (run `. scripts/env` first only if rustc is not on PATH):
```
mkdir -p out && rustc --edition 2021 --test drivers/rtl8125/lib.rs -o out/pcie && out/pcie
```
Expected summary line: `test result: ok. 36 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (25 pre-existing tests unmodified + 11 new).

## §5 Outcome and discoveries
- Outcome (success/partial/failed): success — all 36 host tests pass with zero warnings via the command above; only drivers/rtl8125/ring.rs and drivers/rtl8125/dma.rs were modified; nothing was committed to git.
- Discoveries that may matter for other tasks:
  - No-close in-order/capacity enforcement for external TX had to live in PackedDma::publish_external (TX only): the two-method Dma trait extension exposes no no_close_tail/tx_closed to Ring and the check must be mode-aware (identical observable states require different answers with vs without no-close). Error semantics match write_padded_payload (Layout out-of-order, Busy when full). This is the one reported deviation from "do this check explicitly in the Ring method".
  - rx_external[slot] == 0 is the private-slot sentinel, so arm_rx_external(Some(0)) is rejected with Layout; arm_rx_external also rejects slot >= N with Layout to avoid Ring::desc arithmetic overflow on the None form.
  - send_external's END flag must be conditional on slot == N-1; a literal unconditional END fails publish's flags-shape validation on non-final slots.
  - The Rx::External path does not re-arm the slot; the caller must re-arm with arm_rx_external before the RX cursor wraps back to it or the stale completion is reported again (documented on receive_external).
  - Cursor discipline is symmetric and enforced in code: receive_external consumes only slots with rx_external != 0, receive consumes only slots with rx_external == 0, and both return Rx::Pending on the other's slots without advancing.
  - Baseline before this change: 25 tests green; the ring.rs test Model is an N=4 flags matrix; dma.rs tests use crate::mac::tests::Model plus Aligned buffers; PackedDma::publish strictly requires address == buffer_bus + (index*BUFFER).
