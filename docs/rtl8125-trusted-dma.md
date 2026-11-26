# Trusted RTL8125 DMA

The native router may use trusted onboard PCIe devices once controller startup and DMA reachability are verified. Per-device hardware confinement is optional. A malfunctioning or hostile endpoint could read or corrupt unrelated physical memory. seL4 protection-domain mappings only limit CPU accesses.

The inbound iATU code programs an address-match window using the PCIe source range and CPU target range, with field readback and bounded enable polling. It supports aligned ranges wholly below 4 GiB. Keep bus mastering off when programming it. Register readback verifies configuration. Board reachability and the actual root integration still require qualification. [DesignWare inbound implementation](https://raw.githubusercontent.com/torvalds/linux/v6.12/drivers/pci/controller/dwc/pcie-designware.c)

The LAN1 bring-up profile uses a dedicated Normal-NC pool, two 64-entry descriptor rings and a single broadcast ARP transaction. The provider enables mastering after B51 initialization, firmware execution, link negotiation and descriptor-base readback. A valid peer capture is the final acceptance evidence. LAN2 stays in reset. The working WAN image remains available separately.

See the [LAN1 implementation report](lan1-bringup/implementation-report.md), [DMA map](lan1-bringup/dma-map.md) and [operator procedure](lan1-bringup/operator.md).
