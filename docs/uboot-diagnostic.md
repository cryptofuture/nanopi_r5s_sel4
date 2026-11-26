# R5S U-Boot command/LED diagnostic

Built and statically validated; this diagnostic has not run on hardware. It
cannot establish seL4 execution, EL2 correctness, Microkit memory safety or Rust
startup. The single hardware success target is two long SYS flashes per four
seconds: initially OFF 1000 ms, then ON 750, OFF 250, ON 750, OFF 2250, forever.
These are nominal U-Boot timer delays until physically measured.

The user's previous normal-image attempt showed solid red and no Rust pattern.
Removing SD and power-cycling restored Ubuntu and SSH. That establishes observed
card-removal recovery for that test, not bytewise eMMC preservation. Matching
first-16-MiB hashes establish those written bytes, not command execution. Keep
any existing eMMC hashes; a new baseline only applies to subsequent tests.

## Build and changes

From the project root on the existing development host:

```sh
. scripts/env
python scripts/build-diag
python -m unittest discover -s tests -v
python scripts/verify-image out/diag-uboot/r5s-uboot-diag.img
```

The builder refuses an existing output. Use a fresh filename within
`out/diag-uboot` for another run, for example
`python scripts/build-diag --output out/diag-uboot/reproduced.img`.
`--package-only` re-audits the existing diagnostic build before packaging it.
No build command opens a physical device or rebuilds the normal image.

The only firmware behavior change is the `R5S_UBOOT_DIAGNOSTIC` define for
`cmd/r5sboot.o`. The registered name and automatic entry remain `r5sboot`.
The diagnostic branch is first, before normal declarations/checks; its infinite
loop has no fallthrough. The normal EL2, MMC, FAT, hash and handoff code is
preprocessed out. `loader.img` is omitted. FAT contains only the diagnostic
`boot.txt` and `manifest.json`. The normal validator's payload requirements and
negative-image checks remain in the separate Microkit profile.

`out/diag-uboot/audit.json` records comparisons with the preserved normal build.
DDR/idbloader, SPL, SPL DT, full U-Boot DT, configuration and all six BL31 FIT
components remain byte-identical. BL32/OP-TEE remains absent, as in the normal
image. U-Boot proper, its FIT hash and consequently the raw firmware/image hash
change. GPT/FAT geometry stays 96 MiB total, raw firmware at sector 64, FIT at
8 MiB, FAT16 at 16 MiB with length 64 MiB. Diagnostic IDs and manifest differ.
The original `sources.lock` is immutable; `u-boot.patch`, `firmware-source/` and
the manifest record diagnostic source/configuration hashes separately.

## Command-entry audit and limits

The source path is `common/board_r.c:init_sequence_r` (normal relocated driver,
MMC and environment initialization) to `run_main_loop`, then
`common/main.c:main_loop`, `common/autoboot.c:bootdelay_process/autoboot_command`,
and `run_command_list` through the command parser/table to `do_r5sboot`.
`env/nowhere.c` imports the compiled default environment. The generated config
and ELF environment contain `bootcmd=r5sboot`, `bootdelay=-2`. No persistent
environment backend, preboot command, environment-from-DT setup, boot counter,
distro/standard boot or button command is enabled. The generated DT has no
`/config` boot-command/delay override. An interactive `env import` command is
compiled but is not invoked on this automatic path.

The audit decodes the linked command-table entry, checks its handler address,
extracts the default environment, retains object/linked disassembly, extracts
each FIT component and checks its advertised SHA-256. FIT's U-Boot data must
match the new `u-boot-nodtb.bin`; that FIT must match the raw packed image. This
proves registration/packaging, not that hardware reached the command.
The diagnostic command object's only external functions are `udelay` and the
nonrepeatable-command wrapper. There are no command storage calls or payload
dependencies. Earlier U-Boot MMC initialization can still initialize the SD;
the generated eMMC node is disabled and its SDHCI driver/write support is off.
No assertion is made about BootROM probing before SPL.

No definite earlier-stage defect has been established. The prior solid red
remains compatible with inherited LED state, normal refusal, a stall before
command entry, GPIO failure, or selection/firmware issues. No EL-dependent
encoding, fallback or speculative DDR/BL31 fix is included.

## GPIO and timing audit

