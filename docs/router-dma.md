# GMAC DMA integration audit

This supersedes the remaining-work list in the historical `dma-strategy.md`.
The preferred cacheable Normal strategy is implemented in new profiles, without
changing the tested SDK kernel, loader, monitor, initialiser or old images.
Source/configuration qualification is distinct from physical DMA visibility.

`pds/gmac_dma/src/cache.rs::maintain` borrows the actual Rust TLS IPC buffer via
`sel4::with_ipc_buffer_mut`. It never aliases C `__sel4_ipc_buffer`. Generated
CapDL installs the driver's own VSpace at slot 3; the build asserts that exact
object binding. Operations validate the mapped pool, 64-byte bounds and overflow,
split at 4096-byte page boundaries, check each syscall result and issue DSB.
All 64 physical frames are separately present exactly once in the allocator
report. Kernel flush success alone cannot prove a mapping or device visibility.

The unchanged seL4 `src/arch/arm/64/kernel/vspace.c::makeUserPagePTE` selects
S2_NORMAL=15 for cached RAM and S2_DEVICE_nGnRnE=0 for cached=false. MMIO retains
Device semantics. Native HCR_DC establishes the default cacheable first-stage
regime; the kernel direct RAM alias is Normal WB. Loader translations are no
longer active after the established loader/kernel handoff. No MAIR, global
uncached semantics, cache/MMU state or mapping ABI is changed. These are generated
configuration/source checks, not captured live translation tables.

The pool is PA/bus [0x24000000,0x24040000), CPU VA
[0x60000000,0x60040000), cached Normal WB, owned only by the driver.
`dma-map.json` records every subrange. Allocator/layout reports include kernel,
PD segments, page tables, stacks, shared frames and loader copy regions. The
complete captured FDT reservations and earlier RAM exclusions remain checked by
the existing layout validator. Runtime relocated U-Boot overlap guards remain.

The device address contract is 32-bit low physical identity, following locked
Rockchip EQoS physical-address programming, not a cast from a user pointer.
The captured root-level MAC has no translation provider. That absence alone is
not proof. The code explicitly supplies a distinct physical base, zeros high
address words and disables enhanced addressing. Real device visibility remains
a hardware gate. ARM_SMMU is disabled; effective DMA containment is unproven.

`drivers/gmac0/dma/dma.c` uses 64 descriptors per queue. Each descriptor owns its
entire 64-byte cache line; each 1536-byte buffer also occupies exclusive lines.
The descriptor's hardware words occupy 16 bytes. Locked U-Boot
`drivers/net/dwc_eth_qos_rockchip.c::eqos_rockchip_config` selects an eight-byte AXI
bus. Its generic EQoS DSL calculation gives (64-16)/8=6. Ring length is 63;
TX tail is one past the published producer; RX tail follows the freed descriptor.
The separately fetched Linux v6.1 DWMAC4 reference confirms fatal bus error bit12
and context descriptor error bit13. No independently owned 16-byte cached slots
remain in the native backend. The earlier preparatory ring core is not linked.

Buffers are zeroed before publication, cleaned/invalidated before OWN is cleaned
and published, and invalidated before completion data is read. Reclaimed RX
buffers are invalidated/zeroed/rearmed. All operations cover entire exclusive
lines. MMIO writes use DSB; `volatile` is not treated as cache maintenance.
TX leaves one slot unused, checks a two-second completion deadline and validates
writeback errors. RX rejects incomplete/error/oversized frames before copying.
MAC CRC stripping is configured; actual RX length semantics remain a hardware
check. MAC/MTL sizes are derived from runtime feature registers and bounded.

The original proven link source is unchanged. Raw builds generate isolated
patches allowing descriptor bases and preserving active MAC bits during link
polls. They quiesce DMA before speed/duplex changes. Start/stop paths use bounded
polls; failed quiescence produces a terminal reset/fault rather than reusing a
possibly owned pool. Reconnection and lease changes advance a queue epoch;
stale queued frames cannot authorize a new configuration. DMA terminal faults
remain sticky in this experiment, requiring a later controlled boot.

Only copied CPU queues are shared with the Rust app. Each direction has eight
1536-byte slots, fixed head/tail bounds, release/acquire publication and epoch
checks. The app has no hardware/pool/GPIO mappings. One supervisor owns SYS.
On CPU0, MCS budgets per 1ms period are supervisor100us (priority110), driver200us
(105), app200us (104), health100us (100). Preflight omits the app budget. Polls
process at most eight completions/RX/TX requests per pass. Busy-running services
are preempted on budget exhaustion; counter reads do not provide wakeups.
Driver/app/health freshness is 100ms; PHY freshness is 500ms (poll every200ms).
The existing diagnostics poll deadline is10ms. Whole-kernel failure can freeze
SYS; no software pattern is guaranteed after loss of scheduling.

The preflight driver has no linked DMA backend, OWN publication or DMA-start
path. It executes real Rust cache syscalls over the actual reserved pool, then
the proven native link path. READY requires both. The raw profile adds actual
MAC/MTL/DMA and the existing nonce/sequence challenge-response-ACK app, not mock
completions. Only a matching fresh external ACK renews IO_VERIFIED for five
seconds; link/epoch loss revokes it. All timings remain nominal.

Host C tests use separate CPU-cache and device arrays and execute the actual
backend, including 6300 TX/2000 RX completions, wraps, backpressure, invalid
lengths, error flags, cache errors and bus faults. These model the protocol;
they do not establish hardware cache visibility, RGMII timing or packet I/O.
