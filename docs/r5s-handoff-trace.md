# R5S handoff trace: operator and source guide

The user observed the preserved U-Boot diagnostic's two long SYS flashes about
every four seconds. That confirms `r5sboot` entry and its GPIO/delay path. It
does not establish EL2, payload loading, Microkit, seL4 or Rust execution.
All three new images are hardware-untested. No write or reboot was executed.

## Profiles and visible protocol

| Image | Intentional stop |
|---|---|
| `r5s-preflight-stop.img` | Repeat checkpoint 3, before SD quiescence/CPU teardown/branch |
| `r5s-loader-entry-stop.img` | Real loader repeats checkpoint 4, before UART/architecture initialization |
| `r5s-boot-trace.img` | Emit finite frames, then only Rust emits its unprefixed heartbeat |

Every progress frame: ON 1000 ms, OFF 500 ms, N times ON 150 ms/OFF 250 ms,
then OFF 2000 ms. Stop images repeat their selected frame intentionally. A
checkpoint loop is neither a hang nor evidence of completed kernel startup.
All timings are nominal until measured on the R5S. The long preamble is required
when counting stages; prefixed checkpoint 3 is not the Rust success heartbeat.

| N | Component | Exact emitting location |
|---|---|---|
| 1 | U-Boot | `firmware/r5sboot-trace.c:do_r5sboot`, after five proven GPIO setup writes |
| 2 | U-Boot | Same function, after exact read length and SHA-256 comparison |
| 3 | U-Boot | Same function, after runtime EL/live-memory/FIFO checks, before quiescence/teardown |
| 4 | Microkit loader | Patched `loader/src/loader.c:main`, after `_start` stack setup and relocation, before `uart_init` |
| 5 | Microkit loader | `start_kernel`, after `arch_mmu_enable`, before `arch_jump_to_kernel` |
| 6 | seL4 | Patched `src/arch/arm/kernel/boot.c:try_init_kernel`, after `init_cpu`, before `init_plat` |
| 7 | Microkit monitor | Patched `monitor/src/main.c:main`, before thread naming and monitor receive loop |
| 8 | Rust PD | Generated heartbeat `init`, after monitor ownership acquisition and GPIO direction setup |

The initialiser must have created/resumed the monitor for 7 to run; 7 is a
monitor marker, not an initialiser-entry marker or proof all PDs started. At 6,
CPU/kernel mappings exist, but platform/kernel initialization is incomplete.
At 4, stack/relocation already ran: this is not the loader's first instruction.
Only Rust emits the final three short flashes without a preamble, followed by
a long pause. Earlier components never imitate that signal.

Error frame: ON 1500 ms/OFF 500 ms twice, then numbered ON 150/OFF 250 pulses,
then OFF 2000 ms, repeated. Optional serial text gives details.

| Error | Meaning |
|---|---|
| 1 | Actual runtime EL is not EL2 |
| 2 | Intended SD controller/card not found or initialization/type failed |
| 3 | GPT partition, FAT, file lookup or read failed |
| 4 | Partition/capacity, payload length, short read or load range invalid |
| 5 | SHA-256 differs from the compiled payload expectation |
| 6 | Live memory, FIFO/DMA, interrupt-mask or cache/MMU setup failed |
| 7 | Unexpected loader/helper return, if CPU state remains usable enough to signal |

Normal ordering is retained: after 1, EL2 is checked before SD access. Live
U-Boot reservations are checked next; only then can payload validation reach
2. EL2 is checked again, DMA must be inactive, and then 3 is emitted. No earlier
EL failure can be reported as reaching 2. Preflight 3 stops before changing
SD interrupt state, DAIF or caches. Other detected failures stop and repeat
their error; a hard lockup cannot invent an error signal.

## Source audit and concrete fixes

The original diagnostic compiles out the normal path. The original normal
command's first normal-only condition is `current_el() != 2`, following its
single stage flash. Pinned U-Boot `arch/arm/include/asm/system.h:current_el`
reads CurrentEL and returns `(el >> 2) & 3`, correctly. No EL upgrade is attempted.

Two source-confirmed defects were found:

