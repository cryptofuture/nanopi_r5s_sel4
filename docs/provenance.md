# Inputs and source audit

`sources.lock` records actual commits, selected blobs, host packages and cached
Python artifacts. Never substitute a similarly named firmware blob or track a
branch silently. The Rust revision is the same one pinned by Microkit 2.4.0's
workspace; kernel headers used for both initializer and PD come from this build.
The kernel commit was selected from the upstream Microkit manifest and compiled
with the locked Microkit release, rather than claiming a preexisting R5S port.

| Component | Reference and license |
|---|---|
| U-Boot | `vendor/u-boot`, GPL-2.0-or-later; R5S defconfig and DTS |
| seL4 kernel | `vendor/sel4`, GPL-2.0-only; RK3568/ROCK 3B CPU/GIC/timer reference |
| Microkit | `vendor/microkit`, see LICENSE.md and LICENSES, chiefly BSD-2-Clause |
| Rust runtime | `vendor/rust-sel4`, BSD-2-Clause, per-file licenses |
| DDR and BL31 | `vendor/rkbin/LICENSE`, Rockchip binary redistribution terms |
| New host/Rust code | BSD-2-Clause, except custom U-Boot C under GPL-2.0-or-later |

DDR: `rk3568_ddr_1560MHz_v1.26.bin`. BL31: `rk3568_bl31_v1.46.elf`.
BL32/OP-TEE is deliberately not included; binman reports optional `tee-os`
absent. This is a compiled combination, not proof of compatible firmware ABI on
the physical board. Do not concatenate/reuse the existing vendor FIT. Preserve
its dump only as evidence.

GPIO register layout follows pinned U-Boot `drivers/gpio/rk_gpio.c`; GPIO-v2 high
data/direction offsets are 0x04/0x0c. Clock gates follow
[`Linux v6.12 clk-rk3568.c`](https://github.com/torvalds/linux/blob/v6.12/drivers/clk/rockchip/clk-rk3568.c),
stored unchanged in `evidence/clk-rk3568.c`: CLKGATE_CON31 bits 8/9, at CRU
0xfdd20000 + 0x37c. Reset IDs 352/353 are SOFTRST_CON22 bits 0/1 at +0x458.
Pinned U-Boot's pinctrl implementation gives GPIO4_D2 the GRF 0x78 nibble 8.
GRF base is 0xfdc60000. All writes use high-half write enables. These settings
require physical validation; no full board runtime DTB was supplied.

Boot layout follows pinned `arch/arm/dts/rockchip-u-boot.dtsi` and generated
`simple-bin.map`: raw image begins on SD sector 64, its FIT at relative 0x7f8000
(absolute 8 MiB), FAT begins 16 MiB. `check-layout` rejects a changed map.
Build logs retain the exact compiler and binman commands.
Pinned `disk/part_efi.c:find_valid_gpt` accepts a valid primary GPT when the
physical card is larger than the image and performs no automatic repair.
The SD firmware's MMC write support is disabled.

Additional primary references:
[U-Boot Rockchip build guide](https://docs.u-boot.org/en/latest/board/rockchip/rockchip.html),
[Microkit 2.4.0 manual](https://docs.sel4.systems/projects/microkit/manual/2.4.0/),
[official Rust Microkit example](https://docs.sel4.systems/projects/rust/tutorial/microkit/hello-world.html).
