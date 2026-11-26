# Current delivery and evidence

User-observed prior results: intended Rust startup, baseline-v2 READY and GMAC
link/cable/cold-start success. These are user reports, not independent lab tests.
No new physical test, SSH, MMIO access, SD write, shutdown or reboot was performed.
No native packet exchange, DHCP, routed Internet or latest controlled recovery
result is inferred. All old image/lock hashes were rechecked unchanged.

| Stage | Coded | Built | Host-tested | Simulation | Board-tested | Client Internet |
|---|---|---|---|---|---|---|
| Historical baseline/link | yes | preserved | prior + retained regressions | none claimed | user observed | no |
| A cache preflight | real Rust IPC/VSpace calls | yes | layout/bindings/negative profiles | no | pending | no |
| A native GMAC rawio | real MAC/MTL/DMA + bounded Rust queues/probe | yes | separate CPU/device-cache model, compiled C/Rust | no | pending | no |
| B WAN DHCP/IPoE | real smoltcp client, ACD, ARP, finite gateway probes | yes | parsers/config/core; not full adapter execution | no | pending | no |
| C RTL8125 LAN1 | candidate/source/evidence audit only | absent | no native backend | no | no | no |
| D LAN1 router | host NAPT/DHCP server core, incomplete | absent | packet/time harness | no | no | no |
| E dual LAN router | no integrated datapath | absent | realm isolation core only | no | no | no |

Built images, all100663296bytes:

| File | SHA-256 |
|---|---|
| r5s-gmac-dma-preflight.img | 1a6567485cffe5b7998aea8957bce66cc260488d282271dc0cef102270588411 |
| r5s-gmac-rawio.img | 360902adc52e02c810cdd81a41c8a188093bf40037aaf7e4904932a97dfc7e22 |
| r5s-wan-dhcp.img | 5f3ca926cce92465baf14346be32188e3945173402f29b2f50376a79c9b3bf51 |

Each final payload has its own size/hash embedded in the corresponding U-Boot
expectations and validated FAT manifest. Two final GPT/FIT/FAT image assemblies
per profile matched byte-for-byte. This is assembly reproducibility using the
same built inputs, not two independent toolchain/ELF builds or cross-host proof.
Preliminary candidates are retained separately and excluded from handoff.

Validation run:75 Python tests (including all69 prior tests),10 Rust router-core
packet/time tests, one compiled actual-queue Rust test with8000 transfers plus
wrap/epoch/corrupt cases, and the actual C DMA backend under ASan/UBSan with6300
TX/2000 RX model completions. The Python suite also compiles/runs that backend
under UBSan. Core parser cases include1600 deterministic generated inputs;
this is a bounded corpus, not coverage-guided fuzzing. No QEMU, TAP, namespaces,
AF_PACKET peer, native syscalls or physical DMA was executed on a board.

The final core checks related-ICMP quote lengths/flags/UDP length and bounded
TCP sequence, private WAN acceptance, two LAN clients sharing a source port,
address/port dependent reverse filtering, checksums, echo identifiers, TCP
handshake/retransmit/FIN, expiry/generation, quotas, DHCP role/probe/release/DNS,
ARP probing/conflicts/DECLINE and malformed/fragment rejection. Full router
fragmentation/PMTU and native LAN adapters remain unfinished.

Build from the existing prepared checkout, never a replacement project:

```sh
. scripts/env
python3 scripts/build-gmac-rawio
python3 scripts/build-router
python3 -m unittest discover -s tests
cargo test --manifest-path router-core/Cargo.toml
rustc --edition 2021 --test tests/gmac_queue_test.rs -o out/gmac-rawio/queue-test
out/gmac-rawio/queue-test
python3 scripts/package-router
```

The builders use locked local SDK/source trees and offline Cargo. New .crate
archives/checksums and reference licenses are retained. Toolchain/source caches
from the existing checkout are still required; this operator bundle does not
pretend to contain an independently bootstrapped compiler or every vendor tree.
Source snapshots, generated system/CapDL/layout reports, ELF load/section maps,
object disassembly, U-Boot link maps/config deltas and source hashes are included.

Concrete blockers to the product target: native PCIe host/PHY/iATU/BAR setup and
RTL8125 method-specific initialization/DMA are not implemented; the host router
core lacks complete fragments/PMTU/local errors and a LAN-facing service adapter.
No missing broad inventory is asserted as an excuse. Existing evidence is
resolved in pcie-evidence.json. Completing these source adaptations, then the
ordered board gates, remains necessary. No placeholder final images are shipped.

Diagnostics-v2 is unchanged:500ms all-OFF transition delimiter; WAIT2s/2s,
READY200ms/3800ms, IO_VERIFIED2500ms/500ms/200ms/2800ms, FAULT2s of125ms flutter
then4s darkness. Fault selection has priority. Lease expiry is5s and service
freshness100ms, plus10ms scheduled display poll bound when execution functions.
Partial frames and static LED states are inconclusive; kernel failure may freeze
SYS. Only raw test mode uses IO_VERIFIED; WAN lease/traffic never manufactures it.

The first operator image is `r5s-gmac-dma-preflight.img`, then rawio only after
preflight/recovery; WAN only after genuine fresh external exchange/recovery.
See router-operator.md for exact transfer, guarded write, readback and recovery.
Drivers remain trusted with ARM_SMMU disabled and no demonstrated effective DMA
confinement. This is not a verified, hardened, line-rate or completed router.