1. Original generated U-Boot configuration disables both PARTITIONS and
   EFI_PARTITION after its scanning/command features were removed. GPT partition
   selection therefore cannot work if the normal path reaches it. The trace
   config enables the read-only GPT parser and its UUID dependencies, while
   explicitly keeping SPL partition parsing and all storage writes disabled.
   `config-delta.json` lists the six effective additions. This is not proof it
   was the first failure on hardware, because the preceding EL check is untested.
2. Pinned Microkit loader `_start` used `adrp _stack` plus STACK_SIZE without
   `_stack`'s low 12 bits. The preserved normal ELF places `_stack` at
   `0x30005010`; initial SP was `0x30006000`, 16 bytes short of its intended end.
   The isolated patch adds `add x1,x1,:lo12:_stack` before adding STACK_SIZE.
   This fixes the allocation boundary; it does not establish this small offset
   caused the earlier solid LED.

No DDR, BL31, board pinmux/clock or memory-bank replacement was made. All image
builds compare idbloader, SPL and the U-Boot DT against the proven diagnostic.
The package audit extracts and verifies FIT components: only U-Boot proper
changes; BL31 components and DT remain identical. BL32 remains absent, as before.
The newly introduced per-profile loader build also explicitly requests `make
all`, avoiding a default target from included dependency files that left a stale
ELF during development. Compiled stop-path tests caught that packaging defect.

## Actual entry, memory and instrumentation contract

U-Boot identifies controller `0xfe2b0000` by DT register address, verifies SD
type, explicitly selects GPT partition 1 and requires the existing 512-byte
sector geometry (start 32768, size 131072 sectors). FAT `loader.img` must have
the exact profile-specific compiled length and SHA-256, fit the 64 MiB bound,
and load at `0x30000000` within `[0x20000000,0x40000000)`. Expectations are
regenerated from each real payload and checked again by the layout audit.

Before branch, SD DMA must already be inactive under the existing FIFO policy.
DMA is disabled again, SD interrupt mask/pending state cleared, DAIF masked,
data cache flushed, data cache/MMU and instruction cache disabled, barriers
issued, and SCTLR M/C/I read back. `firmware/r5s_enter.S` performs `br x0` into
the raw loader, not Linux `booti`, `bootelf` or a guessed `go` wrapper. A small
return trampoline records original SP in U-Boot BSS and can restore it if the
loader unexpectedly returns and translation/EL remain usable. It cannot recover
arbitrary changed CPU state. That BSS slot is not a persistent crash log.
Pinned RK3568 U-Boot mappings have matching virtual/physical addresses; runtime
reservation checks protect its relocated code, stack, heap and DT from the work
interval. Actual live addresses remain UNKNOWN until execution; optional text
logs them. No UART is needed to observe successful stage 3.

Pinned loader `_start` is at the first raw image bytes and the ELF entry.
The audit compares those bytes to the ELF, fixes/sets its stack, and checks
loader input, BSS/stack/page tables and all copy destinations for overlap.
Microkit's ELF handling materializes BSS in the raw loader and patches page
tables there; blindly zeroing BSS at entry would destroy those tables.
`main` installs exceptions, calls `arch_init` (EL/PSCI and MMU handling), validates
loader metadata and copies the kernel/initial task. `start_kernel` enables
prepared mappings, emits 5, sets TPIDR_EL1 logical CPU 0 and calls kernel entry
with initial-task physical bounds, physical/virtual offset, user entry and zero
DTB pointer/size. It does not consume Linux boot arguments. Secondary CPUs are
not started (MAX_NUM_NODES is 1).

GPIO remains physical `0xfe770000`, GPIO-v2 high-half offsets 4/12 with
write-enable `0x04000000` and active-high data `0x400`. Only U-Boot performs
the proven clock/reset/mux setup. Later stages use the same masked data writes
and barriers, with component-local code from `firmware/r5s_trace.h`:

- Loader 4 runs with U-Boot's MMU/cache teardown complete. At 5, the actual
  generated 1 GiB identity device block covers UART and GPIO with Device nGnRnE;
  this descriptor is checked in the raw payload.
- The kernel adds one non-owning Device nGnRnE alias at `KDEV_BASE+0x1f0000`,
  checked against generated device slots. It is deliberately not added to the
  reserved device list, so the same physical GPIO frame remains available for
  the PD. This is an explicit debug-only structural change, not verification.
