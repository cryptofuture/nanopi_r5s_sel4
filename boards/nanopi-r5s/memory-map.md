# Memory policy

User-observed half-open RAM ranges:

| Start | End (exclusive) | MiB |
|---|---|---:|
| 0x00200000 | 0x08400000 | 130 |
| 0x09400000 | 0xf0000000 | 3692 |
| 0x1f0000000 | 0x200000000 | 256 |

Zero-length entries are ignored. The high bank is above 4 GiB. Preserve low
firmware/SCMI/ramoops, the OP-TEE hole `[0x08400000,0x09400000)`, and initially
CMA `[0x10000000,0x10800000)`. The peripheral gap is not RAM.

The prototype uses only `[0x20000000,0x40000000)`, 512 MiB. The kernel overlay
applies this before kernel linking and SDK platform generation. The same SDK
drives Microkit allocation; XML grants only GPIO4. `check-layout` checks the
generated RAM list, kernel ELF segments, loader metadata outputs, loader input
including its ELF BSS/stack/page tables, and CapDL physical object addresses.
The PD's ELF physical fields are virtual-link addresses; physical placement
comes from the CapDL report, not those fields.

`0x30000000` is the loader link/input address. Its overlap with output ranges is
checked from the actual loader format. U-Boot is relocated near top of RAM;
the custom boot command refuses if its relocation or stack reservation enters
the prototype range (including a 16 MiB stack margin). Firmware loaded low is
outside the chosen subset. No DTB is passed as a Linux argument to Microkit.

This does not establish that opaque firmware owns no additional memory. Full
board DTB reservation-table evidence is absent. Capture and reconcile it before
physical acceptance. The existing firmware's OP-TEE address is preserved even
though the new SD FIT deliberately omits BL32. BL31 compatibility and any
additional firmware reservations remain untested.
