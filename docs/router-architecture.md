# Product objective and promotion gates

The staged NanoPi R5S shared-LAN profiles attach LAN1 and LAN2 to one local Ethernet service with one DHCP lease pool at 10.44.1.0/24. WAN remains an independent DHCP client. This profile has its own deployment and physical acceptance sequence. [Shared-LAN bring-up report](../out/r5s-dual-lan-bringup-report.md)

WAN obtains its IPv4 address, mask, gateway and DNS from upstream DHCP over
Ethernet (IPoE). Each LAN runs its own DHCP server for local clients and shares
WAN Internet access through native NAT. The two LAN subnets stay separate.

Current objective: standalone native seL4/Microkit IPv4 router, DHCP WAN on GMAC0,
separate 10.44.1.0/24 and 10.44.2.0/24 LANs on independently enumerated RTL8125
controllers, DHCP servers, native endpoint-preserving NAPT and fixed policy.
Linux clients are external computers. Recovery Ubuntu remains on eMMC and is
not a concurrent guest or packet backend. Historical boot/firewall tasks are
records, not the present product contract.

A: GMAC cache/DMA preflight and external raw challenge/response/ACK, then recovery.
B: native WAN DHCP/ARP/external IPv4. C: RTL8125 LAN1 cold-start/raw frames/ring wrap.
D: single-LAN DHCP/DNS/native NAT and HTTPS. E: concurrent dual LAN and isolation.
No later-stage image is a substitute for missing earlier hardware evidence.

Driver PDs own narrowly mapped MMIO and DMA pools; Rust services own protocol
parsing and router state. Bounded per-port queues carry authoritative ingress
identity from the mapping/channel, never a packet-supplied port label. One
supervisor owns SYS and checks essential progress with bounded reads, not IPC
that a dead driver can block. Services have explicit MCS budgets on CPU 0.

LAN1 and LAN2 are routed, not bridged. Unsolicited WAN and all inter-LAN traffic
are denied. Private WAN addresses are valid. Unsupported protocols are dropped;
no general rule language, inbound pinholes, UPnP or proxy termination of client
TCP/TLS is included. Configuration and lease generations invalidate stale
neighbors, flows, fragments and queued packets together. LAN local service stays
available during WAN outage; vacant LAN sockets are not faults.

Drivers remain trusted: ARM_SMMU is disabled and effective DMA isolation has not
been demonstrated. No formal verification, production security or performance
claim follows from these builds. Diagnostics-v2 rhythms remain exact; ordinary
router traffic never drives IO_VERIFIED. New router images must not contain the
raw bench reflector on WAN.

The current delivery implements A's cache/DMA/raw service and B's native DHCP
client. Both are offline-qualified and await staged hardware results. The
allocation-free router core is exercised on the host; no LAN driver or complete
router is represented by those tests. See [operator procedure](router-operator.md),
[DMA audit](router-dma.md), [protocol limits](router-protocols.md) and
[results](router-results.md). The first new operator image is the DMA-disabled
cache preflight, not the already-tested unchanged baseline.