- The patched CapDL builder maps the single GPIO frame object uncached into
  both monitor and PD at `0x40000000`. There is no duplicate retype/allocation.
  A separate cached RAM page at `0x40001000` carries a release/acquire owner
  flag: monitor finishes 7, sets the flag, then never blinks again; the PD waits
  before touching GPIO. Preemption cannot create two concurrent LED owners.

All post-U-Boot timing uses CNTVCT with the pinned 24 MHz frequency. The traced
kernel explicitly enables `KernelArmExportVCNTUser`, whose pinned
`user_access.c` sets EL0VCTEN. The SDK's pre-existing physical-counter export
also remains enabled. Both timer-control exports stay disabled; no scheduler
timer is reprogrammed. U-Boot uses its proven `mdelay`. Monitor and PD have
1000-us full budgets/periods; elapsed-counter delays tolerate preemption.
Rust's previous uncalibrated loops are replaced only in the traced PD by
counter delays. Exporting the counter adds a timing channel in this debug build.
No generic Microkit sleep function or unrestricted EL0 counter access is assumed.

`layout-report.json` contains actual ELF/copy intervals, page-table symbols,
CapDL frame/stack allocations and the conditional runtime U-Boot guards. It
preserves all existing RAM exclusions, including the OP-TEE gap and CMA range.
No complete captured runtime DTB is present, so its flattened reservation table
remains explicitly MISSING. Firmware ownership of other RAM is not proven.
Nothing writes runtime records to SD, eMMC, SPI or a presumed persistent RAM slot.

## Builds and host checks

Use the existing project-local tools and locked sources; do not upgrade pins:

```sh
. scripts/env
CARGO_NET_OFFLINE=true python scripts/build-handoff-trace
python -m unittest discover -s tests -v
python scripts/package-handoff-trace
```

Initial output images must not already exist. Preserve any earlier trace run
before rebuilding. The isolated `work/` cache checks its SDK/PD instrumentation
fingerprint and refuses incompatible inputs; use a fresh output directory for
such changes. Each final assembly is built twice from the same pinned compiled
inputs and hashes are compared. This does not claim byte reproducibility of
all debug ELFs across different absolute checkout paths/toolchain hosts.

For a later narrowed interval, build e.g.
`python scripts/build-handoff-trace --stop-at-checkpoint 6`, then
`python scripts/package-handoff-trace --directory out/handoff-trace/stop-6`.
That creates a separate `handoff-trace-stop-6` bundle directory; no existing
diagnostic or normal image is overwritten. Any checkpoint 1-8 can be selected
at build time. Stop 6 loops in the kernel, stop 7 holds monitor ownership, and
stop 8 loops in Rust. An intentional stop is never presented as final success.

## R5S transfer and dependencies

No automatic remote access, write, shutdown or reboot is performed. On the
development computer, set the already known SSH address:

```sh
R5S_HOST='YOUR_EXISTING_R5S_SSH_ADDRESS'
scp out/handoff-trace/r5s-handoff-trace-bundle.tar.gz \
    out/handoff-trace/r5s-handoff-trace-bundle.tar.gz.sha256 \
    "root@$R5S_HOST:/root/r5s-flash/"
```

