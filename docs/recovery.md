# Independent recovery contract

Before the experiment, demonstrate Ubuntu boots with no SD, release MASK, and
record its identity/version. Preserve a quiesced restorable eMMC backup on a
different computer. A live whole-disk dump is not necessarily filesystem
consistent. An offline external recovery environment must leave eMMC unmounted
while imaging it; do not use this experimental seL4 card as that environment.
No offline backup or restoration has been performed by this project.

Preserve read-only GPT and immutable eMMC boot-region snapshots and SHA-256
checksums before and after each test. Identify eMMC by sysfs type MMC and its
CID, not a fixed Linux number. Include its raw loader area, Ubuntu's uboot
partition, and boot0/boot1 if accessible read-only. Do not toggle force_ro,
open RPMB, or run firmware/environment tools. Snapshot the primary and backup
GPT and record exact byte ranges so the comparison repeats the same reads.
Those snapshots supplement, rather than replace, the full OS backup. A running
Ubuntu root/data filesystem is expected to change and is not a bytewise
immutable comparison target.

If the experiment hangs or shows no expected pattern within 60 seconds:

1. Disconnect power completely.
2. Remove experimental SD and release MASK.
3. Power on normally and verify Ubuntu, then compare immutable boot-region hashes.

This procedure is required after both normal and deliberate failure tests. It
must also cover a broken SD first-stage loader that cannot light SYS. It has not
been validated on the user's board. If Ubuntu does not return, stop testing;
do not erase eMMC as a workaround.

## Last resort USB detection

Follow the [R5S manufacturer's connection diagram](https://wiki.friendlyelec.com/wiki/index.php/NanoPi_R5S#Unbricking_Method)
for the specific USB Type-A data port and USB A-to-A cable. The USB-C connector
is power. Use the right-hand Type-A socket when viewing the pair as shown in
the [manufacturer's cable diagram](https://wiki.friendlyelec.com/wiki/images/a/a8/Nanopir5s-usba-to-usba2.jpg).
Confirm that orientation against the actual board before connecting.
Remove SD/peripherals, hold physical MASK while applying power,
and release after four seconds. The manufacturer's Linux detection-only command
is `upgrade_tool LD`; it should report Maskrom mode. No recovery tool is bundled
or invoked by project scripts.

If a recovery specialist needs a loader, obtain the manufacturer's RK3568
`MiniLoaderAll.bin`, verify provenance/hash and board compatibility first.
Detection is separate from downloading that loader, erasing or reinstalling.
The manufacturer's destructive steps are outside this preservation workflow;
they are intentionally not included as commands here. Reinstalling Ubuntu does
not satisfy preservation of the existing installation.
