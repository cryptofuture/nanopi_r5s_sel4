# Hardware TCP Segmentation Offload capability audit

This records the TSO capability evidence for GMAC0 (Rockchip EQoS) and the
RTL8125 B51 LANs, verified against pinned reference sources. Nothing here is
physical board validation; runtime capability is additionally probed from the
hardware at startup (GMAC `MAC_HW_FEATURE1`, RTL8125 chip identity) and the
feature is disabled automatically when the probe fails.

## GMAC0 — Synopsys DWMAC 4.20a (RK3568 EQoS)

Board identity (see `gmac-hardware-audit.md`): MAC at `0xfe2a0000`,
compatibles `rk3568-gmac, dwmac-4.20a`. The native driver reads
`MAC_VERSION` (`0x110`) at startup and admits revisions `0x40/0x41/0x50/
0x51/0x52` (`drivers/gmac0/link.c`); unknown revisions fault. The exact
silicon revision of the tested board is a runtime value, not inferred from
the compatible string.

### Capability bit

TSO capability is `MAC_HW_FEATURE1` (`0x0120`) bit 18, `GMAC_HW_TSOEN`
(Linux `drivers/net/ethernet/stmicro/stmmac/dwmac4.h`, master). The driver
already reads `0x0120` in `dma_start()` (TX/RX FIFO sizes, address width),
so the probe is one additional bit test. TX checksum offload is
`MAC_HW_FEATURE0` (`0x011c`) bit 14 (`TXCOESEL`), already probed.

Whether RK3568 silicon actually sets `TSOEN` is established only by the
runtime probe; the driver never assumes it.

### Descriptor format (Linux `dwmac4_descs.c`, `stmmac_main.c`)

On a recognized revision with `TSOEN`, the driver sets and verifies
`DMA_CHAN_TX_CONTROL` (`0x1104`) bit 12 before starting the channel.
A missing capability or rejected enable keeps software segmentation available.
Each TSO request still uses the per-descriptor TSE bit and
**normal 4-word descriptors**, the same format the driver uses today:

