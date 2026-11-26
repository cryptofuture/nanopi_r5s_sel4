# RTL8125 B51 LAN1 DMA and TX investigation

The root cause remains unknown. The reported hardware evidence is carrier, Linux r8125 9.016.01-NAPI-g3fda70e, CFG_METHOD_5-compatible identity, No Close v3 enabled under Linux, and unsuccessful native trials with both TX layouts. The Linux packet counters and native carrier leave peer delivery unverified. No paired tally and TX results from the board were available.

## Tally prerequisite

The original diagnostic writes ChipCmd 0x04 and runs the tally with RX disabled. FriendlyARM commit 3fda70ebcba22f281ec4e3a69c355992c8823fc0 is the closest source reference for the installed module. Its dump helper itself has no RX check and its normal start enables both engines. Linux r8169 checks CmdRxEnb before counter updates and documents that some parts require it. Either native tally failure remains ambiguous with RX off. Applicability to B51 is a hypothesis. [FriendlyARM dump](https://github.com/friendlyarm/r8125/blob/3fda70ebcba22f281ec4e3a69c355992c8823fc0/r8125_n.c#L752) · [FriendlyARM start](https://github.com/friendlyarm/r8125/blob/3fda70ebcba22f281ec4e3a69c355992c8823fc0/r8125_n.c#L18529) · [Linux counter logic](https://github.com/torvalds/linux/blob/adc218676eef25575469234709c2d87185ca223a/drivers/net/ethernet/realtek/r8169_main.c#L1788)

The pinned tally struct has ARM64 natural alignment of 8 bytes and size 192 bytes, calculated from the unsigned fields and native padding. FriendlyARM allocates coherent memory and writes high, low, then low with bit 3. The new test reserves the last pool page, places counters at offset 0x100, checks both guards, and samples every counter word. It reads ChipCmd before each phase. After one TX attempt it tries the tally with RX off, then enables RX using the already initialized private ring and tries again. When the first command remains pending it polls the same request without rewriting the target or refilling the buffer. This branch cannot distinguish an RX prerequisite from a late completion in a single run. No RX consumer runs. [Tally layout](https://github.com/friendlyarm/r8125/blob/3fda70ebcba22f281ec4e3a69c355992c8823fc0/r8125.h#L2426) · [Coherent allocation](https://github.com/friendlyarm/r8125/blob/3fda70ebcba22f281ec4e3a69c355992c8823fc0/r8125_n.c#L17603)

## Inbound audit

The native driver checks unrolled mode and programs inbound region 0 at DBI + 0x300100. Control 1 is memory type 0, control 2 is 0x80000000 (enabled, address match), base is 0x80000000, inclusive limit is 0x800fffff, and target is 0x24200000. Upper address words are zero. The provider checks alignment, implemented limit bits, every programmed field and the enable state. The outbound configuration and BAR windows operate in the other direction and use distinct outbound banks. The code does not inventory other enabled inbound regions. The installed Linux DMA ranges and effective RK3568 fabric routing are still open. Build-time DRAM and reservation checks establish that the CPU pool is reserved in the candidate. Hardware write evidence is needed to validate endpoint reachability. [Native translation](../drivers/rtl8125/pcie.rs#L610) · [DMA attributes and mapping audit](rtl8125-b51-dma-audit.md) · [DesignWare inbound implementation](https://github.com/torvalds/linux/blob/adc218676eef25575469234709c2d87185ca223a/drivers/pci/controller/dwc/pcie-designware.c)

| Object | CPU VA | CPU PA | PCIe address | Size | Alignment |
|---|---|---|---|---|---|
| TX ring, v3 | 62000000 | 24200000 | 80000000 | 2048 B plus 32 B guard | 256 B |
| TX ring, classic | 62000000 | 24200000 | 80000000 | 1024 B | 256 B |
| TX buffers, frame in slot 0 | 62001000 | 24201000 | 80001000 | 64 × 2048 B | 2048 B |
| RX ring, v3 | 62000900 | 24200900 | 80000900 | 1024 B plus 16 B guard | 256 B |
| RX ring, classic | 62000400 | 24200400 | 80000400 | 1024 B | 256 B |
| RX buffers | 62021000 | 24221000 | 80021000 | 64 × 2048 B | 2048 B |
| Tally, within guarded 4 KiB page | 620ff100 | 242ff100 | 800ff100 | 192 B | 256 B |

All values are hexadecimal except sizes and alignment. All allocations are disjoint and lie inside the checked 1 MiB window. The build uses the pinned Normal-NC seL4 mapping with ordered volatile accesses. Publication orders payload and descriptor body before OWN and orders OWN before notifying the NIC. A cacheable alias would invalidate that reasoning. [Native packing](../drivers/rtl8125/dma.rs) · [Kernel mapping patch](../patches/router/sel4-rtl8125-normal-nc.patch)

## Initialization matrix

FriendlyARM locations below refer to the pinned [r8125_n.c](https://github.com/friendlyarm/r8125/blob/3fda70ebcba22f281ec4e3a69c355992c8823fc0/r8125_n.c). Native locations are in [b51.rs](../pds/pcie_provider/src/b51.rs), [mac.rs](../drivers/rtl8125/mac.rs) and [dma.rs](../drivers/rtl8125/dma.rs). Native values are source expectations or software readback gates. Board register snapshots are unavailable.

| Register or operation | FriendlyARM operation and location | Native expectation or readback gate | Difference and possible impact |
|---|---|---|---|
| PCI Command | PCI enable and master in board setup | Memory decode, then command low bits 6 at root and endpoint before engines | BAR visibility establishes CPU MMIO. Mastering is prerequisite for DMA. |
| ASPM / Link Control | PCIe policy including L1 disable history | Root trains Gen2 and native sets endpoint entry latency at 0x70f | Capture live root and endpoint Link Control to compare policy. Ubuntu reported ASPM disabled. |
| MPS / MRRS | PCI core MPS, MRRS set to 512 by rtl8125_set_mrrs at 18086 unless SoC override | No explicit B51 MPS or MRRS override | Inspect live Device Control first. Ubuntu endpoint was 128/512. |
| Reset | rtl8125_nic_reset at 4499 stops requests and waits for FIFO before reset | Checks FIFO and mitigation idle, then verifies reset clears | Native has bounded failure gates. |
| ChipCmd, IRQ | rtl8125_hw_start at 18529 enables both engines and IRQ | TX 0x04 verified, then RX 0x0c only for tally comparison. Masked, polled IRQ. | RX state may gate tally. |
| TxConfig | rtl8125_hw_config at 18277 programs burst/IFG and bit 6 | 0x03000700 classic or 0x03000740 v3, masked readback | Bit 6 and submission mode are paired. |
| OCP 0xeb58 | 18310 sets bit 0 | Clear in classic, set in v3, verified | Selects 16- or 32-byte stride. |
| OCP 0xe614 | 18315 sets queue-related field to 0x200 for method 5 | Same field programmed | MAC OCP write, no saved board readback. |
| OCP 0xe63e | 18179 encodes queue count, 18326 sets bits 5:4 to 2 | One queue, bits 11:10 checked zero, bits 5:4 cleared as in r8169 B51 | Real difference. Field meaning and relevance to the failure are unverified. |
| TX/RX bases | rtl8125_desc_addr_fill at 18935 writes address halves | 0x24/20 and 0xe8/e4, verified before master | Both use device addresses. |
| Tail/close | rtl8125_setup_mqs_reg at 14648, v3 selected for method 5 at 14979, w16/r16 | 0x2800/0x2802, initially zero, 16-bit descriptor count wraps independently of 64-slot ring | Native polls one in flight and reclaims only when close equals submitted tail. |
| MAC MCU / PHY | link_on_patch at 4950 and hw_config at 18200 run vendor sequence | Native B51 firmware and mixed Linux/vendor initialization, carrier checked | Differences remain. Carrier confirms PHY link only. |
| Descriptor publication | rtl8125_start_xmit at 19964 prepares DMA, publishes OWN, advances producer and rings doorbell | Aligned Normal-NC ring and barriers before OWN and doorbell | Classic r8169 uses OWN and 16-bit TxPoll at 0x90. Both native modes are internally paired. |

FriendlyARM's DMA-barrier change and ASPM change provide comparison points. PR 3 concerns PTP/build and RX-related changes and supplies no board evidence for this TX failure. [DMA barriers](https://github.com/friendlyarm/r8125/commit/a74102bd1f949f80617441a54cad0315ebbedb65) · [ASPM change](https://github.com/friendlyarm/r8125/commit/61ed2fda85e3a17148129fb7fb9072c8b247a16c) · [PR 3](https://github.com/friendlyarm/r8125/pull/3)

## Board decision table

| Evidence from same boot | Next discriminating test |
|---|---|
| TX timeout and guarded tally write | Check tail readback and PCIe read-request completion errors. A PCIe trace can distinguish absence of a TX fetch from an unsuccessful fetch. |
| TX timeout, RX-off unchanged, RX-on write | Repeat from cold boot to establish RX dependence, then focus on the queue and PCIe reads. |
| TX timeout and both dumps unchanged | Collect live Linux DMA addresses and root inbound registers on the same port to compare hardware routing. |
| TX timeout and tally pending even with RX | Capture endpoint and root AER around the request and compare with a known successful FriendlyARM tally on the board. |
| TX completes, no peer capture | Baseline the same peer and cable with a directed Linux transmission and compare TX hardware counters. |
| TX completes and peer captures expected frame | Repeat cold boot to verify repeatability. |
| Inbound readback mismatch | Identify the rejected iATU field and verify the detected alignment and region layout before another DMA attempt. |
| Guard modification | Stop this trial. Inspect byte offsets and determine whether the actual hardware write span exceeds the pinned struct or aliases another region. |

The confirmed diagnostic defect is ambiguity from running the sole tally test with RX off and checking only the first counter without guards. No confirmed hardware root cause supports changing the production datapath.
