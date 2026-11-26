# NanoPi R5S shared LAN IPv4 router and front LEDs

The dual-lan-router image adds WAN IPv4 NAT to the dual-LAN DHCP profile. LAN1 and LAN2 share 10.44.1.1/24 and DHCP addresses 10.44.1.100 through 10.44.1.199. DHCP advertises 10.44.1.1 as the default gateway and the DNS servers from the active WAN DHCP lease. R5S_LAN_DNS optionally supplies a resolver override at build time. The router forwards IPv4 DNS traffic using NAT. There is no DNS server listening on 10.44.1.1 and IPv6 routing is outside this checkpoint.

The bridge remains separate from WAN. Its local stack owns NAT state for both LAN ports. Separate bounded single-producer queues relay eligible WAN traffic and outbound frames through the existing WAN process. Gateway ARP replies resolve upstream neighbors, and unsolicited inbound IPv4 is rejected by the existing stateful policy. The relay uses a fresh, matching WAN lease and GMAC generation. WAN UDP status stays on port 48125. Schema 3 appends counters for frames queued toward WAN, forwarded back toward LAN and dropped at the WAN relay.

The supervisor drives the active-high LAN1 and LAN2 front LEDs on GPIO3_D6/D7. Each stays lit with carrier and blinks with RX or completed TX. Link down turns it off. GPIO3 clock, direction and pinmux are checked before use. SYS and the WAN LED keep their existing behavior. [NanoPi R5S LED wiring](https://github.com/torvalds/linux/blob/v6.12/arch/arm64/boot/dts/rockchip/rk3568-nanopi-r5s.dts#L23)

## Deploy

The build is under out/r5s-dual-lan-router/final/first/. Copy the archive r5s-dual-lan-router.tar.gz and its .sha256 file to recovery Ubuntu or a Linux staging host, outside the SD card. Verify and extract from that directory.

~~~bash
sha256sum -c r5s-dual-lan-router.tar.gz.sha256 &&
tar -xzf r5s-dual-lan-router.tar.gz &&
cd r5s-dual-lan-bringup &&
sha256sum -c SHA256SUMS &&
python3 scripts/verify-image dual-lan-router/r5s-rtl8125-lan-repair.img
~~~

Identify the removable test SD using the live device path, model, full CID and byte count. Back it up and unmount its partitions. Leave the recovery eMMC outside the operation. Apply the guarded dry run using the verified card values.

~~~bash
python3 scripts/flash-sd --profile r5s   --device "${SD_DEVICE:?}"   --image dual-lan-router/r5s-rtl8125-lan-repair.img   --expect-model "${SD_MODEL:?}"   --expect-serial "${SD_CID:?}" --expect-bytes "${SD_BYTES:?}"
~~~

If the dry run succeeds and shows the right SD, repeat with --execute and wait for readback verification. Boot the R5S from the SD. For recovery, power off, remove the SD and boot from the existing eMMC.

## Test

1. Connect WAN to Keenetic. Confirm the R5S WAN DHCP lease for 02:52:35:53:00:01. Connect a client to each LAN. Renew the clients after WAN binds, so they replace the previous DHCP-only leases with leases carrying a default route and DNS.
2. On each client, check its assigned address, default route via 10.44.1.1 and received DNS servers. Run the following. A public host may filter ping, so also use the DNS result and HTTPS as independent checks.

~~~bash
ip -4 addr
ip -4 route
ping -c 3 10.44.1.1
ping -c 3 8.8.8.8
getent ahostsv4 example.com
curl -4I https://example.com
~~~

3. Check each front LAN LED with the other port idle. With its cable connected it should be lit and pulse during a sustained ping. Unplugging its cable should turn that LED off.
4. If IPv4 or DNS fails, save an on-link WAN report. Check the WAN lease, WAN queue counters and both LAN TX/RX counters. Capture WAN DHCP or translated traffic via a mirror/TAP if needed.

~~~bash
python3 scripts/read-b51-udp "${WAN_IP:?set the R5S WAN lease address}" --timeout 5 > r5s-router-status.json
~~~

## Verification

Two clean builds matched byte-for-byte. Image SHA-256 is `181c833c0c20534956bd363a55c8bd7d84d8961fd1f3f0e45085effbcdf5e483`. Integrated router and sanitized GMAC tests, the WAN relay regression, and 105 Python tests passed. Physical WAN forwarding, client DNS and the front LED behavior await a board trial.

## Rebuild

~~~bash
. scripts/env
stage=$(mktemp -d out/r5s-dual-lan-router-rebuild.XXXXXXXX)
for run in first repeat; do
  R5S_DUAL_LAN_STAGE=dual-lan-router     python3 scripts/build-lan1-bringup "$stage/$run" --lan-repair=both || exit 1
done
python3 scripts/package-b51-matrix --dual-lan --checkpoint dual-lan-router   "$stage/first" "$stage/repeat" docs/r5s-dual-lan-router.md
~~~