The command runs after U-Boot relocation/initialization. GPIO4, pinctrl and
`sys-led-pin` survive in the final full DT; GPIO4 is pruned from SPL's DT, where
this command does not run. The final pin tuple is bank 4, pin 26, mux 0; DT
phandles are resolved in that tree and never copied from Linux's capture.
Pinned `drivers/gpio/rk_gpio.c` has GPIO-v2 write/direction support, but its
probe does not enable clocks or release resets. To preserve this first
diagnostic's existing initialization, `led_setup` isolates the same five raw
writes used by the normal command, followed by the tested mask writes:

| Address | Write | Meaning and source |
|---|---|---|
| 0xfdd2037c | 0x03000000 | CRU CLKGATE_CON31 bits 8/9 enable GPIO4 clocks; captured upstream `evidence/clk-rk3568.c` |
| 0xfdd20458 | 0x00030000 | CRU SOFTRST_CON22 bits 0/1 deassert reset IDs 352/353; pinned `rk3568-cru.h` |
| 0xfdc60078 | 0x0f000000 | GRF GPIO4D low mux, D2 nibble bits 11:8 select GPIO; pinned pinctrl core and RK3568 driver |
| 0xfe770004 | 0x04000000 / 0x04000400 | GPIO-v2 high-half data, bit 10 value and bit 26 write-enable |
| 0xfe77000c | 0x04000400 | GPIO-v2 high-half direction, pin 26 output |

CRU offsets follow pinned `cru_rk3568.h` (gates 0x300, resets 0x400).
Pinctrl's PMU bank 0 does not advance GRF offsets; banks 1-3 consume 0x60,
then bank 4 D-low is at 0x78. Writes affect only these masked fields; GPIO
active-high data is initially cleared before direction is set. `writel` supplies
U-Boot I/O ordering, and `led` uses `dsb`. No live Linux register access was used.

Pinned `include/linux/delay.h:mdelay` calls `udelay`; `lib/time.c` uses ticks and
`arch/arm/cpu/armv8/generic_timer.c` reads CNTFRQ_EL0/CNTPCT_EL0. The R5S config
specifies 24 MHz. This uses U-Boot's existing timer path, not an instruction
count loop. Watchdog/cyclic processing is disabled. Counter/clock behavior and
electrical GPIO operation remain hardware assumptions. No pattern can still
mean either pre-command failure or GPIO/timing failure.

## Transfer to the running R5S, no PC card reader

No hardware write/reboot is authorized by the development task. The commands
below are for the operator to run later with physical access. Keep the staging
directory, writer, image and logs on eMMC, never the SD.

On the development computer, set `R5S_HOST` to the existing SSH address:

```sh
R5S_HOST='YOUR_EXISTING_R5S_SSH_ADDRESS'
ssh "root@$R5S_HOST" 'mkdir -p /root/r5s-flash'
scp out/diag-uboot/r5s-diag-handoff.tar.gz "root@$R5S_HOST:/root/r5s-flash/"
scp out/diag-uboot/r5s-diag-handoff.tar.gz.sha256 "root@$R5S_HOST:/root/r5s-flash/"
```

On R5S Ubuntu, as root, use Bash. These steps stage/check files and do not write SD:

```sh
set -euo pipefail
cd /root/r5s-flash
sha256sum -c r5s-diag-handoff.tar.gz.sha256
tar -xzf r5s-diag-handoff.tar.gz
cd diag-uboot
sha256sum -c SHA256SUMS
printf '%s  %s\n' \
  c9ba7c07b801bb1d0beaa7cd4fdfe56f941435b723aad3477e9f90df03406304 \
  /root/r5s-flash/r5s-microkit.img | sha256sum -c -
command -v python3 mcopy mdir mtype fsck.fat lsblk findmnt swapon cmp
test "$(cat /sys/class/block/mmcblk0/device/type)" = SD
test "$(cat /sys/class/block/mmcblk2/device/type)" = MMC
findmnt --target .
findmnt --target /
findmnt -rn -o TARGET,SOURCE,FSTYPE,OPTIONS
lsblk --bytes --paths -o NAME,TYPE,SIZE,MODEL,SERIAL,MOUNTPOINTS /dev/mmcblk0 /dev/mmcblk2
swapon --show
```

