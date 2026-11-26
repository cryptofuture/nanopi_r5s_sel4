# Native network milestone: baseline ready for operator testing

The user reported the unprefixed three-short Rust heartbeat from preserved `r5s-boot-trace.img` (`4cfe4737605b507a65394f931b5415aa1bbcdc003f5aac33fe097a34d7b7789d`). Record this as user-observed native Rust-PD startup through the intended seL4/Microkit boot path. It is not independent laboratory verification and supplies no Ethernet evidence. Earlier loader-stop video interpretation remains tentative. Earlier Ubuntu card-removal recovery was observed; recovery after the latest full boot and eMMC before/after identity remain unverified.

Only `r5s-runtime-baseline-v2.img` is built. It has real kernel/loader/initialiser/monitor and two Rust PDs. It does not initialize Ethernet, map GMAC, allocate DMA, or emit IO_VERIFIED. `r5s-gmac-link.img` and `r5s-gmac-rawio.img` are NOT built. There is no placeholder image or bypass: their profiles are rejected by the image validator. Native bidirectional RX/TX integration is not implemented or proved. The remaining work is substantial and explicitly listed below.

## Boot and diagnostics changes

Pinned DDR v1.26, BL31 v1.46, SPL and generated U-Boot DT remain byte-identical to the proven standalone diagnostic. Source locks, GPT-parser correction, loader stack low-address correction, EL2 tests, exact payload integrity checks, FIFO SD access, quiescence, entry assembly, RAM restrictions and debug mappings remain. New U-Boot expectations bind this baseline's actual payload size/hash. New U-Boot proper, loader, kernel, monitor, initialiser image and PD assembly differ; initialiser source is unchanged. Generated maps and ELF files are under `runtime-baseline-v2/`; `logs/fit-inventory.log` records the actual FIT inventory.

`scripts/build-native-net` reuses `handoff_trace.instrument`, `build_uboot` and `layout`. `build_uboot` gains an optional header argument; default behavior remains the previous trace implementation. A new format-4 baseline validator retains all normal payload/hash validation and rejects network profiles until qualified. Original image formats and negative-image checks remain intact.

One generated diagnostics specification drives the Rust display, manifest, early-stage header, operator pattern table and text preview: `protocol/diagnostics-v2.json`. See [diagnostics-v2.md](diagnostics-v2.md). Early checkpoints each emit one WAIT cycle; detected early errors repeat FAULT. There are no numerical progress/error flash counts in this new image. Original trace artifacts remain available unchanged for regressions.

In `pds/diagnostics/src/main.rs`, the supervisor acquires the existing monitor release flag at 0x40001000, sets GPIO direction with the proven masked GPIO-v2 write, verifies CNTFRQ=24 MHz, then polls counter, shared health progress and display state. Monitor writes no GPIO after releasing ownership. Kernel retains its debug-only non-owning GPIO alias; it does not run a competing LED task. The health PD has only a shared Normal cached page, not peripheral mappings. The supervisor has the single GPIO4 Device page at VA 0x40000000, PA 0xfe770000, plus the shared page. GPIO pin 26 writes use 0x04000000/0x04000400 at data-high offset 4 and masked direction-high offset 12, with DSB. Other bank bits are preserved; controller-page capabilities are broader than this one bit.

Supervisor priority 110 and health priority 100 each have 100 us budget per 1000 us period on CPU 0. Both remain runnable and use finite scheduling contexts; there is no synchronous IPC or blocked timer wait. This intentionally spends up to 20% CPU polling for the baseline, and is not a network scheduling design. Reading CNTVCT does not wake a blocked PD. No kernel scheduler timer is reprogrammed. Actual scheduling and counter access in this new two-PD image remain hardware-untested.

READY requires fresh health-PD increments within 100 ms, expected counter frequency and no observed supervisor polling gap over 10 ms. Fault is sticky for unexpected counter frequency or supervisor deadline miss. Only the supervisor owns the LED during userspace. A dead health PD is detected without IPC; total scheduler/kernel failure may freeze the LED. State changes preempt frames, start a 500 ms OFF delimiter, and then start the new rhythm. Under the declared 10 ms scheduling bound, stale IO success is revoked within 10 ms of expiry; first FAULT light begins within 510 ms of detecting an overdue condition. These are software bounds conditional on execution, not hardware latency guarantees. Baseline cannot establish packet proof.

## GMAC evidence and exact blockers

