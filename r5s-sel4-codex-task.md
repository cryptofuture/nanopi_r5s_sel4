# Codex implementation task: NanoPi R5S bootable seL4 microSD with independent Ubuntu recovery

## 1. Objective and working style

Implement a reproducible development project that produces a **bootable microSD image for the physical FriendlyElec NanoPi R5S, 4 GB model**. Once the board selects the SD boot chain, it must automatically start **seL4/Microkit and a native Rust protection domain**, without a Linux kernel, VM, interactive U-Boot commands, network boot server, or network connection.

**Preserve the existing Ubuntu installation and its boot firmware on internal eMMC.** Ubuntu remains installed but does not run concurrently with this first standalone prototype. Independent recovery means that powering off, removing the experimental SD, and powering on normally returns to the existing Ubuntu installation, without depending on the experimental kernel or reflashing eMMC. This recovery procedure must be tested, not merely described as guaranteed.

The longer-term product is an independent firewall with iptables-like rule construction, not Linux/Netfilter compatibility. This task is the boot/platform milestone, not the firewall implementation. Exclude PCIe/NIC drivers, NAT, DHCP, Linux virtualization, a web console, performance claims, and production-security claims from the initial deliverable.

Implement code, image-building tools, tests, and documentation, not another research-only plan. Use the supplied observations instead of asking the user to repeat the inventory. Work autonomously on builds and offline tests. If hardware execution or a required input is unavailable, finish everything possible and state the exact untested step or missing input; do not invent successful board tests.

The user does not want soldering or a mandatory UART adapter. Serial logging may exist as an optional diagnostic. Hardware success must be observable using a deliberate LED pattern. Do not assume HDMI or USB console support in seL4.

## 2. Non-negotiable storage and execution safety

A previous attempt to run `sd_update` against the mounted FriendlyWrt eMMC caused segmentation faults and loss of a recognizable partition table. The board was subsequently recovered and Ubuntu installed. Never reproduce that workflow.

- Do not write to internal eMMC, its boot0/boot1/RPMB areas, SPI flash, OTP/eFuses, firmware environment, `misc`, recovery, or Ubuntu partitions. No eMMC erasure to force SD boot; no `fw_setenv`, `saveenv`, `sd_update`, or vendor eFlasher in this project’s normal workflow.
- Do not modify the running Ubuntu kernel, `/etc/fstab`, network setup, installed bootloader, or persistent boot priority. Propose necessary host configuration changes separately instead of applying them.
- Generate regular image files by default. Do not automatically flash any block device, reboot a board, bind/unbind NICs, access `/dev/mem`, or invoke recovery tools.
- Provide a separately invoked, dry-run-by-default SD writer with explicit device selection and interactive confirmation. This task authorizes preparation of that tool, not execution of destructive writes on attached hardware.
- All boot stages generated for the SD must avoid persistent eMMC writes, including implicit environment saves, boot counters, automatic filesystem repair, or firmware updates. Prefer environment-in-nowhere; any necessary persistent state must live on the SD at verified non-overlapping locations.
- Do not initialize network DMA or grant the first Rust component eMMC/SD/PCIe capabilities. Audit any bus-mastering hardware initialized by the SD firmware and quiesce it before the handoff.

The existing 64 GB card is **currently storage**, mounted at `/mnt/storage`, and contains useful evidence. A bootable image may require destroying/replacing that layout. Back up its contents outside that card first. Never store the only image, backup, or recovery instructions on the device about to be overwritten.

## 3. Measured target baseline

These are user-supplied observations, not promises about every R5S revision. Existing Linux and U-Boot MMC numbers differ; never translate one numbering scheme directly into the other.

