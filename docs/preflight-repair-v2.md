# One replacement DMA-disabled preflight

The user confirmed FAULT on the first repair image. This second candidate fixes
status/time sampling order: a preempted supervisor could read a newer producer
timestamp using an older clock sample and latch a false timeout. Host tests
reproduce that defect; its role in the physical failure remains unknown. The
resolved-PHY decoder, cache exercises, all health deadlines and inactive-DMA
checks are retained. No new diagnostic tools or captures are requested.

Only install `r5s-gmac-dma-preflight-repair-v2.img`. It performs the cache exercises,
inactive-MAC/DMA checks and native PHY initialization internally. It never starts
DMA, DHCP, RX/TX, raw test services, another profile or an automatic reboot.
Both failed preflight images, known-good images and raw/WAN candidates are preserved.
Raw/WAN promotion stays blocked until this prerequisite and recovery pass.

## Transfer once

From the development checkout, transfer the new archive and checksum into the
existing R5S staging parent. Do not overwrite an older archive/result directory;
if these exact files were already transferred, reuse them after checksum checks.

```sh
scp /workspaces/mounted/projects/r5s_custom/out/preflight-repair-v2/r5s-preflight-repair-v2-handoff.tar.gz /workspaces/mounted/projects/r5s_custom/out/preflight-repair-v2/r5s-preflight-repair-v2-handoff.tar.gz.sha256 root@10.0.1.81:/root/r5s-flash/
```

## One root-run installation block on Ubuntu

This is for the operator later, not a command executed by Codex. Ubuntu must be
running normally from eMMC. Reinsert the SD, keep CWD/scripts/image on eMMC and
run the block once. It verifies/extracts one new `preflight-repair-v2/` archive root,
checks existing dependencies, normally unmounts the old FAT mount, performs a
dry-run and reaches the writer's explicit confirmation. Refusal means stop.
The subshell does not terminate the interactive shell on failure.

```sh
(
  set -eu
  test "$(id -u)" = 0
  cd /root/r5s-flash
  sha256sum -c r5s-preflight-repair-v2-handoff.tar.gz.sha256
  python3 - <<'PY'
import pathlib,tarfile
parent=pathlib.Path('/root/r5s-flash')
if (parent/'preflight-repair-v2').exists():raise SystemExit('Refusing existing preflight-repair-v2 directory')
with tarfile.open(parent/'r5s-preflight-repair-v2-handoff.tar.gz','r:gz') as archive:
    members=archive.getmembers();names=set();total=0
    for member in members:
        path=pathlib.PurePosixPath(member.name)
        if path.is_absolute() or '..' in path.parts or not path.parts or path.parts[0]!='preflight-repair-v2' or not (member.isfile() or member.isdir()) or path.as_posix() in names:
            raise SystemExit('Unsafe archive member')
        names.add(path.as_posix());total+=member.size
    if total>512*1024*1024:raise SystemExit('Unexpected archive size')
    print('Validated archive root and',len(members),'regular members')
    archive.extractall(parent,members=members,filter='data')
PY
  cd /root/r5s-flash/preflight-repair-v2
  sha256sum --quiet -c SHA256SUMS
  python3 scripts/check-preflight-deps
  if mountpoint -q /mnt/r5s-boot; then umount /mnt/r5s-boot; fi
  python3 scripts/flash-sd --device /dev/mmcblk0 --image /root/r5s-flash/preflight-repair-v2/r5s-gmac-dma-preflight-repair-v2.img --profile r5s --expect-model NCard --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488
  python3 scripts/flash-sd --device /dev/mmcblk0 --image /root/r5s-flash/preflight-repair-v2/r5s-gmac-dma-preflight-repair-v2.img --profile r5s --expect-model NCard --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488 --execute
)
```

The second writer invocation is the separate explicit-write operation. Review
the successful dry-run before answering its prompt. Type exactly to authorize:

```text
OVERWRITE /dev/mmcblk0 8903034e43617264100d3aa313016c07 62534975488
```

Cancel instead if anything differs. The full CID is required, not the short
serial. The unchanged writer checks FriendlyElec model/both compatible strings,
hidden eMMC OverlayFS/active-ext4 evidence, source/target dependencies, mounts,
swap, image/capacity/identity and post-confirmation reinspection. It refuses
mmcblk2 and its partitions/boot/RPMB areas. O_RDWR|O_EXCL stays held through the
write, flush and byte-for-byte100663296-byte readback. Wait for “Written, flushed
and byte-compared.” No separate whole-card scan or unguarded dd is required.
Do not force/lazy-unmount, edit fstab, expand/repair GPT or bypass a refusal.

Existing requirements are Python>=3.11, dosfstools, mtools, util-linux,
coreutils, tar and gzip. The dependency check installs nothing. No debugging
package, raw socket privilege or new board tool is needed. If a dependency is
missing, stop and resolve installation separately; the image is not written.

## Boot and observe, then ordinary recovery

After successful readback, cleanly shut down Ubuntu using the usual procedure
(`shutdown -h now` if desired), then use the already-proven physical SD-selection
procedure. No new MASK timing, boot-priority change or eMMC operation is involved.
Keep the usual supported partner/cable connected; it is expected in preflight.

Observe SYS for90seconds from power application. This is a finite observation
window, not a promised boot duration. Look for several complete READY periods,
not an isolated transition. Optional: unplug/replug once to check WAIT then READY.
No video, commands, packet probe or register capture is requested.

| Stable repeating rhythm | Meaning |
|---|---|
| ON2s/OFF2s | WAIT: safe initialization or link pending |
| ON200ms/OFF3800ms | READY: real cache exercises passed, MAC/DMA inactive, valid fresh link and essential health |
| Flutter125ms burst then about4s darkness | FAULT: fatal check or bounded liveness failure |
| Long-short pair | Not legal success for this profile; a partial transition is inconclusive |
| Solid light/darkness | Inconclusive, never success |

The500ms all-OFF transition delimiter and four diagnostics-v2 patterns are
unchanged. IO_VERIFIED is unreachable here. PHY/RJ45 activity is separate from
SYS and does not establish packet I/O. If READY is absent, do not promote to
raw/WAN or repeatedly reflash other diagnostic stages. This remains a candidate
until a new physical observation establishes the result.

Power off, remove experimental SD, normal power-on and confirm ordinary Ubuntu
access at its existing recovery address10.0.1.81. Latest failed-test recovery is
not assumed. Compare only genuine existing pre/post hashes if already available;
no new hash/capture campaign is required. If recovery fails, stop experiments;
do not erase/reinstall eMMC automatically.

Build/audit details: `fault-audit-v2.md`, `source-diff.md`, `status-abi.json`,
`differential.json`, ELF/maps and logs in the handoff. The RAM first-failure record
is not power-persistent and the operator is not asked to retrieve it. Source and
host tests do not establish physical cache visibility or native packet exchange.

Auxiliary case indicators are not included. Existing captured DT declares the
candidate GPIOs but does not establish their physical visibility; this candidate
adds no GPIO2/GPIO3 or shared GRF/CRU authority. If FAULT remains, its family is
unknown from SYS alone. Do not infer an excluded fault from an unlit case LED.

Only the supervisor sampling order and build dependency tracking change.
The kernel, loader, monitor, scheduling, PHY decoder and board setup remain the
same as the first repair. There is no additional no-Ethernet regression image.
