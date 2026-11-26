# RTL8125 B51 Repair Progress

This records the CPlusCmd candidate. See the [one-shot diagnostic procedure](rtl8125-b51-physical-validation.md) for the current candidate.

## Exact sources

- Repository: `32a898a0270a880402a32789937836dfa37e3476`.
- Linux upstream: `torvalds/linux` v6.12, commit `adc218676eef25575469234709c2d87185ca223a`.
- Realtek vendor: `RealtekNIC/Realtek-RTL8125-Series-driver`, commit `7a5b9713043cef55ebb8433e722141e6ba2ab6fd`.

## Completed

- Repository state inspected: clean tree before this repair.
- Actual Rust driver files identified under `drivers/rtl8125/` and `pds/pcie_provider/`.
- Reference sources checked out locally for pinned comparison.
- Exact B51 identity confirmed from the local test model and source classifier: PCI ID `0x8125:10ec`, revision `0x05`, TxConfig `0x64100000`.

## Confirmed differences

- CPlusCmd was masked with `0x2003`, while Linux's `CPCMD_MASK` is `0x2063`; the code discarded `RxVlan` and `RxChkSum` and then rejected those bits on readback. Corrected to preserve and verify `0x2063`.
- TxConfig `0x03000700`, RxConfig `0x40000f0a`, ring-base ordering, descriptor ownership, DMA barriers and `TxPoll_8125=0x90` match the B51 Linux paths after source comparison.
- Linux v6.12 writes RSS control `0x4500` as 8-bit while the vendor writes it as 32-bit; the Rust code does both. This remains a documented unresolved reference difference.

## Changes implemented

- `pds/pcie_provider/src/b51.rs`: CPlusCmd mask/readback correction.
- `tests/lan1_bringup.rs`: verifies that the mask preserves supported bits and clears an unsupported bit, and that failed readback blocks bus mastering.
- `docs/rtl8125-b51-upstream-comparison.md`: completed call-graph and register comparison.

## Tests executed

- `bash scripts/test-lan1-bringup`: 61 test executions passed across five configurations.
- `bash scripts/test-router-integrated`: router, PCIe, network health and sanitized GMAC DMA tests passed.

## Diagnostic candidate

- Two independent `scripts/build-lan1-bringup` builds used `--lan-repair=lan1` with `R5S_LAN1_TX_SELF_TEST=1`. Both passed image verification and matched byte for byte.
- Image SHA-256: `808dcff4d717e8c71d618482ac8c9b287d1baf739955284e644c30ff08ddc509`.
- The WAN GMAC driver, DHCP app and health binaries match the previous LAN1 DMA diagnostic. The image manifest confirms LAN2 is disabled.
- Guarded handoff: `out/router-rtl8125-b51-cplus/candidate.kDLXGbWC/r5s-rtl8125-b51-cplus-handoff.tar.gz`. The archive includes the image, verification and flash scripts, source patch, test log and operator instructions.

## Remaining work

- Resolve the `0x4500` access-width disagreement only with a hardware register-width source or readback evidence.
- Run the WAN + LAN1 board trial and capture the LAN1 startup announcements, ARP reply and DHCP exchange at the peer.

