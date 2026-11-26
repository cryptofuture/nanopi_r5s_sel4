# Hardware acceptance, not yet executed

Run only after reviewing full board DT reservations, the locked firmware, and
the read-only immutable-region snapshot/backup plan in [recovery](recovery.md).
No board connection, hardware write, reboot or recovery command was executed
by the development tools in this workspace.

| State | Current evidence |
|---|---|
| Built | SDK/kernel/loader, Rust normal/fault PD and SD U-Boot compile |
| Image validated | See generated validation log and checksums |
| Simulated | Not run; R5S GPIO is not a QEMU machine |
| R5S boot-tested | Not confirmed: user observed solid red, no Rust pattern |
| Recovery-tested | User observed Ubuntu SSH after SD removal; no before/after eMMC hashes |
| Watchdog-tested | Not implemented |

1. Boot existing Ubuntu with SD absent. Record the baseline, backup and immutable
   boot hashes. Copy `/tmp` firmware and DT evidence to another computer.
2. Validate the regular image. Back up SD storage off-card, unmount it, and use
   the separately invoked writer only after explicit operator authorization.
3. Power off and use the candidate physical MASK selection procedure. No UART
   or network interaction is required after selection. Observe for at most
   60 seconds: one long SYS flash is U-Boot; repeated groups of three short
   flashes are Rust. No light is inconclusive, steady SYS is firmware refusal.
   Spin-loop timing is uncalibrated. Optional serial is 1500000 8N1, no flow
   control; missing UART hardware is not a blocker for LED observation.
4. Power off, remove SD, release MASK, and verify Ubuntu returns. Compare
   immutable boot hashes. Record actual outcomes, not expectations.
5. Test missing/corrupt payload variants using `scripts/make-negative-image` on
   regular image files. These images retain normal firmware's compiled payload
   hash, and explicitly declare their expected failure in the manifest. They
   should hold SYS on without any fallback. Repeat card-removal recovery.
6. For the PD-fault variant, preserve the normal artifacts outside `out/`, copy
   `loader-fault.img` to `loader.img` and `heartbeat-fault.elf` to `heartbeat.elf`,
   copy `microkit-report-fault.txt` to `microkit-report.txt`, then rebuild
   `build-uboot`, `check-layout`, and `make-image` under a different output name.
   This binds the firmware to the fault payload; otherwise the firmware would
   reject it first. The PD must not produce three flashes. Repeat recovery.
7. Finally test a deliberately invalid first-stage on a separate test card,
   prepared as a regular file variant by a reviewer. No LED is expected or
   required; card removal must still recover Ubuntu. No automatic reboot loops.

The fault variant can leave SYS off after firmware's stage flash. A steady LED
from firmware refusal and a PD panic are different observations. Without serial,
the negative result proves absence of the success pattern, not which internal
instruction failed. Retain image hashes and label each test card/record.

Stop if boot selection cannot be established without touching eMMC. Required
unresolved inputs: full flattened DT reservations, physical firmware ABI/EL2
handoff and DDR behavior, GPIO clock/reset/pinmux behavior, visible timing,
SD selection, card-removal recovery and before/after boot-region evidence.
