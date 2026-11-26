# LAN1 DMA map

All intervals are half-open. The 64-entry TX and RX rings use 16-byte Type 1 descriptors and separate 2,048-byte buffers.

| Allocation | Owner CPU VA | CPU physical | PCIe bus |
|---|---|---|---|
| Entire LAN1 pool | 0x62000000-0x62100000 | 0x24200000-0x24300000 | 0x80000000-0x80100000 |
| TX descriptors | 0x62000000-0x62000400 | 0x24200000-0x24200400 | 0x80000000-0x80000400 |
| RX descriptors | 0x62000400-0x62000800 | 0x24200400-0x24200800 | 0x80000400-0x80000800 |
| TX buffers | 0x62001000-0x62021000 | 0x24201000-0x24221000 | 0x80001000-0x80021000 |
| RX buffers | 0x62021000-0x62041000 | 0x24221000-0x24241000 | 0x80021000-0x80041000 |
| Adjacent Normal-NC padding | Unmapped | 0x24300000-0x24400000 | Outside the window |
| Existing WAN pool | 0x60000000 in GMAC | 0x24000000-0x24040000 | GMAC identity mapping |

The Microkit report accounts for every fixed 4 KiB frame. The seL4 kernel maps the entire aligned 2 MiB block Normal non-cacheable, outer-shareable. The LAN1 PD alone receives the first 256 pages, writable and execute-never. The second 256 pages remain reserved without a PD mapping. Microkit supplies the virtual and physical addresses to the provider. Build checks cover captured DRAM, FDT reservations, fixed objects and loader copy destinations. The inherited U-Boot guard checks its live allocations against the prototype RAM interval before handing off.

LAN1 uses APB 0xfe260000, DBI 0x3c0000000, configuration 0xf4000000 and BAR2 at 0xf4200000. The provider maps 32 KiB of BAR2 only after the build reserves the MMIO pages, and checks the actual BAR size before operational accesses. The DesignWare region 0 inbound register block is at 0x3c0300100. Its source limit is inclusive 0x800fffff and target is 0x24200000. CPU VA never enters a descriptor.

Hardware-reported alignment and limit readback gate activation. Actual reachability requires an RX completion with data in its corresponding buffer. The trusted endpoint has no per-device IOMMU confinement.

The captured RK3568 node supplies DBI, APB, configuration and outbound memory ranges. The PCIe bus address above is the diagnostic's proposed inbound address. The register contract uses control words at offsets 0x00/0x04, base at 0x08/0x0c, limit at 0x10/0x20 and target at 0x14/0x18. An unimplemented viewport at DBI+0x900 selects unrolled mode. Address-match memory type and enable are read back with BAR-match clear. [Pinned U-Boot register definitions](https://github.com/u-boot/u-boot/blob/127a42c7257a6ffbbd1575ed1cbaa8f5408a44b3/drivers/pci/pcie_dw_common.h), [Rockchip controller integration](https://github.com/u-boot/u-boot/blob/127a42c7257a6ffbbd1575ed1cbaa8f5408a44b3/drivers/pci/pcie_dw_rockchip.c), [Linux DesignWare implementation](https://github.com/torvalds/linux/blob/v6.12/drivers/pci/controller/dwc/pcie-designware.c).
