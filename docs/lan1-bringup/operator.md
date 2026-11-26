# LAN1 hardware acceptance

This candidate answers one broadcast ARP request for 10.44.1.1/24, waits for TX completion, stops the controller and retains the SYS milestones until power-off. One captured reply is sufficient. Connect a dedicated Linux Ethernet interface directly to the jack marked LAN1. Power cycling starts a new trial.

## Prepare the peer

Record the board's LAN1 factory MAC from the corresponding RTL8125 interface on the working Linux installation before the trial, using `ethtool -P <board-lan1-interface>`. Use the interface associated with the PCIe root at fe260000. On the connected peer, substitute its actual interface name below. Start the capture before SD boot so the first reply is retained.

```bash
iface=YOUR_INTERFACE
sudo ip addr add 10.44.1.2/24 dev "$iface"
sudo ip link set "$iface" up
sudo tcpdump -eni "$iface" arp
```

## Write the experimental SD

Stage the archive and checksum together on a sufficiently large tmpfs or separate USB device. On the R5S, run the existing guarded writer from the working Ubuntu system. Keep the source files, extraction directory and logs off the experimental SD and eMMC. Leave all eMMC partitions alone. Check that no SD partitions are mounted.

```bash
(
set -eu
sha256sum -c r5s-lan1-bringup-handoff.tar.gz.sha256
test ! -e router-lan1-bringup
tar -xzf r5s-lan1-bringup-handoff.tar.gz
cd router-lan1-bringup
sha256sum -c SHA256SUMS
python3 scripts/check-deps
python3 scripts/flash-sd --profile r5s --device /dev/mmcblk0 --image r5s-lan1-bringup.img --expect-model NCard --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488
)
```

After a successful dry run, run this separately from the extracted directory. Verify the displayed model, full CID and size before typing the writer's confirmation. Allow the writer to finish its readback check.

```bash
python3 scripts/flash-sd --profile r5s --device /dev/mmcblk0 --image r5s-lan1-bringup.img --expect-model NCard --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488 --execute
```

## Observe the trial

1. Boot from the experimental SD with the peer connected. Leave LAN2 disconnected. Check peer carrier with `cat /sys/class/net/"$iface"/carrier`.
2. Once carrier is up, run `sudo arping -b -I "$iface" -c 5 10.44.1.1` in another terminal. Repeat during the five-minute receive window if carrier appeared before DMA started. The Linux command without `-b` also begins with broadcast. The firmware waits up to 90 seconds for negotiation.
3. Record the SYS preamble and both symbol groups. The long preamble identifies LAN1 reports. Consult fault-codes.md. Optical frames take 42.5 seconds each. Send requests while watching, since the complete milestone history takes longer than the receive window.
4. Accept the trial only if the peer capture shows a unicast Ethernet reply to its MAC, ARP opcode 2, sender IP 10.44.1.1, sender MAC equal to the recorded LAN1 address, and target IP 10.44.1.2. Confirm link, RX, request and TX milestones and absence of a fatal or shutdown fault. Save the capture and decoded SYS words.
5. After the trial, power off, remove the experimental card and recover using the established boot selection. A shutdown fault requires a power cycle before another trial.

WAN continues its existing DHCP client behavior. LAN1 offers ARP only. ICMP echo and DHCP on LAN1 are outside this acceptance test.