`mcopy`, `mdir`, `mtype` are from mtools and `fsck.fat` from dosfstools. If
missing, stop. Installing those host packages is a separate operator action.
The transferred validator/writer use Python 3.11+ and system tools, not an
x86-64 compiler or development PC. Current Ubuntu 24.04's Python 3.12 suffices.

Back up remaining SD data if needed. Unmount the current read-only FAT mount
without force. The writer refuses other mounted descendants or SD swap and unknown
root chains. For the reported FriendlyElec hidden OverlayFS root, install the
[narrow overlay writer update](r5s-overlay-writer.md) and use that guide's exact
commands. Other overlay layouts remain unsupported. If it refuses, stop and resolve the stated
dependency; do not bypass its check, edit fstab or switch to unguarded writes.

```sh
umount /mnt/r5s-boot
sd_model=$(cat /sys/class/block/mmcblk0/device/name)
sd_cid=$(cat /sys/class/block/mmcblk0/device/cid)
sd_bytes=$(blockdev --getsize64 /dev/mmcblk0)
image_bytes=$(stat -c '%s' r5s-uboot-diag.img)
python3 scripts/flash-sd --device /dev/mmcblk0 --profile r5s \
  --image r5s-uboot-diag.img --expect-model "$sd_model" \
  --expect-serial "$sd_cid" --expect-bytes "$sd_bytes"
```

The last command is dry-run. Verify that the plan selects the SD CID/type and
capacity, while Ubuntu remains on `/dev/mmcblk2`. `/dev/mmcblk2`, boot0, boot1,
RPMB, SPI and Ubuntu partitions must never be selected.

Only when separately choosing to perform the physical test, run the explicit
write below. It destroys the selected experimental SD contents, rechecks device
identity and requires the typed `OVERWRITE /dev/mmcblk0 <CID> <bytes>` confirmation.

```sh
python3 scripts/flash-sd --device /dev/mmcblk0 --profile r5s \
  --image r5s-uboot-diag.img --expect-model "$sd_model" \
  --expect-serial "$sd_cid" --expect-bytes "$sd_bytes" --execute
cmp --bytes="$image_bytes" r5s-uboot-diag.img /dev/mmcblk0
```

The writer flushes, invalidates the block cache and compares exactly the image
length. `cmp` independently compares the same bounded length. Any failure means
stop, not permission to boot. Do not expand/repair GPT or remount the SD for this
test. No whole-card string scans or normal-image reflash are required.

## One controlled boot and recovery

Arrange physical access, then power off. Insert SD if removed. FriendlyElec's
[R5S/Rockchip boot-priority instructions](https://wiki.friendlyelec.com/wiki/index.php/Template:RockchipBootPriority)
support holding the physical MASK key while applying power and releasing after
about three seconds. Use that documented non-destructive option, not the page's
eMMC-erasure option. The prior test does not establish that ordinary power-on
selected this particular U-Boot. No soldering, UART or network boot is needed.

Record SYS behavior for 60 seconds from power application. This is an observation
window, not a guaranteed boot time. Make one attempt; do not repeat normal-image
boot tests or enter an automatic reset loop. Then power off, remove experimental
SD, release MASK, and power on normally. Verify Ubuntu SSH. Compare any available
pre/post immutable boot-region hashes, without treating a new baseline as proof
about earlier boots. If Ubuntu fails, stop; do not erase/reinstall automatically.

| Physical observation | Supported conclusion | Next step |
|---|---|---|
| Two-long-flash pattern repeats | Diagnostic command and its LED/delay path are executing | Instrument normal payload validation and handoff in a later build |
| Solid red, no diagnostic pattern | No proof that command entry or GPIO setup succeeded | Review earlier stages and LED initialization separately |
| SYS stays off | No observed diagnostic success; location remains unknown | Review power/early boot/LED path without treating it as a specific kernel fault |
| Ubuntu boots with SD inserted | Experimental diagnostic did not become the final running system | Investigate selection/fallback with the existing firmware evidence |
| Ubuntu boots after SD removal | Independent recovery worked for this test | Record the result; compare available boot-region hashes |
| Ubuntu fails after SD removal | Recovery regression | Stop tests; do not erase or reinstall eMMC as an automatic remedy |
