# Native GMAC0 link-test handoff

The user observed both the original unprefixed Rust heartbeat and the baseline-v2 READY tick. Those are user-observed native boot results, not independent laboratory verification. Ubuntu was available for this evidence collection; a controlled latest recovery test or immutable eMMC before/after comparison is not inferred.

The first new image is **r5s-gmac-link.img**. It uses the preserved baseline kernel/loader/monitor/initialiser binaries, with native driver and supervisor PD changes. No new kernel/memory ABI was introduced, so no extra no-Ethernet regression image is required and the unchanged baseline need not be tested again. The original tested images remain unchanged.

This image performs real native clock/reset/pinctrl/MDIO/RTL8211F initialization and live link polling. MAC RE/TE, DMA ST/SR and descriptor bases are required inactive. There are no active rings or packet buffers. No U-Boot network service or Linux datapath is substituted. **Native link and raw RX/TX have not been tested on hardware.** A PHY link or READY is not packet-I/O proof.

Read [gmac-hardware-audit.md](gmac-hardware-audit.md) for exact captured evidence, register/source references and memory reservations; [dma-strategy.md](dma-strategy.md) for qualified cache APIs and remaining DMA gates; [gmac-bringup-results.md](gmac-bringup-results.md) for build/test status. Original and reconstructed FDTs remain distinct. All supplied inner checksums were verified.

## Diagnostics and scheduling

The existing machine-readable diagnostics-v2 specification is unchanged. SYS has one userspace owner, the supervisor. Monitor release/acquire ownership and the existing kernel debug alias are retained. Driver has GPIO0 for PHY reset, never GPIO4/SYS. Health PD has no MMIO. Supervisor maps link status read-only; protocol code receives no platform registers.

| State | Nominal pattern | Link profile meaning |
|---|---|---|
| WAIT | 2000 ms ON, 2000 ms OFF | Native initialization or cable/autonegotiation pending |
| READY | 200 ms ON, 3800 ms OFF | Native initialization passed, fresh valid negotiated PHY link and driver/health/supervisor progress |
| IO_VERIFIED | Long-short pair from the preserved specification | Not emitted by this profile; there is no packet proof input |
| FAULT | 2-second 125 ms flutter, then long darkness | Detected terminal operation, inconsistent state or stale essential service |

Every state transition starts a 500 ms OFF delimiter and preempts the previous frame. Partial frames and solid/default LED states are inconclusive. There are no appended numeric flash counts. An interconnect/kernel/counter failure can freeze the LED; FAULT is not guaranteed to execute after every possible failure.

All three PDs remain runnable on CPU 0 with bounded MCS budgets: supervisor priority 110, 100 us/1000 us; GMAC priority 105, 200 us/1000 us; health priority 100, 100 us/1000 us. Maximum declared total budget is 40% CPU; polling is intentionally conservative, not a throughput design. No kernel timer is reprogrammed, and no blocked PD is expected to wake merely by reading a counter. The native driver polls PHY state every 200 ms, publishes versioned status at most once per millisecond and cannot synchronously block the supervisor through IPC. Hardware busy operations have explicit deadlines. Initialization delays run only in the driver, whose budget leaves supervisor and health scheduling time.

Supervisor reads status with two bounded seqlock attempts and can use the last consistent snapshot while it remains fresh. It requires driver progress/update within 100 ms, a real PHY sample within 500 ms after initialization, valid speed/duplex, and the original health/counter checks. Link loss revokes READY on the next valid sample, normally within the 200 ms polling period; the software allowance is 500 ms after the PHY reports the change, conditional on the declared 10 ms supervisor scheduling bound. Physical PHY detection time is not measured here. Stale driver/health progress faults after 100 ms; a stale PHY sample faults after 500 ms. First FAULT light then needs the 500 ms delimiter plus scheduling allowance. Exact timings remain nominal until measured.

