# RTL8125 B51 DMA audit

The candidate layout reserves 1 MiB at CPU PA `0x24200000` and exposes it to LAN1 at PCIe address `0x80000000`. The provider VA is `0x62000000`. The image build validates the captured DRAM banks and reservations, checks loader overlap and verifies the pinned Normal-NC kernel. The generated Microkit system maps the region only into the LAN1 provider. The inactive LAN2 root has no provider to enable its DMA.

| Object | CPU VA | CPU PA | PCIe DMA address |
|---|---:|---:|---:|
| TX descriptor table | `0x62000000` | `0x24200000` | `0x80000000` |
| RX descriptor table, legacy TX layout | `0x62000400` | `0x24200400` | `0x80000400` |
| RX descriptor table, 32-byte TX layout | `0x62000900` | `0x24200900` | `0x80000900` |
| TX packet slot 0 | `0x62001000` | `0x24201000` | `0x80001000` |
| RX packet slot 0 | `0x62021000` | `0x24221000` | `0x80021000` |

## Configuration and ownership

1. `R5sPcie::map_inbound` calls `pcie::map_inbound` on LAN1 DBI `0x3c0000000`. It requires unrolled iATU mode and programs region 0 at `0x3c0300100`. It disables the region before probing alignment and limit bits, sets PCIe base `0x80000000`, inclusive limit `0x800fffff` and CPU target `0x24200000`, then checks fields and enabled state. In Linux v6.12, `dw_pcie_iatu_detect` uses the same all-ones viewport test, and `dw_pcie_prog_inbound_atu` uses the same PCIe-base/CPU-target direction and bounded enable readback. `rockchip_pcie_probe` matches RK3568 and delegates root setup via `rockchip_pcie_configure_rc`. The board may still have interconnect, routing or DRAM accessibility constraints that these MMIO reads cannot prove. [Linux DesignWare implementation](https://github.com/torvalds/linux/blob/adc218676eef25575469234709c2d87185ca223a/drivers/pci/controller/dwc/pcie-designware.c) · [Linux Rockchip glue](https://github.com/torvalds/linux/blob/adc218676eef25575469234709c2d87185ca223a/drivers/pci/controller/dwc/pcie-dw-rockchip.c)
2. `InboundWindow::translate` bounds-checks the entire extent. `PackedDma` rejects CPU VA/PA overlap and enforces 256-byte ring and 2048-byte packet-buffer alignment. Both rings hold 64 descriptors. RX slots are 16 bytes. In the new v3 layout TX slots are 32 bytes, the TX guard starts at offset `0x800`, the RX guard at `0xd00`, and the cleared allocation ends before payload offset `0x1000`. The one-shot frame uses TX slot 0 and its buffer.
3. `patches/router/sel4-rtl8125-normal-nc.patch` assigns outer-shareable Normal-NC to the kernel direct map and corresponding user pages. It rejects cacheable, executable, device and larger overlapping user mappings. The build checks the cap layout. This is different from sDDF i.MX, which uses an uncached descriptor mapping with device-visible ring PA and explicitly cleans/invalidates cached payloads in `virt_tx.c` and `virt_rx.c`. A CPU alias using conflicting attributes would invalidate the R5S contract. [sDDF DMA example](https://github.com/sel4-cap/sDDF/blob/2cae425f735d6dd214bdb285e89275a87625bba3/examples/echo_server/board/maaxboard/echo_server.system)
4. `PackedDma::publish` writes payload before the descriptor handoff, writes options and address before OWN, and uses `dsb sy` before handing over ownership. `kick_tx` orders OWN before the 16-bit doorbell. `status` reads OWN volatile and barriers before subsequent CPU reads. `DeviceIo` validates mapping, width and alignment and orders volatile accesses. Ordering cannot fix an unreachable bus address.
5. The setup enables memory decoding while mastering is off. It verifies descriptor bases and enables root and endpoint mastering before enabling TX. On terminal result the shared shutdown path resets the MAC, clears endpoint mastering, waits for PCIe Transactions Pending to clear, disables endpoint/root decode and uses the port-reset fallback on a failed shutdown. The pool stays reserved.

## Interpreting evidence

The DMA tally diagnostic runs the same one-frame TX attempt and snapshots its result before requesting a counter dump into a separate 256-byte slot in the same inbound window (CPU PA `0x242fff00`, PCIe address `0x800fff00`). It uses two 32-bit address registers at `0x14` and `0x10` and requests the dump with bit 3 of `0x10`. The counter request uses neither the TX ring nor its doorbell. [FriendlyARM counter dump](https://github.com/friendlyarm/r8125/blob/3fda70ebcba22f281ec4e3a69c355992c8823fc0/r8125_n.c#L752)

| SYS | Observation |
|---:|---|
| 28 | Dump command cleared and the sentinel changed. Endpoint-to-RAM DMA writeback works at the scratch address. With TX timeout 23, prioritize TX descriptor fetch and queue activation. |
| 29 | Command cleared and the sampled TX counter words still contain the sentinel. Check inbound translation and memory visibility. |
| 30 | Command timed out or MMIO access failed. The tally probe is inconclusive. Linux normally gates counter dumps on RX enabled, while this image leaves RX off. |

Raw status words 17 and 18 contain the tally TX packet count low and high dwords after the command clears. The count is an absolute hardware snapshot. A tally write proves neither descriptor fetch nor physical TX. Record codes 21–30 and the peer capture from the same boot.

- PCI configuration enumeration, BAR and iATU readback and carrier verify distinct control-path steps. None proves the endpoint can fetch the TX descriptor at `0x80000000` or payload at `0x80001000`.
- The diagnostic records descriptor opts1, MAC interrupt status, and root and endpoint PCI Command/Status. Conventional PCI error bits and MAC TX or system errors get separate LED codes. These are sticky/status snapshots, so an asserted bit can predate the doorbell. PCIe AER and Device Status are outside this diagnostic. A quiet snapshot cannot exclude a fabric transaction failure.
- OWN clear supports DMA descriptor access and writeback. Only the matching captured frame establishes payload delivery. OWN timeout with a PCI error points toward the PCIe transaction path. OWN timeout without a sampled error leaves TX engine/doorbell, inbound reads and writeback hypotheses open. A second independent board or PCIe trace can distinguish those if the optical evidence is inconclusive.
- The iATU configuration and PD mapping are distinct from hardware DMA confinement. This design uses a trusted onboard endpoint and reserves a private pool in software.