| Item | Observed state |
|---|---|
| Board | FriendlyElec NanoPi R5S; runtime board model `NanoPi R5S` |
| SoC / CPU | RK3568B2; AArch64, four Cortex-A55 cores |
| RAM | 4 GB installed; Ubuntu reports approximately 3.8 GiB |
| Current OS | FriendlyElec Ubuntu 24.04 Core; kernel `6.1.141` |
| Internal eMMC | `/dev/mmcblk2`, type `MMC`, 15,634,268,160 bytes |
| Ubuntu root/data | `root=/dev/mmcblk2p8`, `data=/dev/mmcblk2p9`; do not rely only on blank `lsblk` mountpoint fields |
| eMMC partitions | p1 uboot 4 MiB; p2 misc 4 MiB; p3 dtbo 4 MiB; p4 resource 16 MiB; p5 kernel 40 MiB; p6 boot 32 MiB; p7 recovery 32 MiB; p8 rootfs about 1.9 GiB; p9 data about 12.5 GiB |
| eMMC auxiliary areas | `/dev/mmcblk2boot0`, `/dev/mmcblk2boot1`, each 4 MiB; all protected |
| microSD | `/dev/mmcblk0`, type `SD`, 62,534,975,488 bytes / about 58.2 GiB |
| Current SD filesystem | One ext4 partition, `/dev/mmcblk0p1`, mounted at `/mnt/storage`; UUID-based automount previously configured; ext4 reserved blocks set to 1% |
| Existing `/boot` | Empty directory; no discovered extlinux configuration |
| Environment tools | `fw_printenv` absent in current Ubuntu; earlier FriendlyWrt had no configured environment location |
| Kexec | Unknown: kernel configuration was not available; do not use kexec as the initial boot path |

The firmware string is:

```text
androidboot.fwver=ddr-v1.23-03ea844c5d,spl-v1.14,bl31-v1.46,bl32-v2.17,uboot-c5c053fa55-07/10/2026
```

Boot arguments also include `storagemedia=emmc`, `androidboot.storagemedia=emmc`, `androidboot.verifiedbootstate=orange`, `earlycon=uart8250,mmio32,0xfe660000`, `console=ttyFIQ0`, `rootfstype=ext4`, and `rootflags=discard`. These describe the current Linux boot, not arguments to pass blindly to Microkit.

## 4. Existing firmware and evidence

The read-only dump of `/dev/mmcblk2p1` is a 4 MiB FIT file, created July 10, 2026, description `FIT Image with ATF/OP-TEE/U-Boot/MCU`, default configuration `conf`, description `rk3568-nanopi5`.

| FIT index | Component | Compression | Observed load address |
|---|---|---|---|
| 0 | U-Boot, 596,643 stored bytes | gzip | `0x00A00000` |
| 1–6 | ARM Trusted Firmware components | mixed | `0x00040000`, `0xFDCC1000`, `0x0005C000`, `0xFDCCE000`, `0xFDCD0000`, `0x0005A000` |
| 7 | OP-TEE | gzip | `0x08400000` |
| 8 | U-Boot DTB, 13,631 bytes | none | not specified |

The FIT lists `sha256,rsa2048:dev`, but the displayed signature value is unavailable. Neither this metadata nor the Android “orange” state establishes the complete secure-boot policy. Do not change secure boot settings. `0x00A00000` is the existing U-Boot component address, not a recommended Microkit load address.

The U-Boot DT identifies `friendlyelec,nanopi5`, `rockchip,rk3568-evb`, `rockchip,rk3568`. It is smaller than the Linux runtime DT and generated warnings about missing interrupt parents/phandles. Use it as firmware evidence, not a complete board description.

Existing files on the **R5S**, not necessarily on the Codex host:

```text
/tmp/r5s-uboot.itb
/tmp/r5s-uboot.dtb
/tmp/r5s-uboot.dts
/mnt/storage/r5s-linux.dtb
/mnt/storage/r5s-linux.dts
```

Preserve `/tmp` artifacts before reboot. Archive evidence on the development PC, including checksums. Paths are not proof that Codex can access these files. The supplied brief bundle contains `evidence/device-tree-excerpts.txt` and `evidence/pci-inventory.txt`, not the complete DTBs. Use the observations below and inspect supplied full DTBs if available. Never manufacture their contents.

An earlier `strings` scan missed U-Boot commands because the embedded U-Boot component is compressed. Any further command inspection must extract FIT index 0, decompress it, and correlate strings with the actual source/configuration. Strings alone do not prove a working boot feature.

## 5. Memory, interrupt, GPIO and network facts

### Physical RAM

The runtime tree uses two address cells and two size cells. Its nonzero `/memory/reg` entries decode to these **half-open** ranges:

| Start | End, exclusive | Size |
|---|---|---|
| `0x00200000` | `0x08400000` | 130 MiB |
| `0x09400000` | `0xF0000000` | 3,692 MiB |
| `0x1F0000000` | `0x200000000` | 256 MiB |

