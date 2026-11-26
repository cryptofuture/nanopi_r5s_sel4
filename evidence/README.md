# Evidence status

Only `r5s-sel4-codex-task.md` was present initially. Its observations are supplied
board evidence, not measurements performed by this implementation. The mentioned
DTB/DTS/FIT files and brief-bundle excerpts were not present on this development
host. No files have been fabricated under those names.

`clk-rk3568.c` is an upstream Linux v6.12 implementation reference, not a board
capture. `upstream-manifest.xml` is a fetched upstream build reference, not
hardware evidence. Full DT reservation-table reconciliation remains outstanding.

On the board, preserve `/tmp/r5s-uboot.{itb,dtb,dts}` before reboot and the
`/mnt/storage/r5s-linux.{dtb,dts}` files. Copy them to another computer first, then
use `python scripts/collect-evidence --destination UNIQUE_PC_ARCHIVE INPUT...`.
The collector copies regular files and hashes them; it never reads a block
device. Missing files produce an explicit nonzero result and manifest entry.
