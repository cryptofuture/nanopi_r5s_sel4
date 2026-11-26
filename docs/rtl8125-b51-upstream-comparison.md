# RTL8125 B51 Upstream Comparison

This records the initial, Linux v6.12 comparison. The current [Linux and Realtek comparison](rtl8125-b51-linux-comparison.md) resolves the Rust RSS access policy using newer upstream source. Physical TX still needs a board trial.

## Pinned references

- Repository under test: `32a898a0270a880402a32789937836dfa37e3476`.
- Linux `torvalds/linux` v6.12 checkout: `adc218676eef25575469234709c2d87185ca223a` (tag `v6.12`).
- Realtek RTL8125 driver checkout: `7a5b9713043cef55ebb8433e722141e6ba2ab6fd` (`RealtekNIC/Realtek-RTL8125-Series-driver`, `main`).

## Hardware identity

The repository identifies the tested endpoint as RTL8125 PCI ID `0x8125:10ec`, revision `0x05`, and TxConfig `0x64100000`. The local method classifier maps that signature to `Method::B51`, and `Identity::supported_method` additionally requires revision `0x05`. This is the exact branch under comparison. A48, A49 and B50 paths are outside the supported runtime identity.

## Current trace status

The Linux call graph has been located through `rtl_probe`, `rtl_init_one`, `rtl_open`, `rtl_hw_start`, `rtl_hw_config`, `rtl_hw_start_8125`, `rtl_hw_start_8125b`, `rtl_hw_start_8125_common`, `rtl_set_rx_tx_desc_registers`, `rtl_init_rxcfg`, `rtl_set_tx_config_registers`, `rtl8169_start_xmit`, `rtl8169_doorbell`, `rtl8169_mark_to_asic`, `rtl8169_rx_interrupt` and `rtl8169_reset_work`. The vendor call graph has been located through `rtl8125_init_software_variable`, `rtl8125_exit_oob`, `rtl8125_powerup_pll`, `rtl8125_hw_phy_config`, `rtl8125_hw_start`, `rtl8125_init_ring`, `rtl8125_start_xmit` and `rtl8125_tx_interrupt`.

## Register comparison

| Register or operation | Linux upstream | Realtek vendor | Current Rust | Difference | Severity |
|---|---|---|---|---|---|
| TxPoll 0x90 | `RTL_W16(TxPoll_8125, BIT(0))` in `rtl8169_doorbell` | 16-bit queue poll in TX submit path | `write16(BAR + 0x90, 1)` after `ordered()` | Match in width and value; ordering is still under review | None found so far |
| CPlusCmd 0xe0 | `CPCMD_MASK = Normal_mode | RxVlan | RxChkSum | INTT_MASK = 0x2063`; `rtl_init_one` retains that mask and `rtl8169_set_features` writes 16 bits (`r8169_main.c:443-460`, `5444`, `1712-1733`) | `rtl8125_hw_config` clears only debug/force/ASF bits then writes 16-bit `CPlusCmd` (`src/r8125.h:1697-1713`, `src/r8125_n.c:18400-18408`) | Was `cplus & 0x2003`, dropping `RxVlan|RxChkSum`; now `cplus & 0x2063`, with readback compared against that mask | Confirmed implementation defect: supported operating and RX feature bits were discarded | High for RX correctness; not a proven explanation for TX-only failure |
| TX/RX ring bases | `TxDescStartAddrHigh=0x24`, low=0x20, `RxDescAddrHigh=0xe8`, low=0xe4` in `rtl_set_rx_tx_desc_registers` | Vendor ring setup programs equivalent Q0 base registers | Writes high then low for each ring, readback each dword | Match in register pair; ordering/readback stricter | None found so far |
| ChipCmd 0x37 | `CmdTxEnb | CmdRxEnb` after descriptor/config programming | Vendor enables TX/RX in `rtl8125_hw_start` after setup | Writes `0x37=0x0c` after ring/config programming | Match in value; full setup ordering is under review | None found so far |
| TxConfig 0x40 | DMA burst and interframe gap fields | Vendor method-specific TX config writes | Writes `0x03000700`, checks masked readback | Matches Linux B51 startup | None found |
| RxConfig 0x44 | RTL8125B fetch, DMA burst and pause-slot fields plus the RX filter | Vendor method-specific RX config writes | Writes `0x40000f0a`, checks masked readback | Matches the Linux B51 fields with broadcast and station-address reception. RTL8125 VLAN stripping is controlled separately by RxConfig bits 22 and 23, which stay clear. | None found |

## Completed source comparisons

- Linux v6.12 `r8169_main.c` register definitions and B51 hardware-start path inspected.
- Realtek vendor `src/r8125_n.c` and `src/r8125.h` firmware, MAC OCP, PHY OCP, ring and TX paths inspected.

## Confirmed differences

1. **CPlusCmd mask.** Linux defines `CPCMD_MASK` as `0x2063` and preserves `Normal_mode`, `RxVlan`, `RxChkSum`, and `INTT_MASK`. The Rust path used `0x2003` and then required bits `0x60` to read zero. This silently disabled RX VLAN and checksum modes that upstream preserves. The Rust path now uses `0x2063` and validates the programmed mask. The change is B51-safe because the value is a read-modify-write of the live register, and it preserves the same mode bits upstream retains. It does not claim to restore physical TX by itself.

2. **TxConfig/RxConfig values match the B51 branch.** Linux `rtl_set_tx_config_registers` produces `0x03000700` for this MAC version because `rtl_is_8168evl_up` excludes versions above 53. Linux `rtl_init_rxcfg` for RTL8125B produces `RX_FETCH_DFLT_8125 | RX_DMA_BURST | RX_PAUSE_SLOT_ON`, then `rtl_set_rx_mode` adds `AcceptBroadcast|AcceptMyPhys`, yielding `0x40000f0a` for the diagnostic filter. The current values are intentional and match these paths.

3. **Ring bases, ownership, and doorbell match.** Linux `rtl_set_rx_tx_desc_registers` writes `0x24/0x20` and `0xe8/0xe4`; the Rust path writes the same high/low pairs and readbacks them. Linux `rtl8169_start_xmit` publishes address/options before `opts1` with `wmb()`, sets `DescOwn|FirstFrag|LastFrag`, and calls `rtl8169_doorbell`; the Rust `PackedDma::publish` and `Ring::send_frame` do the same with a device barrier and `TxPoll_8125` 16-bit write of bit 0.

4. **Register width divergence left unresolved.** Linux v6.12 `rtl_hw_start_8125_common` writes `0x4500` with an 8-bit store (`r8169_main.c:3705-3712`), while the vendor `rtl8125_hw_config` writes the same RSS control register with a 32-bit store (`src/r8125_n.c:18220-18224`). The Rust code performs an 8-bit write during MAC setup and a 32-bit zero write/readback later. The two references disagree, so this is recorded as a potentially relevant difference rather than changed blindly.

## Implementation

- `pds/pcie_provider/src/b51.rs`: preserve the upstream `CPCMD_MASK` and validate the programmed value.
- `tests/lan1_bringup.rs`: the activation model verifies that the driver preserves `0x2063` and clears an out-of-mask bit. A lost in-mask bit must fail initialization before bus mastering.