`boards/nanopi-r5s/gmac0-evidence.json` distinguishes measured facts from reference candidates. The locked board DTS, `vendor/u-boot/dts/upstream/src/arm64/rockchip/rk3568-nanopi-r5s.dts:74`, describes output 125 MHz clock, PHY address 1, rgmii, MAC delays 0x3c/0x2f and active-low GPIO0_C5 reset delays 0/15000/50000 us. These are upstream reference values, not the missing captured runtime board configuration. They are not written by this implementation. No PHY ID/revision was captured here. Resolve complete-tree pinctrl, clocks, regulator and reset phandles before adopting those candidates or programming PHY delays. Do not double-apply delays.

The locked SoC DTS at `rk3568.dtsi:257` identifies GMAC0 PA 0xfe2a0000, 64 KiB region, dwmac-4.20a, SPI 27 and wake SPI 24. Linux IRQ numbers must not be reused as seL4 IDs. Neither interrupt is allocated in the baseline. GMAC1 and both RTL8125 remain inactive/unowned. Identify the physical socket corresponding to measured eth0, the sole onboard 1 GbE port, from board evidence before the later isolated-link test; no WAN/LAN label or physical position is inferred here.

There was no native NIC service to reuse. The [published sDDF support list](https://github.com/au-ts/sddf/blob/main/docs/drivers.md) names 5.10a and 5.20; 4.20a compatibility is not established. No sDDF version was added as a dependency. The already locked U-Boot EQoS and Rockchip files are reference material only. `native-sources.lock` records their hashes and the unchanged old lock. `drivers/gmac0/mdio.c` implements and host-tests bounded Clause-22 reads through injected register/time interfaces. The independent descriptor core adds bounded submit/reclaim/copy/rearm with mandatory ordering hooks, zero padding, bounds, errors, ownership, backpressure and wrap tests. No live MMIO backend or full native MAC/PHY/RX/TX service is supplied. No U-Boot network stack was enabled. No new GPL driver code was copied.

There is a separate confirmed integration blocker: `vendor/sel4/src/arch/arm/64/kernel/vspace.c:797` maps uncached user frames as Device nGnRnE. Consequently `cached=false` is NOT a suitable Normal noncacheable DMA pool in this pinned kernel. A reviewed kernel mapping extension or verified cache-maintenance route is required. Baseline does not change those semantics. Cached and uncached aliases must not be created speculatively. No DMA addresses, pool, descriptor ring, bus translation, hardware coherency or effective DMA firewall have been verified. `dma-map.json` deliberately lists no allocations. `native_layout.py` is a host-side qualification check, not a DMA allocator or proof of coherency. [DMA address domains and barriers](https://docs.kernel.org/core-api/dma-api-howto.html) remain separate obligations from CPU capabilities. Rust memory safety and seL4 CPU isolation do not constrain an enabled DMA bus master. No DMA isolation is claimed.

Needed next: the single collector archive below; resolve its full phandles/reservations and actual PHY identity; establish independent native clock/reset/supply/pinmux sequences; implement/qualify Normal-memory DMA mappings, bus-address translation, allocation and cache/barrier ownership; integrate descriptor core, MAC/MTL control, driver, bounded service queues and protocol PD scheduling; implement reset/autonegotiation/reconnect, descriptors, RX errors, TX completion/backpressure and terminal fault recovery. Ring wrap is tested with mocked descriptor completions only. Real driver timeouts, cold starts, actual RX/TX and leases cannot be validated by the present preparation code.

## Read-only evidence collector

After baseline testing and Ubuntu recovery, use the bundled collector once, if this evidence is still absent. It does not change networking, reset the PHY, unbind eth0, install tools, use /dev/mem or perform MDIO writes. It verifies board identity and destination backing with the existing hidden-overlay resolver. Keep the destination on eMMC, not SD:

```sh
cd /root/r5s-flash/native-net
python3 scripts/collect-gmac-evidence --output /root/r5s-flash/native-net/gmac-evidence.tar.gz
sha256sum gmac-evidence.tar.gz
```

It copies original `/sys/firmware/fdt` when exported, the complete live tree, `/proc/iomem`, command line, mount evidence, eth0 device/driver/of_node/phydev links, MDIO sysfs IDs, and available read-only ethtool results. Optional dtc reconstruction is explicitly labelled reconstructed and cannot recover the original FDT reservation table. Missing tools/results are recorded, never installed. Archive may contain host/device identifiers; transfer it to the development workspace for analysis. Redacted iomem entries are insufficient evidence of physical allocations.

## Protocol preparation, not a network acceptance result

`pds/net_probe/src/lib.rs` is a dependency-free no_std Rust parser and challenge/response/ACK state machine tested on the host, not connected to native DMA. `tools/net-peer` is the Linux Python standard-library AF_PACKET peer; it was not run on a network. Protocol v1 uses EtherType 0x88B5, local experimentation per the [registry](https://www.iana.org/assignments/ieee-802-numbers), on an isolated bench link only. The reserved locally administered unicast board MAC is 02:52:35:53:00:01; it has not been programmed into hardware.

Wire header is 68 bytes, network byte order: magic R5SN (4), version (1), type (1: challenge, 2: response, 3: ACK), data length (2), session nonce (16), sequence (8), build ID (16), zero reserved (4), RX count (8), TX count (8), followed by at most 1400 arbitrary data bytes. Maximum Ethernet frame is 1482 bytes excluding FCS. Minimum is 82 bytes, so Ethernet minimum padding is unnecessary; trailing bytes, tagged frames and FCS included in captures are rejected. AF_PACKET normally excludes FCS. RX FCS stripping/length conventions in a future MAC implementation must match this contract.

Response echoes session/sequence/build/data and generated counters; ACK must exactly bind that outstanding response, including counters, within 2000 ms. It is consumed once. Link loss revokes pending proof; display leases expire at 5000 ms. Same-session non-increasing challenges are rejected. Nonces prevent accidental stale/self-loop success, not malicious replay or peer authentication; this is not a secure management protocol. The protocol accepts no commands, broadcasts or forwarding. Status ABI and driver-health integration are not yet implemented. Internal baseline status fields and sticky reasons are recorded in `protocol/runtime-status-v1.json`. They are trusted shared debug RAM, not a coherent network snapshot or read-only capability ABI. Future build ID must derive from source/config/protocol inputs, never the final self-referential image hash.

Peer requires explicit interface, unicast target, expected build ID and new report path. It ignores PACKET_OUTGOING, rejects wrong MACs/type/identity/session/sequence/length/content, sends one ACK only after validation, caps rate at 10 Hz (default 1), uses a 750 ms deadline and no retries. Reports distinguish validated responses, ACKs sent, loss, duplicates, mismatches, ignored frames and host RTT. ACK sent is not evidence the board accepted it. There is no runnable rawio command with an invented build ID because no qualified rawio image exists. `python3 tools/net-peer --help` is safe to inspect. Later raw-socket use requires privileges but must never automatically change addresses, routes, firewall or NetworkManager.

## Development build and checks

Existing development dependencies and locked compiler/Rust/Python caches are retained. The actual build uses the pinned Microkit 2.4.0/seL4/rust-sel4 and GNU 12.2.Rel1 inputs, not unrelated SDK binaries:

```sh
. scripts/env
python3 scripts/build-native-net
python3 -m unittest discover -s tests -v
python3 scripts/package-native-net
```

A prepared native worktree rejects changed inputs rather than silently reusing stale instrumentation. For a fresh compile, preserve/move its work directory and reuse the local locked caches; do not remove old artifacts. Existing image names refuse conflicting content. `CARGO_NET_OFFLINE=true` can require cached Cargo dependencies. SDK source and toolchain caches remain in the development repository; the operator bundle is not a self-contained compiler distribution. Final image assembly was performed twice and compared; full toolchain rebuild reproducibility is not claimed. Logs, configuration hashes, real loader expectations, source patches, SDK ELFs, maps and memory checks are shipped. Hardware-only assumptions are explicit in layout-report.json.

## First operator action: baseline only

These commands are instructions for the operator, NOT actions executed by Codex. First boot working Ubuntu without SD and confirm current SSH access/recovery; preserve any real existing before-test hashes. Do not create a new baseline and describe it as evidence about previous boots. Only after recovery/access is established, reinsert SD and stage this bundle on eMMC. No PC card reader is required.

On the development computer, set the existing SSH address yourself; no new address is inferred:

```sh
R5S_HOST='YOUR_EXISTING_R5S_SSH_ADDRESS'
scp out/native-net/native-net-handoff.tar.gz out/native-net/native-net-handoff.tar.gz.sha256 "root@$R5S_HOST:/root/r5s-flash/"
```

On R5S Ubuntu, dependencies must already be present or installed separately by the operator. Check:

```sh
command -v python3 fsck.fat mcopy mdir mtype blkid lsblk findmnt swapon mountpoint blockdev stat cmp sha256sum tar gzip mount umount shutdown
```

Optional operator installation, never performed automatically:

```sh
apt-get update
apt-get install --no-install-recommends python3 dosfstools mtools util-linux mount coreutils tar gzip
```

As root in Bash, extract into a NEW directory. Stop on any error; no force/lazy unmount, GPT repair/expansion or fstab edits:

```sh
set -euo pipefail
cd /root/r5s-flash
sha256sum -c native-net-handoff.tar.gz.sha256
test ! -e native-net
tar -xzf native-net-handoff.tar.gz
cd native-net
sha256sum -c SHA256SUMS
findmnt --target .
findmnt --target /
test "$(cat /sys/class/block/mmcblk0/device/type)" = SD
test "$(cat /sys/class/block/mmcblk0/device/name)" = NCard
test "$(cat /sys/class/block/mmcblk0/device/serial)" = 0x0d3aa313
test "$(cat /sys/class/block/mmcblk0/device/cid)" = 8903034e43617264100d3aa313016c07
test "$(blockdev --getsize64 /dev/mmcblk0)" = 62534975488
if mountpoint -q /mnt/r5s-boot; then umount /mnt/r5s-boot; fi
lsblk --bytes --paths -o NAME,TYPE,SIZE,MOUNTPOINTS /dev/mmcblk0 /dev/mmcblk2
swapon --show
python3 scripts/flash-sd --device /dev/mmcblk0 --profile r5s --image r5s-runtime-baseline-v2.img --expect-model NCard --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488
```

The writer preserves exact FriendlyElec model and both compatible checks, hidden eMMC overlay/ext4/UUID proof, topology and mount/swap/source dependencies, identity/capacity/image validation and exclusive kernel claims. Unknown evidence refuses. Only after successful dry run, the separate destructive command is:

```sh
python3 scripts/flash-sd --device /dev/mmcblk0 --profile r5s --image r5s-runtime-baseline-v2.img --expect-model NCard --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488 --execute
```

Type exactly:

```text
OVERWRITE /dev/mmcblk0 8903034e43617264100d3aa313016c07 62534975488
```

The writer reinspects after confirmation and retains O_RDWR|O_EXCL through write, flush and exact-image-length comparison. Do not substitute dd. Optional extra bounded read comparison after successful writer verification:

```sh
cmp -n "$(stat -c %s r5s-runtime-baseline-v2.img)" r5s-runtime-baseline-v2.img /dev/mmcblk0
sha256sum -c SHA256SUMS
```

Only after successful verification, the operator may cleanly shut down:

```sh
shutdown -h now
```

Use exactly the physical SD-selection procedure that produced the known-good diagnostic and trace results. No new MASK ritual, eMMC erase or boot-priority change. Record SYS for the first 60 seconds from power application, preferably video; that window is not a guaranteed boot duration. Early slow WAIT cycles are deliberate. Acceptance for this first profile is sustained isolated READY ticks (200 ms ON, 3800 ms OFF), followed by recovery. Flutter/darkness means FAULT; no rhythm/solid state is inconclusive. Power off, remove experimental SD, normal power-on, verify Ubuntu and SSH. Compare genuine available before/after hashes. Recovery failure means stop, never automatically erase/reinstall Ubuntu.

Do NOT flash link or rawio variants now: they do not exist. After baseline and recovery, collect the missing evidence and finish native integration. Promotion then proceeds link/recovery before rawio/recovery. For later networking disconnect the experimental port from production networks and connect the measured 1 GbE socket directly to a dedicated peer Ethernet port. No DHCP or Internet is needed. Keep all other ports inactive.

## Predeclared later hardware suite

1. Baseline READY and Ubuntu recovery, then an independent cold start.
2. Qualified link image: actual PHY ID, negotiated speed/duplex, cable absent WAIT, cable insertion READY, removal WAIT, reconnect and Ubuntu recovery. DMA remains off.
3. Qualified rawio: 1000 fresh transactions, default 1 Hz, varied/random/max data lengths, repeated ring wrap, bounded errors/backpressure and driver faults, malformed/tagged/unrelated/oversize frames without crash, stale ACKs rejected. Retain host JSON and video; a link LED or TX completion is not packet proof.
4. Cable removal or stopped peer revokes IO_VERIFIED within the lease plus scheduling bound; stale driver/probe health causes FAULT. Reconnect and fresh challenges must work. Cold-start and repeated boot/link cycles must initialize independently of Linux state.
5. Power off, remove SD, boot Ubuntu and confirm access after each promoted profile. Stop on regression. No throughput, firewall, latency, DMA confinement or formal verification claim follows from this bench test.
