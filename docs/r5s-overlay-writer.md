# Guarded writer on the R5S early-boot OverlayFS root

The writer now recognizes the reported FriendlyElec layout without mounting,
remounting, changing namespaces or changing Ubuntu. This is a narrow R5S
resolver, not a general OverlayFS bypass. Dry-run remains the default.

The displayed lower/upper/work paths belong to the early-boot namespace. Looking
up `/root` or `/data/root` again in the current root could identify different
objects. Kernel command-line strings alone also do not prove current backing
mount identity. The resolver therefore requires all of the following:

- Runtime board model `FriendlyElec NanoPi R5S`, exact NUL-separated compatible
  entries `friendlyelec,nanopi-r5s` and `rockchip,rk3568`, and the same mount
  namespace as PID 1.
- Exactly one overlay mount, at `/`, with one lower `/root`, upper `/data/root`
  and work `/data/work`. Additional overlays are refused.
- Exactly one `root=/dev/mmcblkNp8`, `data=/dev/mmcblkNp9` and
  `storagemedia=emmc`; conflicting/duplicate arguments are refused. The two
  partitions must belong to the same actual MMC-type device with a valid CID.
- Block node, sysfs partition/parent and major:minor identities must agree with
  the enumerated block graph. Both filesystems must be ext4, have distinct,
  nonzero unique UUIDs, and agree between `lsblk` and an uncached read-only
  `blkid -p` probe. Both must have active kernel ext4 registrations under
  `/sys/fs/ext4/`, even though their mounts are hidden.
- Active block topology must contain only that eMMC, the selected SD, their
  partitions and eMMC boot areas. Additional disks, mapped devices and raw
  block-backed loops are refused by this narrow resolver.
- All visible mounts, image/writer/working-directory/log backing paths and swap
  dependencies must resolve away from the selected SD. Unknown dependencies
  remain fatal. RAM-backed staging is allowed when identified as tmpfs/ramfs.
- After target identity/capacity checks, a read-only
  `O_RDONLY | O_EXCL | O_NOFOLLOW` open must successfully claim the entire SD.
  The descriptor's block identity must match the selected device. A busy device
  or failed claim is fatal, including in dry-run.