Total: 4,078 MiB. Remaining entries have zero size; ignore them. Do not flatten this into a contiguous 4 GiB block. In particular, the final range is at a high physical address, not `0xF0000000`.

Reservations and exclusions:

- Preserve everything below `0x00200000`. `ramoops` occupies `[0x00110000, 0x00200000)`. Earlier DT excerpts also name SCMI shared memory at `0x0010F000`.
- Preserve `[0x08400000, 0x09400000)`. This omitted RAM range includes the observed OP-TEE load address; firmware ownership requires verification, not reassignment.
- Do not treat `[0xF0000000, 0x1F0000000)` as RAM. It includes peripheral space.
- Linux CMA describes `[0x10000000, 0x10800000)`, `shared-dma-pool`, `reusable`, with `inactive` present. This is not automatically secure firmware memory. Conservatively exclude it initially unless deliberately reconciled.
- DRM logo/LUT entries have zero length. Also inspect the flattened DT reservation table, secure firmware requirements, and U-Boot’s relocated live regions; `/reserved-memory` alone is insufficient.

Choose a small proven low-memory subset for the first prototype, not all RAM. Apply the policy consistently to the seL4 kernel build, Microkit SDK/platform data, loader placement, and PD allocations. Limiting only the application XML is not sufficient. Treat `0x30000000`, documented for upstream ROCK 3B, as a **candidate requiring link-map/overlap verification**, not an R5S guarantee. Check loader input/output, kernel/PD ELF segments, stacks, page tables, DTBs, firmware, and live U-Boot buffers for overlap.

### Interrupts and firmware interface

| Item | Observed value |
|---|---|
| GIC | `arm,gic-v3` |
| Distributor | `0xFD400000`, size `0x10000` |
| Redistributors | `0xFD460000`, size `0xC0000` |
| ITS | `0xFD440000`, size `0x20000` |
| PSCI | `arm,psci-1.0`, method `smc` |
| Timer | `arm,armv8-timer`; DT tuples `(1,13,0xf04)`, `(1,14,0xf04)`, `(1,11,0xf04)`, `(1,10,0xf04)` |
| UART2 | `0xFE660000`; optional debug interface |
| SD host | `0xFE2B0000` |
| eMMC host | `0xFE310000` — keep outside application capabilities |

DT interrupt tuples and Linux IRQ numbers are not interchangeable seL4 IRQ identifiers. Reuse/verify the upstream timer/GIC implementation. Start single-core and defer ITS/MSI-X.

### LED success indicator

The runtime pinctrl states `rockchip,pins = <4 26 0 ...>` for SYS. The LED node has `gpios = <... 26 0>`, label `sys_led`, and Linux default trigger `heartbeat`. Thus the observed target is **GPIO4_D2 / bank 4 pin 26, active high**, with GPIO4 at `0xFE770000`. Other pins: WAN GPIO2/17, LAN1 GPIO3/30, LAN2 GPIO3/31.

Implement only SYS initially. Resolve clocks, reset, pinmux and register layout from verified RK3568 sources. Rockchip GPIO-v2 uses split register halves and write-enable masks; do not use a naive whole-register `1 << 26` write. Map only the required device pages with correct memory attributes and use documented barriers/volatile access. Isolate Rust `unsafe` MMIO and test mask calculations.

A GPIO4/0 active-low `gpio-keys` entry labelled `K1` was observed. **Do not assume this is the physical MASK button.** BootROM masking and Linux key events are different mechanisms.

### Ethernet inventory, for later work only

| Linux interface | Controller | Driver / PCI address |
|---|---|---|
| eth0 | Integrated GMAC; currently UP | `st_gmac`, kernel `6.1.141` |
| eth1 | RTL8125, `10ec:8125`, revision 05 | `r8125 9.016.01-NAPI-g3fda70e`; `0000:01:00.0` |
| eth2 | RTL8125, `10ec:8125`, revision 05 | same driver; `0001:11:00.0` |

Both Realtek endpoints negotiated 5 GT/s x1; MSI-X enabled with a 32-vector table capability, not proof of 32 actively allocated vectors. ASPM was disabled. Both Ethernet cables lacked carrier in the snapshot, but the PCIe links were present.

