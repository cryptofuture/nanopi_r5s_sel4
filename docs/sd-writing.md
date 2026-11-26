# SD writer

Back up the useful `/mnt/storage` contents to another computer first. Preserve
the existing `/tmp` evidence before reboot. Keep the image, writer, backup and
logs off the chosen card. Unmount every SD filesystem and disable any SD swap
yourself; the writer never unmounts, edits fstab, or selects a device for you.

Inspect identities with `lsblk --bytes --paths -o NAME,TYPE,SIZE,MODEL,SERIAL,TRAN,MOUNTPOINTS`.
For an R5S use profile `r5s`, sysfs `device/name` as model and `device/cid` as
serial. The target must have sysfs type `SD`; MMC is always rejected. For an
external reader use profile `usb` with exact model, serial and byte capacity.
A reader with no serial is refused. USB/removable flags alone are insufficient.

Run the README dry-run command with those real values. `--execute` additionally
requires typing `OVERWRITE <resolved-device> <serial> <bytes>` at the prompt.
Execution destroys the selected card's existing filesystem and data. This tool
has been tested with mock metadata and regular files only, never physical media.

The writer resolves the root/block topology, mounted descendants, swap and image
backing device, then rechecks identity immediately before exclusive device open.
The exact FriendlyElec R5S early-boot OverlayFS layout has a narrow resolver
with verified eMMC identities and an independent read-only kernel target claim;
see [the R5S overlay writer procedure](r5s-overlay-writer.md). Other overlays and
unknown root backing devices are refused.
Keep shell redirections off the card too. It flushes and compares the image-sized
readback, then stops without rebooting. Failure is not permission to boot.

The image is smaller than the card. Its GPT backup is at the end of the image,
not the end of the physical card; do not run a partition editor's automatic GPT
repair during this milestone. Partition 1 is sufficient for U-Boot. Old bytes
outside the written image are not securely erased. Wiping filesystem signatures
alone would not remove raw Rockchip boot code.

The new filesystem and partition IDs differ from Ubuntu and the old storage SD.
The previous UUID-based `/mnt/storage` automount will no longer match. A future
data partition, automount configuration and 1% ext4 reserve are separate opt-in
work; this project does not change `/etc/fstab`.
