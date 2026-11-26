# Boot selection

The existing eflasher card previously lost to eMMC. Repeated normal resets are
not a selection procedure. Do not erase eMMC or change its persistent priority.

FriendlyElec describes loader-dependent priority and a physical MASK-at-power-on
method: [manufacturer boot priority](https://wiki.friendlyelec.com/wiki/index.php/Template:RockchipBootPriority).
For the authorized hardware test, power off, insert the experimental SD, hold
the physical MASK button, apply power, and release after about three seconds.
This is a candidate procedure for this particular board, not a validated result.
GPIO4/0 Linux K1 is not used as a substitute for the physical MASK function.
If the SD indication does not appear within the finite test window, power off
and perform card-removal recovery. Stop there; do not enter a reset loop.

Upstream U-Boot's R5S definition is used for DDR/board wiring, not ROCK 3B's.
Its RK356x U-Boot DTS overrides MMC aliases. The custom command resolves the
controller by `0xfe2b0000` and verifies card type SD before reading partition 1.
SPL's boot order contains only that controller. eMMC and SPI nodes are disabled.
No U-Boot command entry is required after physical selection.

BL31 must enter nonsecure U-Boot at EL2. The command checks CurrentEL, masks
interrupts, flushes/disables caches and MMU, stops SD DMA/interrupts, and enters
the raw loader directly. Pinned Microkit `loader/src/aarch64/{crt0.S,init.c,el.c}`
sets its own stack, adjusts exception level and sets up translation. The loader
does not consume Linux boot registers. PSCI uses SMC; only one core is started.
Hypervisor support remains enabled. No Linux image header is added.