Root APB addresses: `0xFE260000` and `0xFE270000`; DBI addresses from the DT: `0x3C0000000` and `0x3C0400000`. Linux assigned BAR2/BAR4 at `0xF4200000`/`0xF4210000` and `0xF2200000`/`0xF2210000`. These are observed allocations, not immutable firmware guarantees. The `1d87:3566` root-port ID is not evidence that the board is an RK3566.

No effective PCIe DMA isolation has been demonstrated. Do not promise isolation of malicious DMA drivers. Networking is excluded from this milestone.

## 6. Boot architecture and selection

Implement this intended path:

```text
RK3568 BootROM selects microSD
  -> R5S-compatible DDR/first-stage firmware on microSD
  -> compatible secure firmware + R5S-specific U-Boot on microSD
  -> deterministic automatic load of the SD's Microkit image
  -> seL4
  -> native Rust LED protection domain
```

Use upstream **NanoPi R5S** U-Boot support (`nanopi-r5s-rk3568_defconfig`) as the starting point for SD firmware; use seL4/Microkit’s **ROCK 3B / RK3568** support as the kernel reference. These are different board definitions: do not transplant a ROCK 3B DDR/bootloader image onto R5S. Verify the actual pinned configurations, source paths, firmware ABI, DDR blob, BL31/BL32 requirements, and licensing.

Pin exact compatible source commits, firmware blobs and hashes, Rust toolchain, cross compiler, Microkit/kernel/rust-sel4 versions. Record the inputs in a lock manifest. Do not silently track `main`, select arbitrary firmware by filename, or reuse the existing FIT by concatenation.

**Boot-source selection is unresolved on this physical board.** Its previous eflasher card did not win over eMMC after repeated cold boots. FriendlyElec documents loader-dependent priority and a MASK-button SD-selection method. Validate a non-destructive procedure for this board; do not assume an ordinary reset selects SD or that the MASK button alone implements a software menu. No more blind repeated cold-boot advice.

“Automatic startup” means no console interaction **after selecting the experimental SD boot path**. A documented physical MASK action at power-on is acceptable. If firmware selection cannot be established without touching eMMC, report that precise hardware blocker while still delivering builds and tests. Do not weaken the preservation requirement to make a demonstration boot.

The SD U-Boot must identify its actual SD controller and intended partition; Linux `mmcblk0` does not imply U-Boot `mmc 0`. Use an explicit built-in boot policy or a script that is explicitly loaded by that policy. Merely copying `boot.scr` to FAT is not enough.

Verify image bounds/integrity before jumping. Missing/invalid images must show a distinct failure state and must not start installers, scan arbitrary eMMC boot flows, or execute an unverified address. Establish the loader’s required exception level, MMU/cache state, registers, interrupt state, secondary-core handling, and firmware interfaces from pinned source. Do not wrap the raw loader in a Linux boot header and assume compatibility.

A standalone system may still require EL2 under the selected Microkit kernel configuration; do not disable hypervisor support merely because no guest VM is present.

No network boot, DHCP, Internet, host daemon, Linux kexec, or SSH should be needed for normal SD startup. Do not claim secure boot merely because image hashes are checked.

## 7. Rust boot proof and optional watchdog

Implement a small `no_std` Rust Microkit protection domain using the matching official `sel4-microkit` runtime. Define a stable, clearly distinctive success pattern, for example three short SYS flashes followed by a long pause. Only the Rust PD should emit that pattern; U-Boot must use a different stage indication. A normal Linux heartbeat is not proof of seL4 execution.

Validate timer/counter access for the selected privilege level. Do not steal the kernel’s architectural timer or assume unrestricted counter access. A simple bounded first-bring-up delay loop is acceptable if documented as uncalibrated; a supported timer source is preferable. Keep permissions minimal; an initial trusted setup component may handle shared pinmux/clock registers before the LED PD starts.

Provide a fault-injection build that does not reach the success pattern. Retain optional serial diagnostics, but do not make them prerequisites. QEMU can test build/runtime/IPC logic; it does not validate RK3568 GPIO, DDR or boot selection.