`protocol/gmac-status-v1.json` specifies the shared status fields. Native reasons are 1 clock precondition, 2 unsupported MAC revision, 3 inactive-DMA guard, 4 MDIO operation, 5 PHY identity, 6 software-reset timeout, 7 negotiated-state inconsistency, 8 masked setup/reset readback, 9 fixed IO-domain precondition, 10 counter frequency, 99 rejected write. These are RAM/optional serial values, not LED counts or persistent logs. The debug driver prints initialization IDs/fault and link transitions on optional serial; no console input or adapter is required to boot. Status is read-only to the supervisor, not a network-management interface.

## Development commands and reproducibility

The actual checkout is `/workspaces/mounted/projects/r5s_custom`. Existing firmware/toolchain/SDK pins and local caches are reused. No physical-board package installation or action was performed. On the development computer:

```sh
cd /workspaces/mounted/projects/r5s_custom
. scripts/env
python3 scripts/resolve-gmac-evidence
python3 scripts/generate-gmac-config
python3 scripts/build-gmac-bringup
python3 -m unittest discover -s tests -v
python3 scripts/package-gmac-bringup
```

The imported evidence is already present. To repeat import elsewhere in this same checkout, use NEW destination names, preserve both archive layers and use `gmac_evidence.py`; it refuses an existing destination. Do not extract over earlier evidence. The resolver's paths describe the inspected layout of this particular capture, not a guessed generic archive structure.

Build inputs are hashed before compilation and checked again before assembly. Cargo builds offline with the existing locks. Each finished image is assembled twice and compared. That is image-assembly reproducibility, not a complete second compiler rebuild or cross-host guarantee. Changed/conflicting output is refused; preserve/move experimental outputs before rebuilding changed source. Known-good images and locks are never rebuilt in place. Source/config hashes, patches, SDK ELFs, maps, actual payload expectations, FIT inventory and interval/capability reports are included. Runtime provider frames are trusted, page-granular authority; software write masks are not a malicious-driver security boundary.

## Operator transfer and staging

Everything below is a later **user-run procedure**, not authorization exercised by Codex. No SSH, raw device access, network reconfiguration, PHY operation, shutdown or reboot was executed against the R5S.

Boot working Ubuntu from eMMC without SD and establish access as usual. Preserve genuine existing boot-region hashes; do not relabel a new baseline as proof about earlier boots. This task's baseline observation is accepted, not requested again. Reinsert the experimental SD only after Ubuntu is running. Source image, scripts, current directory and logs must remain on eMMC or verified RAM, never SD.

From the development checkout, transfer the actual bundle to the existing eMMC staging parent:

```sh
cd /workspaces/mounted/projects/r5s_custom
scp out/gmac-bringup/r5s-gmac-bringup-handoff.tar.gz out/gmac-bringup/r5s-gmac-bringup-handoff.tar.gz.sha256 root@10.0.1.81:/root/r5s-flash/
```

On Ubuntu as root, check dependencies:

```sh
command -v python3 fsck.fat mcopy mdir mtype blkid lsblk findmnt swapon mountpoint blockdev stat cmp sha256sum tar gzip mount umount shutdown
```

If missing, the operator may separately install them, never automatically:

```sh
apt-get update
apt-get install --no-install-recommends python3 dosfstools mtools util-linux mount coreutils tar gzip
```