The R5S requires native Python 3.11+ (Ubuntu's Python 3.12 is suitable),
dosfstools, mtools, util-linux, mount, coreutils, tar and gzip. Check first:

```sh
command -v python3 fsck.fat mcopy mdir mtype blkid lsblk findmnt swapon mountpoint \
  blockdev stat cmp sha256sum tar gzip mount umount shutdown
```

If packages are missing, the operator may separately choose to install them:

```sh
apt-get update
apt-get install --no-install-recommends python3 dosfstools mtools util-linux mount coreutils tar gzip
```

These installation commands were not executed on the R5S. No compiler, Rust,
UART adapter, network boot service or PC card reader is needed to write/boot.
Network access here is only for transfer or optional dependency installation.

On R5S Ubuntu as root, in Bash, establish that this is the recovered Ubuntu
session and preserve any genuine existing pre-test boot-region hashes. Then:

```sh
set -euo pipefail
cd /root/r5s-flash
sha256sum -c r5s-handoff-trace-bundle.tar.gz.sha256
test ! -e handoff-trace
tar -xzf r5s-handoff-trace-bundle.tar.gz
cd handoff-trace
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

Do not force/lazy unmount. Any failed safety check stops the procedure. The
bundle carries the current guarded writer with the measured `FriendlyElec
NanoPi R5S` model and both compatible strings, hidden-overlay eMMC/UUID/ext4
evidence, restricted topology, source/swap/mount checks and read-only kernel
claim. It does not substitute the previously broken model check or a bypass.

Begin with this exact preflight dry run:

```sh
python3 scripts/flash-sd --device /dev/mmcblk0 --profile r5s \
  --image r5s-preflight-stop.img --expect-model NCard \
  --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488
```

Only after reviewing a successful plan, explicitly choose to overwrite the SD:

```sh
python3 scripts/flash-sd --device /dev/mmcblk0 --profile r5s \
  --image r5s-preflight-stop.img --expect-model NCard \
  --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488 --execute
cmp --bytes="$(stat -c '%s' r5s-preflight-stop.img)" r5s-preflight-stop.img /dev/mmcblk0
```

Type the displayed exact `OVERWRITE /dev/mmcblk0 <full-CID> 62534975488`
confirmation. The write descriptor itself retains O_EXCL throughout writing,
flush, block-cache invalidation and image-length readback. A read-only probe is
not treated as a substitute for that protected write descriptor. Neither image,
writer, working directory nor logs may reside on SD.

After successful comparison, the operator may run `shutdown -h now`, wait for
Ubuntu to halt, then power off. Use exactly the physical SD-selection procedure
that produced the proven two-long-flash diagnostic. No new MASK timing, persistent
priority change or eMMC erasure is proposed. Record SYS for the first 60 seconds
from power application, preferably on video. This is a finite observation
window, not a boot-time guarantee.

Then power off, remove SD, release any selection button and power on normally.
Confirm Ubuntu SSH. Compare genuine available pre/post immutable boot hashes;
a new baseline cannot prove preservation during earlier boots. Post-diagnostic
recovery and bytewise eMMC preservation have not been supplied as results.

## Conditional next images

If preflight repeats checkpoint 3 and recovery succeeds, stage/usage/checksum
checks above still apply. Next run these literal loader-entry commands:

```sh
python3 scripts/flash-sd --device /dev/mmcblk0 --profile r5s \
  --image r5s-loader-entry-stop.img --expect-model NCard \
  --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488
python3 scripts/flash-sd --device /dev/mmcblk0 --profile r5s \
  --image r5s-loader-entry-stop.img --expect-model NCard \
  --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488 --execute
cmp --bytes="$(stat -c '%s' r5s-loader-entry-stop.img)" r5s-loader-entry-stop.img /dev/mmcblk0
```

Only if the actual loader repeats 4, repeat the controlled shutdown/test/recovery
procedure using boot-trace next:

```sh
python3 scripts/flash-sd --device /dev/mmcblk0 --profile r5s \
  --image r5s-boot-trace.img --expect-model NCard \
  --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488
python3 scripts/flash-sd --device /dev/mmcblk0 --profile r5s \
  --image r5s-boot-trace.img --expect-model NCard \
  --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488 --execute
cmp --bytes="$(stat -c '%s' r5s-boot-trace.img)" r5s-boot-trace.img /dev/mmcblk0
```

Do not execute all variants in advance. An error frame means fix its named check
before handoff testing. A later final frame without Rust success narrows an
interval; it does not identify the exact failing instruction or blame the next
component. Use a stop-at-checkpoint build if needed. No progress means compare
the preserved known-good diagnostic and written artifact identity before any
early-firmware changes. Rust's unprefixed heartbeat is the only proposed native
success signal, and it still needs a physical observation and recovery test.

If Ubuntu fails after card removal, stop experiments. No automatic erase,
reinstall, firmware recovery or repeated reboot loop is provided. Do not repair
or expand the small image's GPT to fill the 64 GB card during diagnosis. The
experimental runtime performs no SD/eMMC writes.