[STMMAC channel enable](https://github.com/torvalds/linux/blob/v6.12/drivers/net/ethernet/stmicro/stmmac/dwmac4_dma.c#L449-L465)

1. **Context descriptor** (only when the MSS changes; this project writes one
   per TSO request for simplicity):
   - TDES0 = 0, TDES1 = 0, TDES2 = MSS,
   - TDES3 = `TDES3_CONTEXT_TYPE (bit 30) | TDES3_CTXT_TCMSSV (bit 26)`,
   - OWN set last, after the data descriptors are written.
2. **First data descriptor** (headers):
   - TDES0 = buffer address of the header region,
   - TDES2 = `proto_hdr_len` in `TDES2_BUFFER1_SIZE_MASK` (bits 13:0),
   - TDES3 = `OWN | FD | TSE (bit 18) | (tcp_hdr_len/4) << 19 |
     (total_tcp_payload & 0x3ffff)`,
   - `tcp_hdr_len/4` occupies the `TDES3_SLOT_NUMBER_MASK` field (bits 22:19),
     i.e. the TCP header length in 32-bit words (5..15),
   - bits 17:0 (`TDES3_TCP_PKT_PAYLOAD_MASK`) hold the **total** TCP payload
     byte count of the whole superframe (18-bit field).
3. **Payload descriptors** (stmmac `stmmac_tso_allocator`): one descriptor per
   up-to-16383-byte chunk (`TSO_MAX_BUFF_SIZE = SZ_16K-1`), TDES0 = chunk
   address, TDES2 = chunk size in bits 13:0, TDES3 = `OWN` (+ `LD` on the
   descriptor holding the final byte). FD/TSE/payload fields only on the first
   descriptor.

The hardware segments the superframe into MSS-sized TCP segments, recomputes
IP/TCP checksums per segment, increments IPv4 ID and TCP sequence numbers.

### Validity limits (Linux `stmmac_tso_valid_packet`, dwmac 4.2/5.3)

- header length (Ethernet+IP+TCP) ≤ 1023 bytes (TDES2 documented limit),
- MSS ≥ 64 (ETH_DMACxCR documented recommendation),
- header length + MSS < 16383,
- per-descriptor buffer size ≤ 16383 (`TDES2_BUFFER1_SIZE_MASK` = 14 bits),
- total TCP payload ≤ 262143 (18-bit TDES3 field). This project caps TSO
  requests at `LSO_64K` = 64000 bytes total (headers+payload) — the largest
  limit proven by the RTL8125 board driver and safely inside every GMAC
  field.

IPv4 and IPv6 both work through the same TSO descriptors (stmmac passes
SKB_GSO_TCPV6 through `stmmac_tso_xmit` unchanged).

Reference: Linux master `drivers/net/ethernet/stmicro/stmmac/dwmac4_descs.h`
(TDES3 `TDES3_TCP_SEGMENTATION_ENABLE BIT(18)`, `TDES3_HDR_LEN_SHIFT 19`,
`TDES3_TCP_PKT_PAYLOAD_MASK GENMASK(17,0)`, `TDES3_CTXT_TCMSSV BIT(26)`),
`dwmac4_descs.c::dwmac4_rd_prepare_tso_tx_desc`, `stmmac_main.c::
stmmac_tso_xmit/stmmac_tso_allocator/stmmac_tso_valid_packet`. The same bit
definitions are pinned in-tree at `drivers/gmac0/reference/dwmac4_descs.h`.

## RTL8125 B51 (LAN1/LAN2)

Board identity: PCI `10ec:8125`, revision `0x05`, TxConfig signature
`0x64100000` → `Method::B51` (`drivers/rtl8125/ring.rs`). This is the exact
branch below; A48/A49/B50 are out of scope.

### Capability

TSO is implemented for RTL8125 by all three reference drivers:

- FriendlyARM board driver `9.016.01-NAPI-g3fda70e` (the module reported on
  this board; commit `3fda70ebcba22f281ec4e3a69c355992c8823fc0`):
  `rtl8125_tso_csum()` (`r8125_n.c:19730`), `NETIF_F_SG|NETIF_F_TSO|NETIF_F_TSO6`,
  `netif_set_tso_max_size(dev, LSO_64K)` (64000), `netif_set_tso_max_segs(64)`.
  TSO runs together with `EnableTxNoClose=0x1` / `HwSuppTxNoCloseVer=0x3` —
  the same no-close tail/close pipeline the native driver uses.
- Realtek vendor driver `7a5b9713043cef55ebb8433e722141e6ba2ab6fd`
  (`RealtekNIC/Realtek-RTL8125-Series-driver`): same `rtl8125_tso_csum`.
- Linux upstream `r8169_main.c` (master): `rtl8169_tso_csum_v2` for
  RTL8125 (`RTL_GIGA_MAC_VER_63`), `NETIF_F_TSO|NETIF_F_TSO6` with
  `RTL_GSO_MAX_SIZE_V2`.

`RequireLSOPatch` (last-fragment byte trim) applies only to
`CFG_METHOD_2/3/6`, **not** CFG_METHOD_5 (B51), so no LSO patch is required
on this board.

### Descriptor format (B51 "new" 32-byte TX format, OCP `0xeb58` bit 0)

The native driver already uses this format (`EXTENDED_TX`, 32-byte stride).
TSO fills the existing words; the reserved words 4..7 stay zero.

opts1 (word 0), from `r8125.h::_DescStatusBit` + `rtl8125_tso_csum`:

| bits | meaning |
|---|---|
| 31 | DescOwn |
| 30 | RingEnd (last descriptor of the ring only) |
| 29 | FirstFrag (first descriptor of the packet) |
| 28 | LastFrag (last descriptor of the packet) |
| 26 | `GiantSendv4` — IPv4 TSO request |
| 25 | `GiantSendv6` — IPv6 TSO request |
| 24:18 | `GTTCPHO` = L4 (TCP) header offset from frame start, **even**, ≤ 0x70 |
| 15:0 | this descriptor's byte length |

opts2 (word 1): MSS at bits 28:18 (`TD1_MSS_SHIFT`), 11 bits, max `0x7ff`.
The non-TSO checksum bits (29/30/31/28 — `TxIPCS_C/TxTCPCS_C/TxUDPCS_C/
TxIPV6F_C`) keep their existing meaning and are unused for TSO requests
(hardware inserts IP+TCP checksums for GiantSend frames).

Every descriptor of a TSO frame carries `GiantSendv4/6` + `GTTCPHO` + MSS;
the first adds `FirstFrag`, the last adds `LastFrag` (vendor
`rtl8125_start_xmit` + `rtl8125_xmit_frags`). The FIRST/LAST bits for the
V3 *RX* format (`FirstFrag_V3/LastFrag_V3`, bits 25/24) do not apply to TX.

### Limits (FriendlyARM `r8125.h`, `r8125_n.c`)

- MSS: 1..`MSS_MAX` (0x7ff = 2047).
- L4 offset: ≤ `GTTCPHO_MAX` = 0x70 (112) bytes and even
  (Ethernet 14 + IPv4 20 + TCP 20..60 = 54..94; IPv6 = 74..114 — the
  0x70 cap rejects IPv6 TCP headers longer than 18 bytes).
- Total TSO size: `LSO_64K` = 64000 bytes (driver policy; hardware
  `GTPKTSIZE_MAX` = 0x3ffff).
- Fragment count: ≤ 64 (`NIC_MAX_PHYS_BUF_COUNT_LSO2`).
- Per-descriptor length field: 16 bits; this project keeps ordinary
  per-fragment lengths and rejects fragments > 65535.

### 63-outstanding no-close pipeline

Unchanged: 64-slot ring, one slot kept free (`tx_inflight() < N-1`),
u16 tail (`0x2800`) / close (`0x2802`) pointers with wraparound, count-based
`reclaim_tx`. A TSO request reserves its descriptor count atomically (all
slots checked free before the first publish), preserving in-order
reservation/wraparound and the 63-outstanding cap. Partial completion and
reclamation follow the existing count-based close-pointer path; a request is
complete when its final descriptor is retired.

## Runtime capability status

| NIC | Probe | TSO field |
|---|---|---|
| GMAC0 | `MAC_HW_FEATURE1 (0x120) bit 18` (`GMAC_HW_TSOEN`) at `dma_start` | reported by `dma_tso_capability_read` |
| LAN1/LAN2 | `Identity::supported_method` = B51 at probe | reported per port |

Build flags `wan_tso` / `lan1_tso` / `lan2_tso` request the feature; a NIC
whose probe fails runs with TSO disabled and reports
`supported=0`. Requests then take the software segmentation fallback (or an
explicit error), never an oversized frame.

## Host-verified vs hardware-validated

Everything in this document is source-verified against the pinned drivers
above and exercised by host tests (descriptor encoding, validation rules,
software segmentation and checksums). Actual wire-level segmentation
(MTU-respecting frames, correct TCP sequence numbers and checksums under
packet capture) is specified in `docs/tso-hardware-validation.md` and has
not been performed on a physical R5S by this work.
