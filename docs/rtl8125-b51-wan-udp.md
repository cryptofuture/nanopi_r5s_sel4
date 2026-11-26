# Read the LAN1 B51 diagnostic over WAN

The paths below identify the preserved original W build. For new builds, use the repaired iATU mapper and schema-2 WAN checkpoints. [Early initialization repair and deployment](../out/b51-wan-early-init-repair-report.md)

Variant **W** runs the one-frame variant B LAN1 experiment. A diagnostic-only WAN application answers IPv4 UDP port **48125** after it binds its WAN DHCP lease. Run the client on the same upstream IPv4 subnet. Any host on that subnet can read the report. The Microkit mapping gives the WAN application read-only access to the LAN1 status page. The responder selects the first 248 words at a fixed address.

## Retrieve the report

1. Leave WAN connected to its normal upstream network. Connect LAN1 to the capture peer on enp4s0 and leave LAN2 disconnected. Before boot, start capture on the LAN1 peer.

~~~bash
sudo ip link set enp4s0 up
sudo tcpdump -U -n -s 0 -i enp4s0 -w /tmp/b51-wan-status-lan1.pcap
~~~

2. Boot the verified W image. Look up MAC **02:52:35:53:00:01** in the upstream DHCP lease table. Use the address leased to the running diagnostic. The recovery Ubuntu address 10.0.1.81 belongs to a different boot. From the project root or extracted bundle, on a host connected to the WAN subnet, run:

~~~bash
ip route get "${WAN_IP:?set WAN_IP to the diagnostic WAN lease}"
python3 scripts/read-b51-udp "$WAN_IP" > b51-terminal.json
~~~

   The default deadline is 150 seconds. The client reports phase changes on stderr and saves the terminal snapshot on stdout. To retrieve the current sample while LAN1 is waiting, initializing or stalled:

~~~bash
python3 scripts/read-b51-udp "$WAN_IP" --live --timeout 5 > b51-live.json
~~~

   Check the exit code. A timeout can leave an empty redirected file. A successful reply contains **248 hexadecimal dwords** in index order plus the state. Once schema 1 is initialized, it also contains decoded milestones, tail/close pointers, both tally stages and guard results. A live sample can span a LAN1 phase change. A terminal sample follows the producer's release-store of phase 0xff after shutdown handling. A waiting reply confirms WAN reachability before the LAN1 report is ready. If WAN loses its lease or the board stops servicing the GMAC, a timeout gives no LAN1 result. UART and SYS remain available.

3. Stop the LAN1 capture with Ctrl-C, then inspect it:

~~~bash
sudo tcpdump -ennvv -r /tmp/b51-wan-status-lan1.pcap \
  'ether src 02:52:35:53:00:02 and ether dst ff:ff:ff:ff:ff:ff and arp and arp[6:2]=1 and arp[14:4]=0x0a2c0101 and arp[24:4]=0x0a2c0101'
~~~

   Save the pcap and JSON from the same boot. Milestones 21, 22 and 23 mean LAN1 submission, completion and timeout. Tally 28 means changed DMA memory with completion and intact guards. A UDP reply travels through GMAC0. Only the matching peer-captured ARP establishes LAN1 wire TX. The full word table is in **b51-status-reference.md** in the bundle, or **out/b51-dma-debug-report.md** in the workspace.

## Image and guarded SD handoff

Build output is **out/b51-wan-status/first/r5s-rtl8125-lan-repair.img**. The bundle is **out/b51-wan-status/r5s-b51-wan-udp.tar.gz** with a sibling archive checksum. Stage both on verified tmpfs or external storage on recovery Ubuntu, separate from the test SD and recovery eMMC. Allow at least 300 MiB of free space and use Python 3.11+ with mtools and dosfstools available. From the staging directory:

~~~bash
export TMPDIR="$PWD"
sha256sum -c r5s-b51-wan-udp.tar.gz.sha256 &&
tar -xzf r5s-b51-wan-udp.tar.gz &&
cd b51-wan-udp &&
sha256sum -c SHA256SUMS &&
python3 scripts/verify-image W/r5s-rtl8125-lan-repair.img
~~~

Identify the removable SD independently with lsblk, findmnt, its sysfs device/type, name, full CID and blockdev --getsize64. Back it up and unmount its partitions. Recovery eMMC previously appeared as /dev/mmcblk2. Use the existing guarded dry run from the extracted directory with the verified card identity:

~~~bash
python3 scripts/flash-sd --profile r5s --device "${SD_DEVICE:?}" \
  --image W/r5s-rtl8125-lan-repair.img --expect-model "${SD_MODEL:?}" \
  --expect-serial "${SD_CID:?}" --expect-bytes "${SD_BYTES:?}"
~~~

Only after a successful dry run and target review, an operator may repeat the same command with **--execute** and complete its confirmation. Wait for readback verification. Use the existing SD boot selection. To recover, power off, remove the test SD and boot the existing eMMC.

## Protocol and reproduction

Each request and reply has a 1008-byte UDP payload. All integers use network byte order. Bytes 0-3 are B51Q (request) or B51R (reply). Bytes 4-5 are version 1. Bytes 6-7 are request zero or reply state (0 waiting, 1 live, 2 terminal). Bytes 8-15 echo the client's 8-byte nonce. Bytes 16-1007 are 992 zero bytes in the request or the 248 status words in the reply. The nonce correlates replies. Requests must be unfragmented unicast to the bound WAN address. Replies have TTL 1, the same payload size as requests, and a global limit of four per second. The responder accepts no commands or caller-selected memory offsets.

From the project checkout:

~~~bash
. scripts/env
stage=$(mktemp -d out/b51-wan-rebuild.XXXXXXXX)
python3 scripts/build-b51-matrix W "$stage/first" &&
python3 scripts/build-b51-matrix W "$stage/repeat" &&
python3 scripts/package-b51-matrix --wan-udp --early-init-repair "$stage/first" "$stage/repeat" out/b51-wan-early-init-repair-report.md
~~~

Packaging checks image equality, the baseline LAN1 provider, GMAC driver, health and supervisor binaries, the read-only WAN status mapping, the absent LAN2 domain, and the extracted tools and image. The LAN1 experiment, serial report and SYS codes are identical to B. Physical WAN report retrieval and LAN1 wire TX require a board trial.
