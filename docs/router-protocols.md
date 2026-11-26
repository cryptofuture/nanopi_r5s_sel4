# Protocol implementation and remaining gates

`router-core` is an allocation-free native Rust library with 128 fixed contact
records and 100 RAM-only DHCP leases per LAN. Its packet/time harness runs the
same parser, checksum, NAT and DHCP-server code intended for the future PD.
It is not wired to nonexistent RTL8125 services and is not a complete router.

Implemented host paths: authoritative WAN/LAN1/LAN2 ingress enum, strict bounded
IPv4/Ethernet helpers, TTL/MTU rejection, source-prefix enforcement, inter-LAN
and unsolicited inbound drops, TCP/UDP NAPT, ICMP echo-ID translation, related
ICMP quote translation, WAN lease-generation invalidation and per-client release.
Checksums cover odd lengths and IPv4 UDP-zero semantics. Client TCP/TLS remains
end-to-end. NAT mapping is endpoint-independent while return filtering requires
the contacted address and port. Related ICMP may come from an intermediate
router; its quoted tuple must match existing state and bounded quote checks.
Invalid inbound packets never allocate a flow or refresh it.

Lookup and port selection are bounded linear scans. Quotas are16 contacts per
client and128 total, port range40000-59999, LCG-seeded allocation with a bounded
retry count. This is not cryptographic port randomization. Local services must
continue reserving ports outside this range. Exhaustion refuses new work rather
than evicting live sessions. Expired/generation-revoked mappings retain120s
reuse tombstones. UDP idle120s, ICMP60s, TCP setup absolute30s, established2h,
closing120s. TCP validation uses a conservative1MiB sequence/reordering window,
not a complete RFC5382 implementation; high-window compatibility is unproven.

LAN server cores implement probe-before-offer, offer/request/ACK/NAK,
release/decline quarantine, expiry and INFORM. They require an explicit LAN role;
WAN cannot create a server. They use client ID plus chaddr as a strict combined
identity, so client-ID mobility and full INIT-REBOOT behavior need further work.
The future service must implement actual ARP probing, broadcast/unicast delivery,
DNS/config refresh and generation coordination. No LAN service is shipped.

The WAN image instead integrates pinned smoltcp0.12.0 (0BSD, MSRV1.80) with the
real bounded driver queues. Default features are disabled: only Ethernet,
IPv4, DHCPv4 and ICMP sockets are enabled. No hosted TAP backend, Linux stack,
allocator or Linux NAT is involved. Cargo.lock retains previous dependencies;
new crate archives/checksums are retained in dependency-cache. Smoltcp handles
client discovery/request, retries, renewal/rebinding, NAK and expiry. A strict
outer parser rejects duplicate/malformed DHCP options, invalid timers, wrong
client MAC, relays and unsupported overload. Server/xid matching is performed
by the pinned client. Address/mask/gateway/DNS must pass `Wan::validate`; private
WAN is valid, overlap with either LAN is rejected. Only /1 through /30, finite
4s through7day leases and one to three usable DNS addresses are accepted.

On binding, the app requests driver quiescence/epoch advance, clears neighbors
and routes, performs three ARP probes, then binds and announces twice. Conflict
revokes the address, sends bounded DHCPDECLINE and backs off10s. No stale address
survives lease expiry or physical epoch loss. Same-config renewal preserves the
address; configuration changes flush old queues. The transaction seed uses the
live architectural counter, not a deployed deterministic test seed. This is
boot variability, not cryptographic entropy or DHCP authentication.

The source-bound `config/router-v1.json` fixes role assignments, unique local
unicast MACs, routed subnets/pools and policy. The validator rejects any change
except up to three explicit DNS overrides. To change an override, regenerate
`pds/wan_dhcp/src/config.rs` with `router_config.dns_source`, then rebuild.
The builder checks exact generated contents and integrity-binds the JSON in the
manifest/source identity. No mutable FAT configuration is executed.

The WAN app responds to required ARP and consumes DHCP/selected ICMP replies.
It exposes no DHCP server, ping responder, SSH, administration, NAT or raw bench
reflector on WAN. READY requires live essential services, a validated current
lease and fresh gateway ARP. It sends at most five nonce-bearing gateway ICMP
probes per bound lease; a capture/real reply is separate stage-B evidence,
not Internet proof. All replies are counted once by sequence. Router/WAN images
never select IO_VERIFIED. No DNS resolver is advertised or implemented.

Not implemented: native PCIe/RTL8125, a LAN service adapter, local ICMP-error
emission, bounded IPv4 fragment reassembly/refragmentation, full PMTU path,
complete TCP/NAT standards behavior, routed client integration or Internet tests.
Fragments/IP options are deliberately rejected by the laboratory core. Therefore
no single/dual-LAN router image is supplied and no first-release claim is made.

The DPDK r8169 candidate reference is pinned separately at v25.11.4 commit
6953b6da139a83b62c92093aab8e88a5c1e0c042, BSD-3-Clause, with relevant source/license
copies. Its RTL8125 method-specific PHY/MCU/OCP setup and DPDK PCI/I/O/memory APIs
need a native adaptation; revision05 alone is insufficient to choose all paths.
No Linux module or DPDK runtime is linked. `pcie-evidence.json` resolves full-DT
providers and preserves 64-bit DBI windows (LAN1 0x3c0000000, LAN2 0x3c0400000),
APBs fe260000/fe270000, ranges, reset GPIOs, clocks, PHY and supplies. Native
PHY/controller power/reset ordering, config/iATU setup, safe BAR allocation,
endpoint method discovery and independently qualified RTL8125 DMA remain uncoded.
Existing BDF/BAR observations are not assigned as constants. No PCIe bus master
is enabled by any shipped image. No new broad inventory request is needed.

Protocol references: [DHCP](https://www.rfc-editor.org/rfc/rfc2131.html),
[ACD](https://www.rfc-editor.org/rfc/rfc5227.html),
[related ICMP](https://www.rfc-editor.org/rfc/rfc5508.html),
[DPDK r8169](https://doc.dpdk.org/guides/nics/r8169.html).
