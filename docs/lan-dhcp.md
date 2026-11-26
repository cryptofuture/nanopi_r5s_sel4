# Dual LAN DHCP candidate

| Interface | Server IPv4 address | Dynamic pool | Server MAC |
|---|---|---|---|
| LAN1 | 10.44.1.1/24 | 10.44.1.100–199 | 02:52:35:53:00:02 |
| LAN2 | 10.44.2.1/24 | 10.44.2.100–199 | 02:52:35:53:00:03 |

Each LAN runs a separate instance of the DHCP server. It probes addresses with ARP before offering them, responds to requests and renewals, and expires leases. The WAN continues to run its DHCP client and LED driver. This profile provides local LAN DHCP and ARP. Internet forwarding is outside this profile.

LAN1 and LAN2 use separate 1 MiB Normal-NC pools at physical addresses 0x24200000 and 0x24300000. Their inbound bus bases are 0x80000000 and 0x81000000. Each port verifies its own identity, B51 signature, firmware and inbound translation registers before enabling mastering. LAN2 remains unqualified on the board. A non-B51 result causes that port to stop at discovery, so its DHCP server remains unavailable.

## Check on the board

1. Extract the handoff to tmpfs or external USB storage, verify the archive and image checksums, and run the included image verifier. Use the existing guarded SD workflow for the experimental card. Keep files and logs off eMMC.
2. Connect one DHCP-enabled client to each LAN before powering on. Each port has a 90-second Ethernet link negotiation window.
3. Capture UDP ports 67 and 68 on each client. Confirm Discover, Offer, Request and ACK and confirm the assigned IPv4 address and server identifier match the jack in the table. Renew each lease and confirm it stays on the same port. Check the WAN LEDs separately.
4. SYS alternates the WAN and LAN records. A steady five-second ON preamble identifies a LAN record. The LAN2 label is the corresponding LAN1 code plus 50. LAN1 code 74 and LAN2 code 124 indicate an unsupported hardware method. LAN1 code 13 and LAN2 code 63 indicate TX descriptor completion. A client capture proves that an ACK reached the wire.

The guarded dry run uses the expected SD card model, CID and size.

```bash
python3 scripts/flash-sd --profile r5s --device /dev/mmcblk0 --image r5s-lan-dhcp.img --expect-model NCard --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488
```

After a successful dry run, run the same command with `--execute` and confirm the exact target in the writer prompt. The writer verifies the SD readback.
