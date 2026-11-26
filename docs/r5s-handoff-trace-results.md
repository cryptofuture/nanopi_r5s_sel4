# Handoff trace completion report

The preserved U-Boot diagnostic's two-long-flash pattern is a user-observed
hardware success. The new preflight, loader-stop and boot-trace images are built
and statically validated, but hardware-untested. No simulation, physical write,
remote access, shutdown or reboot was performed.

| New artifact | SHA-256 |
|---|---|
| `out/handoff-trace/r5s-preflight-stop.img` | `af697706fc81638b13c8da48b61207891c27e8a021522c67c5670f8576b33c2a` |
| `out/handoff-trace/r5s-loader-entry-stop.img` | `210b64b5c5b187162580adcd7072ec79cec8b9bc2008773483b97add9a47edd0` |
| `out/handoff-trace/r5s-boot-trace.img` | `4cfe4737605b507a65394f931b5415aa1bbcdc003f5aac33fe097a34d7b7789d` |

Each final assembly was generated twice and compared byte-for-byte by SHA-256.
The original diagnostic remains
`7c231a8b7680e06fb8e0c48433f76fb68d9e047ada839fe07b1782bdfac93769`;
the original normal image remains
`c9ba7c07b801bb1d0beaa7cd4fdfe56f941435b723aad3477e9f90df03406304`.
The source lock remains unchanged. Preserved original audit/test records have
not been replaced by new reports.

Source-confirmed fixes: enable U-Boot's read-only GPT parser (previously absent)
and correct the loader's `_stack` low-address calculation (16-byte discrepancy
in the preserved ELF). CurrentEL decoding was already correct. Neither defect
is claimed as the observed first physical failure. No speculative DDR/BL31,
RAM-bank, eMMC, pinmux or firmware-priority replacement was made. The package
audit verifies unchanged DDR/idbloader, SPL, DT and six BL31 FIT components.
Only U-Boot proper changes inside the FIT, with a profile-specific payload
length/hash and explicit stage/error behavior.

Debug instrumentation is isolated in local worktree clones and saved patches.
The kernel has a non-owning GPIO device alias and explicitly enables virtual
counter reads while retaining the SDK's physical-counter read export. Both
timer-control exports remain disabled. The monitor shares the PD's single GPIO frame and a RAM owner
flag, preventing competing LED ownership. These are removable debug changes,
not a formally verified configuration or a production security claim.

The [source/operator guide](r5s-handoff-trace.md) gives exact stage locations,
entry/mapping/timing audit, dependency installation/checks, transfer and full-CID
guarded write/readback commands. Start only with preflight. No generic hardware
inventory or normal-image reflash is requested.

`out/handoff-trace/` contains profile payloads and ELFs, symbol maps, config
deltas, `manifests/`, `patches/`, `stage-map.json`, `layout-report.json`,
`fit-command-audit.json`, `ARTIFACTS.json`, `SHA256SUMS`, logs and the transfer
bundle `r5s-handoff-trace-bundle.tar.gz` with checksum sidecar. Actual live
relocated U-Boot/stack/heap/DT addresses remain UNKNOWN until execution; their
runtime guards precede 3. The complete captured runtime DTB/reservation table
is still absent. A scratch return-SP slot is not a post-power-cycle log.

All 50 host tests and the standalone Rust GPIO mask test passed. Host validation
includes the real U-Boot trace C compiled against mocked devices,
all numbered terminal errors, exact frame timings, no preflight fallthrough,
compiled loader-stop disassembly, linked component DWARF checkpoint code ranges,
stale hash/length rejection, single-frame MMIO sharing, loader/kernel/copy
intervals, GPT/FIT/FAT integrity and all existing writer/model/overlay protections.
The current tests cover the measured board/compatible values and full CID;
the short serial is rejected as a replacement. Test results are retained in
`logs/tests.log`; no host result is presented as physical GPIO or handoff proof.

Still required from hardware: preflight/runtime EL/SD-read outcome, actual
loader entry, kernel/monitor/PD progress, the Rust-only heartbeat, controlled
card-removal recovery after these tests, and genuine immutable eMMC boot-region
comparisons. Earlier recovery was observed, but post-diagnostic recovery has not
been supplied. The complete native seL4/Rust boot target is not yet achieved.