An independent hardware watchdog is a useful **second milestone**, not a substitute for card-removal recovery. Implement it only after proving clock/reset setup, firmware handoff, actual expiry reset, and reset behaviour. Never claim that a watchdog necessarily returns to Ubuntu: it could select SD again. If adding automatic retry suppression or one-shot arming, keep state entirely on SD, persist it before handoff, and document torn-write handling. Do not modify eMMC boot counters or assume an untested chainload into its vendor firmware.

## 8. Independent recovery contract

Required baseline: boot Ubuntu successfully with no SD, then test experimental boot. If the experiment hangs, power off, remove SD, release MASK, and power on normally. This must work even if the Microkit loader, kernel, PD, or SD filesystem fails. No SSH access to the failed system should be necessary.

Document expected LED states and a finite observation window for the test, without presenting LED activity as proof of storage flashing. Do not implement infinite automatic reboot loops. A corrupt first-stage SD loader may never reach any LED or watchdog code; the physical recovery path must still cover that case.

Document USB Maskrom as an independent, **last-resort** recovery interface, using the manufacturer’s exact R5S data-port/cable instructions and RK3568 loader. Separate device detection from destructive erase/reinstall. Do not embed erase/flash commands in normal build, boot, retry, or recovery scripts. Erasing eMMC does not satisfy “Ubuntu preserved.”

Maintain backups and evidence on another computer. A full live eMMC dump is not automatically filesystem-consistent; use a documented quiesced/offline method for a restorable OS backup. Read-only boot-partition/GPT snapshots and their hashes are useful, but are not a substitute for a full backup. Compare immutable boot regions before and after tests; do not expect a byte-identical whole running Ubuntu disk.

## 9. Image construction and guarded SD writer

Create a regular disk-image artifact. Use a validated Rockchip raw bootloader area and a FAT boot partition containing the Microkit image, manifest and explicit boot policy. Starting the first filesystem at or beyond 16 MiB is a design candidate; prove no overlap with the exact bootloader/binman layout, GPT, backups or alignment requirements. Keep optional data storage outside the minimal boot requirement. Give the image unique partition/filesystem IDs rather than copying Ubuntu identifiers.

Prefer unprivileged file-based assembly. Any optional loop-device operations must have cleanup traps and must never select a physical disk implicitly. Image validation must check bootloader locations, GPT consistency, partition bounds, file presence, load ranges, image hashes and the absence of installer/update hooks.

The writer must:

1. Default to displaying its plan; require a whole-device path, explicit execution flag and typed confirmation tied to the actual target identity.
2. Refuse eMMC/boot/RPMB devices, partitions, system/root/boot/backing-overlay devices, swap, mounted descendants, unknown identities, insufficient capacity and a source image located on the target.
3. On R5S, require sysfs type `SD`, and refuse the known eMMC even if device numbers change. On a PC USB reader, use an independent selection policy with model/serial/capacity checks; USB/removable flags alone are insufficient.
4. Identify the active root dependency chain, including overlay/lowerdir/upperdir. Abort if unresolved; blank `lsblk` mountpoint output is not permission to write.
5. Require the operator to back up and unmount SD storage. Do not silently edit fstab or force unmount. Keep the writer, image and logs off the selected card.
6. Revalidate identity immediately before opening the target; write only the selected SD, flush, reread and compare the written image bytes, and report errors without continuing to boot.

Recreating the card invalidates its old `/mnt/storage` UUID-based mount expectation. Document this explicitly. Any new data-partition automount or 1% ext4 reserve configuration is separate and opt-in, not part of flashing. Do not say that filesystem-signature wiping alone removes all Rockchip raw boot code.

## 10. Required repository and outputs

Use a clear structure such as:

```text
README.md
sources.lock
boards/nanopi-r5s/{hardware.toml,memory-map.md,...}
firmware/{uboot-config,boot-policy,patches,...}
microkit/{platform-patches,system.system,...}
pds/heartbeat/                 # native Rust component
scripts/{build,make-image,verify-image,collect-evidence,flash-sd,...}
tests/
docs/{boot-selection,recovery,hardware-test-plan,security-scope}.md
evidence/                      # raw supplied data; do not fabricate DTBs
out/                           # generated, not committed by default
```