Use Bash. The archive has exactly one root, **gmac-bringup/**. Extract in `/root/r5s-flash/`, not inside a precreated gmac-bringup directory. Stop on any failure:

```sh
set -euo pipefail
cd /root/r5s-flash
sha256sum -c r5s-gmac-bringup-handoff.tar.gz.sha256
test ! -e gmac-bringup
test ! -L gmac-bringup
tar -tzf r5s-gmac-bringup-handoff.tar.gz > gmac-bringup.members.txt
sed -n '1,80p' gmac-bringup.members.txt
python3 - <<'PYSAFE'
import tarfile
from pathlib import PurePosixPath
with tarfile.open('r5s-gmac-bringup-handoff.tar.gz') as archive:
    seen=set()
    for member in archive.getmembers():
        path=PurePosixPath(member.name)
        if (path.is_absolute() or '..' in path.parts or not path.parts or path.parts[0]!='gmac-bringup'
            or not (member.isdir() or member.isfile()) or str(path) in seen):
            raise SystemExit('Unsafe bundle member')
        seen.add(str(path))
PYSAFE
tar --no-same-owner -xzf r5s-gmac-bringup-handoff.tar.gz
cd /root/r5s-flash/gmac-bringup
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
```

Do not force or lazy-unmount, edit fstab, suppress errors, expand/repair GPT or substitute dd. The writer checks all mounts, swap, topology, hidden overlay dependencies and source/writer/CWD placement; human inspection does not replace those checks. Internal `/dev/mmcblk2`, boot0/boot1/RPMB, SPI and persistent environments remain protected.

## Dry run, separate write and readback

First the default dry run:

```sh
python3 scripts/flash-sd --device /dev/mmcblk0 --profile r5s --image r5s-gmac-link.img --expect-model NCard --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488
```

Only after it passes, a separately chosen destructive write:

```sh
python3 scripts/flash-sd --device /dev/mmcblk0 --profile r5s --image r5s-gmac-link.img --expect-model NCard --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488 --execute
```

Type exactly:

```text
OVERWRITE /dev/mmcblk0 8903034e43617264100d3aa313016c07 62534975488
```

The full CID is required, not short serial 0x0d3aa313. The unchanged writer checks exact FriendlyElec model and both compatible strings, hidden eMMC OverlayFS root/data ext4/UUID evidence, target identities and capacity, source image integrity, mounts/swap/hidden dependencies, exclusive kernel claim, confirmation and reinspection. It keeps O_RDWR|O_EXCL through write, flush and bounded readback; no unprotected reopen is used for verification.

After the writer reports successful comparison, an optional additional read-only comparison uses the actual image length:

```sh
cmp -n "$(stat -c %s r5s-gmac-link.img)" r5s-gmac-link.img /dev/mmcblk0
sha256sum -c SHA256SUMS
```

Only after successful verification, clean shutdown may be chosen:

```sh
shutdown -h now
```

Use the exact physical SD-selection workflow that produced the known-good diagnostic, Rust and READY observations. No new MASK timing, eMMC erase or priority change is introduced.

## Link hardware acceptance and recovery

Before experimental boot, disconnect the experimental 1 GbE socket from production networking. Use the sole native 1 GbE port identified in the hardware audit, with a dedicated known partner; leave 2.5 GbE/other ports unused. The partner may run Linux. No IP configuration, DHCP, Internet, bridge, route or firewall change is required or authorized by these instructions. A partner's link alone cannot prove RGMII data timing or packet I/O.

1. Cold power-on with no cable. Record the first 60 seconds of SYS, preferably video; this is an observation window, not a boot-time promise. After early WAIT progress, the native link image should remain WAIT without cable.
2. Attach the dedicated partner. Successful native initialization plus fresh negotiated link must produce the isolated READY tick. On the partner, read-only `ethtool` output can corroborate speed/duplex; select its actual dedicated interface explicitly, without reconfiguration.
3. Remove cable. READY must be revoked when the PHY reports loss and the bounded poll observes it; observe WAIT, then reconnect and require READY again. Record times and complete rhythms, not pulse counts. FAULT is a failed operation/health result; stop promotion and retain optional serial details. A static/partial rhythm is inconclusive.
4. Power off. Cold boot again with cable already attached and require native READY, demonstrating the initialization does not depend on a prior Linux link state. Repeat cable cycles and preserve evidence.
5. Power off, remove experimental SD, boot Ubuntu normally and verify SSH at root@10.0.1.81. Compare only genuine available pre/post hashes. Recovery regression means stop experiments; never automatically erase/reinstall Ubuntu.

Do not flash a raw-I/O image now: none is supplied. After link/recovery success and the remaining DMA/software gates, a future raw profile must run a finite peer challenge/response/ACK suite (at least 1000 varied/random/max-length transactions and repeated ring wraps), malformed/stale frames, backpressure/fault recovery, cable loss/reconnect, five-second proof expiry, cold start and Ubuntu recovery. Preserve the external host report. The peer tool and protocol source are retained; there is no runnable raw test command with a fabricated build ID. `python3 tools/net-peer --help` only displays its current interface.
