# Diagnostic completion record

Status: **built, statically validated, hardware-untested**. No simulation, device
write or reboot was performed. Use [the R5S-side handoff](uboot-diagnostic.md)
for transfer, checksum checks, guarded dry-run/write commands, one controlled
observation, recovery and the full result matrix. It requires no PC card reader.

Deliverables in `out/diag-uboot/`:

| Artifact | Purpose |
|---|---|
| `r5s-uboot-diag.img` | 96 MiB GPT/FAT diagnostic image; no loader.img |
| `r5s-uboot-diag.manifest.json` | Profile, exact pattern, pinned inputs and source/configuration hashes |
| `SHA256SUMS`, `ARTIFACTS.json` | Artifact hashes, identities and validation status |
| `r5s-diag-handoff.tar.gz`, `.sha256` sidecar | Image and native Python writer/validator for staging on R5S eMMC |
| `u-boot.elf`, `u-boot.map`, `audit.json` | Command registration/default environment and component evidence |
| `firmware-source/`, `u-boot.patch` | Diagnostic sources and generated upstream integration patch |
| `logs/` | Build, compiler command, object/linked disassembly, FIT, validation and tests |

Diagnostic image SHA-256:

```text
7c231a8b7680e06fb8e0c48433f76fb68d9e047ada839fe07b1782bdfac93769
```

Original normal image SHA-256 remains:

```text
c9ba7c07b801bb1d0beaa7cd4fdfe56f941435b723aad3477e9f90df03406304
```

Changes are limited to a compile-time diagnostic branch and shared LED setup in
`firmware/r5sboot.c`, the isolated build option/wrapper, profile-aware packaging
and validation, tests and documentation. The existing writer interface and
normal artifact are unchanged. The diagnostic object's only external functions
are `udelay` and `cmd_never_repeatable`; its linked loop has no return or payload
handoff. The actual ELF command-table entry resolves to its `do_r5sboot` handler.

DDR/idbloader, SPL and both generated DTs, board configuration, all six BL31
components and the original source lock are unchanged. Only U-Boot proper
changes inside the FIT; BL32 is still absent. Extraction, advertised FIT hashes,
packed-FIT bytes and the newly built U-Boot payload were cross-checked. No
definite earlier-stage defect was discovered, and no speculative earlier-stage
fix was applied. Solid red remains ambiguous until this diagnostic is observed.

Offline checks actually completed: 30 Python host tests, including the real C
command compiled with mocked MMIO and two pattern cycles, native R5S writer
metadata/refusal paths, all original image/negative-image checks, and diagnostic
packaging/profile checks. The standalone Rust GPIO mask test also passed. Two
final diagnostic image assemblies have identical hashes. Reassembling the
normal image with the updated shared tool reproduces its original hash exactly.
GPT CRCs/backups/bounds, FIT placement/content hashes, read-only FAT checks,
policy and firmware hashes passed.

Physical SYS operation, timer behavior, command reachability on the board,
BootROM/firmware selection and this diagnostic's card-removal recovery remain
untested. The user already observed Ubuntu SSH recovery after the prior normal
image attempt; bytewise eMMC preservation still lacks a before/after comparison.
