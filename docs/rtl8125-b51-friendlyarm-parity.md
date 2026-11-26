# B51 FriendlyARM v3 diagnostic comparison

The candidate follows the verified CFG_METHOD_5 register branches below. Native transmission remains unverified. The Ubuntu module suffix matches FriendlyARM commit 3fda70ebcba22f281ec4e3a69c355992c8823fc0. The live module reported No Close v3 enabled. Its compiled firmware and other module options remain to be verified. [Pinned driver](https://github.com/friendlyarm/r8125/blob/3fda70ebcba22f281ec4e3a69c355992c8823fc0/r8125_n.c)

| Register | Previous Rust | FriendlyARM method 5 | Candidate | Verification |
|---|---|---|---|---|
| E63E | Clear 0x0c30 | One queue encodes zero in 11:10, set 5:4 to 2 | Mask 0x0c30, set 0x20 | Readback |
| E0C0 | Masked 0x4403 | Exact 0x4000 | Exact write | Readback |
| E052 | Set 0x68 and clear 0x80 | Clear 0x88, set 0x60 | Same | Readback |
| E056 | Set 0x30 in high nibble of low byte | Clear 0xf0 | Same | Readback |
| D430 | Set low 12 bits to 0x47f | 0x45f | Same | Readback |
| TDFNR 0x57 | Omitted | Byte 0x10 | Same | Host MMIO model |
| 0xd0 | Omitted | Set 0xc0 with DASH absent. Method 5 has DASH support version zero | Preserve low bits and set 0xc0 | Host MMIO model |
| C0B4 | Set 0xc | Clear then set MCU enable and set 0xc | Clear bit 0, set bit 0 and 0xc | Final readback |
| C0B6 | Omitted | Preserve magic-packet bit only | Clear 15:1 | Source |
| EA84 | Omitted | Set bits 1:0 for tally version 1 | Same | Readback |
| E080 | Omitted | Clear EEE+ bit 1 in hardware config | Same | Readback |
| C0AC | Set 0x1f80 | Set 0x1f80 | Existing operation kept | Source |
| EB58 / E614 | v3: bit 0 and 0x200 respectively | Same | Existing v3 values kept | Readback |
| TxConfig and RxConfig | Classic TxConfig and both modes' RxConfig after engine activation | Configure before engine activation | Both modes program and verify before ChipCmd | Host ordering model |

The sources for the table are FriendlyARM lines 14779–14847 (DASH), 15072 (MCU support), 15226 (extended tally support), 5322–5345 (L1 and tally fields), and 18245–18538 (hardware configuration and activation). These are software expectations. Hardware readbacks will come from the candidate boot. [Hardware configuration](https://github.com/friendlyarm/r8125/blob/3fda70ebcba22f281ec4e3a69c355992c8823fc0/r8125_n.c#L18245)

## Initialization and TX

FriendlyARM rtl8125_up at line 14515 calls hardware init, reset, PLL power-up, EPHY, PHY, then hardware config. Link-up runs hardware config again before ChipCmd in rtl8125_link_on_patch at line 4950. The native path resets and exits OOB, applies pinned firmware and configures PHY, resets the MAC again, configures MAC and queues, builds DMA rings, verifies bases and PCI mastering, then configures TX/RX registers and enables the engines. TX uses ordered publication before the doorbell. Changing firmware or PHY order requires a controlled experiment because the driver retains a Linux r8169-derived PHY sequence. [Startup reference](https://github.com/friendlyarm/r8125/blob/3fda70ebcba22f281ec4e3a69c355992c8823fc0/r8125_n.c#L14515)

| Other path | Classification |
|---|---|
| PHY/EPHY and MCU firmware | The ten B51 EPHY writes match FriendlyARM method 5 at lines 8732–8742. The native external firmware interpreter and PHY sequence differ from optional vendor embedded firmware and tunables. Ubuntu firmware build selection is unverified. |
| Reset, OOB and power | Native bounded reset implements stop, FIFO/mitigation checks and reset completion. FriendlyARM also applies wake/PME, CSI/AER mask and ASPM/CLKREQ policies before start. Those platform and power policies are outside these corrections. |
| PCIe | Endpoint and root mastering readbacks are retained. Native entry latency is 0x27. MRRS, MPS, ASPM and CLKREQ require live comparison. The reference MRRS write is conditional at lines 18074–18091. |
| RX filters and IRQ | Single queue, no RSS, VLAN/offloads or MSI-X service. Existing TCAM enable and clear are present. Polling and the TX-only phase intentionally differ from Linux. |
| TX format | FriendlyARM's TxDesc is always 32 bytes independent of No Close, with options at 0/4, 64-bit DMA address at 8 and zeroed reserved words at 16–31. Native v3 matches it. Native classic remains the r8169-derived 16-byte OWN/TxPoll control. |
| TX progression | Method 5 selects v3. Queue 0 tail/close use 16-bit 0x2800/0x2802, count descriptors modulo 65536 and index a 64-slot ring separately. Classic uses 16-bit TxPoll 0x90 and OWN completion. One slot may be outstanding in the native v3 path. |
| Memory ordering | Payload and descriptor body precede OWN, OWN precedes notification, and completion reads are ordered. The native pool uses the pinned Normal-NC mapping. Linux uses its DMA allocation/map APIs and DMA barriers. |
| PCIe DMA | The previous address audit still applies. Inbound region 0 translates 0x80000000–0x800fffff to RAM starting at 0x24200000. Code checks region fields and enable. Effective board routing and overlap with any other enabled inbound region remain unverified. |

The updated source is in [B51 setup and tally](../pds/pcie_provider/src/b51.rs). Descriptor handling and DMA arithmetic retain the audited [DMA implementation](../drivers/rtl8125/dma.rs). [Descriptor definition](https://github.com/friendlyarm/r8125/blob/3fda70ebcba22f281ec4e3a69c355992c8823fc0/r8125.h#L2044) · [Doorbell and submit](https://github.com/friendlyarm/r8125/blob/3fda70ebcba22f281ec4e3a69c355992c8823fc0/r8125_n.c#L19917) · [Existing DMA address table and investigation](rtl8125-b51-root-cause.md)

## Independent tally commands

One broadcast precedes the RX comparison. The RX-off attempt preserves the first result and counter words. With a valid private ring, the second phase enables RX, verifies ChipCmd, waits a bounded interval for the old command to become idle, refills the guarded page and submits a fresh command. If the old command stays busy, SYS 30 records that the second command could not run safely. The page is never reinitialized while that DMA command is pending. Words 19–22 retain ChipCmd and outcome for each phase. Words 23–24 retain RX-off TX counter words and 17–18 retain RX-on words. SYS codes retain their existing meanings. [FriendlyARM tally operation](https://github.com/friendlyarm/r8125/blob/3fda70ebcba22f281ec4e3a69c355992c8823fc0/r8125_n.c#L752)

Host coverage checks mode, register readback, inactive engines during configuration, ring layout, tally guards, separate command submission, and suppression of a second command while the first remains busy. Board acceptance requires the expected ARP broadcast on a directly connected peer.

The pinned Makefile defaults external firmware file support to off at line 49. The installed module's empty firmware-version field is consistent with that default, although it does not identify the actual loaded MCU bytes. Preserve the existing verified firmware checksum until the installed module or a live register comparison provides a basis for changing it. [Build options](https://github.com/friendlyarm/r8125/blob/3fda70ebcba22f281ec4e3a69c355992c8823fc0/Makefile#L49) · [Method-5 PHY setup](https://github.com/friendlyarm/r8125/blob/3fda70ebcba22f281ec4e3a69c355992c8823fc0/r8125_n.c#L13778)

The diagnostic profile is selected with R5S_LAN1_TX_NO_CLOSE=1 and R5S_LAN1_TX_DIAGNOSTIC=1 on the existing LAN1-only build entry point. The prior image is the exact comparison control. The classic build remains selectable with the No Close flag omitted. The new v3 register settings also apply when v3 is explicitly selected in a service build. This investigation ships the diagnostic image only.
