# One replacement DMA-disabled preflight

This diagnostic-only image retains the failing second repair's real cache and
PHY workload. On failure, SYS emits a stable short/long identifier. Normal WAIT
and READY retain their existing meanings. DMA, packet I/O and DHCP remain disabled.
All three previous preflight variants produced user-observed FAULT; none is
promoted as successful. This image is hardware-untested.

No new inventory, commands for debugging, app, video, UART or packet tool is
required. Install this one image, observe its word or stable READY, and use the
usual card-removal Ubuntu recovery. Raw/WAN promotion remains blocked.

## Transfer once

From the development checkout, transfer the new archive and checksum into the
existing R5S staging parent. Do not overwrite an older archive/result directory;
if these exact files were already transferred, reuse them after checksum checks.

```sh
scp /workspaces/mounted/projects/r5s_custom/out/preflight-fault-id/r5s-preflight-fault-id-handoff.tar.gz /workspaces/mounted/projects/r5s_custom/out/preflight-fault-id/r5s-preflight-fault-id-handoff.tar.gz.sha256 root@10.0.1.81:/root/r5s-flash/
```

## One root-run installation block on Ubuntu

This is for the operator later, not a command executed by Codex. Ubuntu must be
running normally from eMMC. Reinsert the SD, keep CWD/scripts/image on eMMC and
run the block once. It verifies/extracts one new `preflight-fault-id/` archive root,
checks existing dependencies, normally unmounts the old FAT mount, performs a
dry-run and reaches the writer's explicit confirmation. Refusal means stop.
The subshell does not terminate the interactive shell on failure.

```sh
(
  set -eu
  test "$(id -u)" = 0
  cd /root/r5s-flash
  sha256sum -c r5s-preflight-fault-id-handoff.tar.gz.sha256
  python3 - <<'PY'
import pathlib,tarfile
parent=pathlib.Path('/root/r5s-flash')
if (parent/'preflight-fault-id').exists():raise SystemExit('Refusing existing preflight-fault-id directory')
with tarfile.open(parent/'r5s-preflight-fault-id-handoff.tar.gz','r:gz') as archive:
    members=archive.getmembers();names=set();total=0
    for member in members:
        path=pathlib.PurePosixPath(member.name)
        if path.is_absolute() or '..' in path.parts or not path.parts or path.parts[0]!='preflight-fault-id' or not (member.isfile() or member.isdir()) or path.as_posix() in names:
            raise SystemExit('Unsafe archive member')
        names.add(path.as_posix());total+=member.size
    if total>512*1024*1024:raise SystemExit('Unexpected archive size')
    print('Validated archive root and',len(members),'regular members')
    archive.extractall(parent,members=members,filter='data')
PY
  cd /root/r5s-flash/preflight-fault-id
  sha256sum --quiet -c SHA256SUMS
  python3 scripts/check-preflight-deps
  if mountpoint -q /mnt/r5s-boot; then umount /mnt/r5s-boot; fi
  python3 scripts/flash-sd --device /dev/mmcblk0 --image /root/r5s-flash/preflight-fault-id/r5s-preflight-fault-id.img --profile r5s --expect-model NCard --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488
  python3 scripts/flash-sd --device /dev/mmcblk0 --image /root/r5s-flash/preflight-fault-id/r5s-preflight-fault-id.img --profile r5s --expect-model NCard --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488 --execute
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

Observe for up to 180 seconds from power application. This is an observation
window, not a promised boot time. READY is the isolated 200ms tick every four
seconds. WAIT is two seconds ON/two seconds OFF; an absent/pending link is WAIT.
Optional: unplug/replug once to check WAIT then READY.

On failure, read the **duration** of each symbol:

- S: ON 0.4 seconds, then OFF 2.6 seconds.
- L: ON 2 seconds, then OFF 1 second.

A full frame is: existing rapid flutter/dark FAULT prefix (6 seconds), four
symbols, an extra 4 seconds dark, four symbols, an extra 8 seconds dark.
The same 42-second frame repeats. A one-time 500ms OFF delimiter precedes it.
Write the two groups as `SSLS / LSSL` (illustration only). Observe the same complete
word twice, then use the included fault-codes.md or report that literal word.
No binary conversion or software decoder is needed. Missing/uncertain symbols,
unequal repetitions or a word absent from the table are inconclusive. Parity
catches single-symbol errors, not every multi-symbol mistake.

Only stable READY means real preflight checks passed with a fresh supported
link. It does not prove DMA visibility, packets or Internet access. IO_VERIFIED
is forbidden. A persistent generic flutter without a slow word means reporting
is incomplete; static light/darkness also has no success meaning. An early boot,
supervisor or kernel failure can prevent the reporter from running. Do not guess
a cause from the last operation or blame a connected cable.

After observation, power off, remove SD and boot Ubuntu normally. Confirm ordinary
access at its existing recovery address 10.0.1.81. No latest recovery or immutable
eMMC comparison is assumed. If recovery fails, stop experiments; do not erase or
reinstall Ubuntu automatically. No new capture/hash campaign is requested.

Registry hash and image/build identity are in the manifest and SHA256SUMS.
Use only this bundle's table. Source mapping and limits are in failure-path-audit.md.
All records are volatile; the operator is not asked to retrieve them. Auxiliary
case LEDs are unused. No repeated profile flashing or automated reboot occurs.