Provide pinned, usable Linux-host build instructions; handle x86-64 versus AArch64 explicitly rather than assuming a host architecture. Use Rust for the PD, Python 3 for validation/image tooling where useful, and robust shell only where appropriate. Keep licensing/firmware provenance clear. Offline rebuilds should work after dependencies are cached; booting is always offline.

Deliver the boot image, SHA-256 manifest, loader and PD ELF files, source/config lock, memory/layout reports, complete build logs, image-validation results and exact SD writing/recovery instructions. A build command must fail loudly if required firmware, board configuration, linker settings or files are missing. Do not output a zero-filled/fake loader under a successful image name.

## 11. Validation and completion report

Implement automated tests for RAM decoding, zero-size entries, excluded-region intersection, 64-bit address handling, partition/loader overlap, GPIO-v2 masks, source-on-target detection, mounted/overlay-root refusal, eMMC rejection, and dry-run behaviour. Test host-side tools against regular files and mocked device metadata. Do not use real eMMC for a negative test.

Maintain separate result states:

| Level | Evidence required |
|---|---|
| Built | Pinned components and Rust PD compile/link; reproducible inputs recorded |
| Image validated | Static structure, integrity and placement checks pass |
| Simulated | Explicit QEMU target boots relevant software logic; no R5S hardware claim |
| R5S boot-tested | SD boot path reaches seL4 and the Rust PD’s unique visible pattern |
| Recovery-tested | Removing SD restores Ubuntu after normal and deliberately failed tests |
| Watchdog-tested | Only if implemented: genuine hardware expiry/reset and documented next-boot behaviour |

Physical acceptance requires: Ubuntu boots without SD before testing; selected SD auto-starts without UART/network; the distinctive PD signal appears; a missing/corrupt payload fails safely; a deliberate PD failure remains recoverable; card removal restores Ubuntu; and immutable eMMC boot-region checks reveal no experimental writes.

End with exact artifacts and commands, tests actually run, results, and a short list of remaining hardware-dependent checks. Never label “image built” as “R5S booted.” Do not present the prototype as a formally verified firewall or production security product.

## 12. Reference sources and provenance

User evidence is authoritative for this individual board’s captured state. Primary upstream documentation supplies implementation references, not a claim of an existing complete R5S Microkit port. Pin the source revisions actually used; documentation headings and moving URLs can disagree about “latest.”

- U-Boot Rockchip board/build documentation (includes NanoPi R5S):
  https://docs.u-boot.org/en/latest/board/rockchip/rockchip.html
- FriendlyElec loader-dependent boot-priority documentation. It also lists an eMMC-erasure method: that method is expressly forbidden here:
  https://wiki.friendlyelec.com/wiki/index.php/Template:RockchipBootPriority
- NanoPi R5S manufacturer instructions and USB recovery:
  https://wiki.friendlyelec.com/wiki/index.php/NanoPi_R5S
- seL4 ROCK 3B reference. Its board-specific SPI installation instructions are not R5S instructions and must not be applied here:
  https://docs.sel4.systems/Hardware/rock3b.html
- Microkit 2.4.0 manual, including ROCK 3B, loader linking and platform support:
  https://docs.sel4.systems/projects/microkit/manual/2.4.0/
- Microkit platform list:
  https://docs.sel4.systems/projects/microkit/platforms.html
- Official Rust Microkit example/runtime:
  https://docs.sel4.systems/projects/rust/tutorial/microkit/hello-world.html
  https://docs.sel4.systems/projects/rust/how-to-use.html
- Kernel / Microkit / Rust sources:
  https://github.com/seL4/seL4
  https://github.com/seL4/microkit
  https://github.com/seL4/rust-sel4
- Rockchip firmware reference:
  https://github.com/rockchip-linux/rkbin
- GPIO-v2 reference implementation and pin binding:
  https://github.com/torvalds/linux/blob/master/drivers/gpio/gpio-rockchip.c
  https://www.kernel.org/doc/Documentation/devicetree/bindings/pinctrl/rockchip,pinctrl.txt
- U-Boot watchdog and bootcount interfaces; availability is configuration-specific:
  https://docs.u-boot.org/en/latest/usage/cmd/wdt.html
  https://docs.u-boot.org/en/latest/api/bootcount.html

Proceed with implementation and offline validation. Preserve Ubuntu, avoid further diagnostic loops, and stop before any unapproved hardware write or reboot.
