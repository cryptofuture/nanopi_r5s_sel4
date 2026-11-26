# Completion record, 2026-11-15

Follow-up hardware evidence supersedes the original recovery status below:
the user observed solid red with the normal SD image, no Rust heartbeat, and
Ubuntu SSH returned after removing SD and power-cycling. Microkit execution
and bytewise eMMC preservation remain unconfirmed. See
[the separate U-Boot diagnostic](uboot-diagnostic.md).

**Built and statically image-validated. R5S boot and recovery are untested.**

The normal 96 MiB image is `out/r5s-microkit.img`, SHA-256:

```text
c9ba7c07b801bb1d0beaa7cd4fdfe56f941435b723aad3477e9f90df03406304
```

`out/r5s-microkit.manifest.json`, `out/r5s-microkit.sha256`,
`out/SHA256SUMS`, and `out/ARTIFACTS.json` record the artifacts. ELF outputs are
`loader.elf`, `sel4.elf`, `initialiser.elf`, `monitor.elf`, `heartbeat.elf`, and
`heartbeat-fault.elf` in `out/`. Raw payloads include normal/fault loaders.
`missing.img` and `corrupt.img` are explicitly labelled negative-test disk images.
The latter two are designed to be refused by normal SD firmware.

Validation actually run:

```sh
. scripts/env
CARGO_NET_OFFLINE=true python scripts/build --output out/final.img
python scripts/make-image out/final-repro.img
python scripts/verify-image out/r5s-microkit.img
python -m unittest discover -s tests -v
rustc --test pds/heartbeat/src/gpio.rs -o out/gpio-test
out/gpio-test
```

The full offline build completed. Two final image assemblies have identical
hashes. Host tests: 21 passed, none skipped. Rust GPIO mask test: 1 passed.
Upstream Microkit tests during the SDK build: 153 passed. FAT checking is
read-only (`fsck.fat -n`); GPT/header/entry CRC, backup consistency, raw firmware,
fixed FIT offset, payload/policy hashes and placement checks passed. Missing and
corrupt-payload negative images also passed their declared static checks.
The writer's execution path was exercised against a regular file with mocked
device metadata; its physical write path has not been exercised on a device.

Logs are in `out/logs/`, including the final numbered build steps,
`offline-build.log`, `host-tests.log`, `gpio-tests.log`, `image-validation.log`,
negative-image logs, and `sd-fit-inventory.log`. The physical layout/allocation
report is `out/layout-approved.json`; it does not establish firmware ownership.
Sources and provenance are locked in `sources.lock`; `requirements.lock` and the
PD/Microkit Cargo locks record language dependencies.

No physical device writes, reboots, board network changes, firmware recovery
commands, or eMMC modifications were performed. No QEMU run was performed.
Watchdog and networking were excluded as requested.

Remaining hardware checks: full runtime DT reservation table; DDR/BL31 ABI and
EL2 handoff; GPIO setup and visible timing; non-destructive physical MASK SD
selection; automatic startup; missing/corrupt/PD-fault behavior; normal and failed
card-removal recovery; immutable eMMC boot-region hash comparisons. Follow
`hardware-test-plan.md`, `sd-writing.md` and `recovery.md`. A failed SD selection
must not be worked around by erasing eMMC.