The final condition is independent of the boot arguments. Linux filesystem
mounts retain block-device claims when hidden by mount detachment or another
namespace. Claiming the whole SD conflicts with a claimed child partition.
Thus stale eMMC boot arguments cannot authorize overwriting an SD still used by
a hidden block-backed overlay. Raw loop/mapped topologies are rejected rather
than assuming their non-exclusive raw backing opens are covered by that check.
See the primary Linux 6.1 implementations of
[`bd_may_claim` and `bd_finish_claiming`](https://github.com/torvalds/linux/blob/v6.1/block/bdev.c)
and [block-device file open](https://github.com/torvalds/linux/blob/v6.1/block/fops.c).

The plan records the validated boot-configured eMMC devices/CID/UUIDs and the
successful kernel target check. It does not claim to recover the exact detached
mount objects from their old path strings. The exclusive claim proves target
non-use for the supported block-backed layout; missing evidence never becomes
permission to write. As with the existing writer, this trusts the running kernel
and excludes concurrent privileged raw-I/O users deliberately bypassing claims.

Execution repeats inspection after typed confirmation, compares identities and
backing evidence, and opens the target exclusively again before any writes.
Image verification, eMMC/boot/RPMB refusal, mounted-target refusal, capacity,
source-on-target refusal, flushing and bounded readback comparison remain active.

## Install the writer update on the R5S

The update archive contains writer/validator Python files, this guide and a new
checksum list. It contains no disk image and changes no boot files. The existing
diagnostic image must remain at `/root/r5s-flash/diag-uboot/r5s-uboot-diag.img`.
If currently elsewhere, stage a regular-file copy there on eMMC first.

On the development computer, substitute the existing SSH address:

```sh
R5S_HOST='YOUR_EXISTING_R5S_SSH_ADDRESS'
scp out/overlay-writer-update/r5s-overlay-writer-update.tar.gz \
    out/overlay-writer-update/r5s-overlay-writer-update.tar.gz.sha256 \
    "root@$R5S_HOST:/root/r5s-flash/"
```

On the R5S, as root, in Bash:

```sh
set -euo pipefail
cd /root/r5s-flash
sha256sum -c r5s-overlay-writer-update.tar.gz.sha256
cd diag-uboot
tar -xzf ../r5s-overlay-writer-update.tar.gz
sha256sum -c OVERLAY-WRITER-SHA256SUMS
command -v python3 blkid lsblk findmnt swapon mcopy mdir mtype fsck.fat cmp
test "$(cat /sys/class/block/mmcblk0/device/type)" = SD
test "$(cat /sys/class/block/mmcblk0/device/serial)" = 0x0d3aa313
card_cid=$(cat /sys/class/block/mmcblk0/device/cid)
```

The new checksum list covers the existing diagnostic image and current scripts.
The old archive's checksum list still describes its old writer and is no longer
the right list for the updated script. The image's required SHA-256 is unchanged:
`7c231a8b7680e06fb8e0c48433f76fb68d9e047ada839fe07b1782bdfac93769`.

The existing `--expect-serial` R5S interface expects the full CID, not the shorter
`0x0d3aa313` serial. The explicit serial check above identifies the reported
NCard, while `card_cid` binds the writer's existing confirmation to its full CID.
Model and capacity remain explicit rather than accepting whichever card appears.

The SD is reported unmounted. Keep it unmounted; if `/mnt/r5s-boot` has been
mounted again, unmount it normally before running the writer. Do not force
unmount, suppress a failure, disable checks or edit fstab. Keep logs, image and
scripts on eMMC or RAM, never on SD.

Exact dry-run command:

```sh
python3 scripts/flash-sd --device /dev/mmcblk0 --profile r5s \
  --image r5s-uboot-diag.img --expect-model NCard \
  --expect-serial "$card_cid" --expect-bytes 62534975488
```

Review the returned root/data devices, MMC CID/UUIDs, SD CID/model/capacity and
`overlay_target_check`. A refusal means stop. No write or reboot was performed
by the development task. Only when separately choosing to overwrite the SD:

```sh
python3 scripts/flash-sd --device /dev/mmcblk0 --profile r5s \
  --image r5s-uboot-diag.img --expect-model NCard \
  --expect-serial "$card_cid" --expect-bytes 62534975488 --execute
cmp --bytes="$(stat -c '%s' r5s-uboot-diag.img)" r5s-uboot-diag.img /dev/mmcblk0
```

Execution additionally requires typing the exact
`OVERWRITE /dev/mmcblk0 <full-CID> 62534975488` shown by the writer. A changed
device, changed backing evidence or failed readback stops the process. Do not
expand/repair GPT. `/dev/mmcblk2`, boot0/boot1/RPMB, SPI and Ubuntu partitions
must never be selected. Existing diagnostic boot/recovery instructions apply;
this update authorizes no physical write or reboot.

## If proof is unavailable

Absent ext4 registration, mismatched UUID, missing device, unknown topology,
extra overlay or failed exclusive claim retains refusal. The program does not
try a second namespace, remount hidden filesystems or trust the boot arguments
alone. A busy SD may genuinely back a hidden filesystem despite being absent
from visible mount output.

The supported alternative on the same R5S is the existing writer's direct
block-backed ext4 eMMC-root path, from an independently provisioned maintenance
boot where root and source dependencies are visible and SD is unused. This
task does not provision that environment or change this Ubuntu installation to
obtain it. If such an environment is unavailable and this resolver cannot prove
target non-use, flashing remains blocked. No PC card reader is required by the
new resolver, and no unguarded-write fallback is supplied.

Validation uses mocked device/sysfs/kernel-claim metadata and regular image
files. It does not establish that this vendor kernel or this R5S has passed the
new checks; the user-run dry-run must establish that before any later write.
