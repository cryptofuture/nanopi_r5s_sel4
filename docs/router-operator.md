# Operator handoff: hardware promotion remains conditional

No commands here have been run against the R5S. New images are hardware-untested.
The unchanged baseline and link result are already user-observed; do not repeat
them just because their report predates this task. No native packet exchange,
WAN DHCP or router result has been reported. Ubuntu10.0.1.81 is the recovery OS,
not a native SSH endpoint. The experimental runtime has no SSH or storage writer.

The product goal is DHCP/IPoE WAN and two separately routed DHCP LANs sharing
Internet through NAT. This bundle implements only stageA and the stageB client.
It does not yet provide a working LAN/router. StageC/D/E images are absent.

## Cabling and acceptance order

| Profile | Integrated 1GbE GMAC socket | Other two sockets | Acceptance |
|---|---|---|---|
| DMA preflight | Dedicated known link partner | Disconnected | READY after real cache calls and PHY link; no DMA |
| rawio | Dedicated Linux bench peer | Disconnected | Fresh externally checked response/ACK, expiry, wraps, recovery |
| wan-dhcp | Isolated controlled DHCPv4 gateway, after stageA | Disconnected | Captured native lease, ARP and fresh IPv4 reply, renewal/expiry, recovery |
| Future router | GMAC upstream, verified RTL8125 roles to clients | Not available yet | No image supplied |

Use the integrated1GbE socket established by the prior GMAC tests. Chassis
LAN/WAN lettering is not assumed. LAN1's eventual controller is APBfe260000,
LAN2's fe270000; observed BDFs are not native assignments. Do not connect either
future DHCP-serving port to a production LAN. Confirm cabling and peer isolation
before each test. No alternative Wi-Fi/VPN/default route may stand in for a
claimed routed test. No host interface is reconfigured by these tools.

## Stage and extract once

Boot Ubuntu with the experimental SD removed and establish SSH access. Preserve
existing evidence and genuine pre-test boot-region hashes. A newly collected
hash is a baseline only for subsequent tests. Reinsert the unmounted SD. Keep
all source files, CWD and logs on eMMC, never the SD. On R5S Ubuntu, check that
these new paths do not already exist before transfer:

```sh
(
  set -eu
  test ! -e /root/r5s-flash/router
  test ! -e /root/r5s-flash/r5s-router-handoff.tar.gz
  test ! -e /root/r5s-flash/r5s-router-handoff.tar.gz.sha256
)
```

On the development computer containing this checkout, transfer regular files:

```sh
scp /workspaces/mounted/projects/r5s_custom/out/router/r5s-router-handoff.tar.gz /workspaces/mounted/projects/r5s_custom/out/router/r5s-router-handoff.tar.gz.sha256 root@10.0.1.81:/root/r5s-flash/
```

On R5S Ubuntu, verify, list and safely extract into the parent directory exactly
once. Any refusal stops the procedure. The checksum file is generated alongside
the archive; it does not authenticate an untrusted transfer source.

```sh
(
  set -eu
  cd /root/r5s-flash
  sha256sum -c r5s-router-handoff.tar.gz.sha256
  python3 - <<'PY'
import pathlib,tarfile
root=pathlib.Path('/root/r5s-flash')
target=root/'router'
if target.exists():raise SystemExit('Refusing existing router directory')
with tarfile.open(root/'r5s-router-handoff.tar.gz','r:gz') as archive:
    members=archive.getmembers()
    names=set();total=0
    for member in members:
        path=pathlib.PurePosixPath(member.name)
        if path.is_absolute() or '..' in path.parts or not path.parts or path.parts[0]!='router' or not (member.isfile() or member.isdir()) or member.name in names:
            raise SystemExit('Unsafe archive member')
        names.add(member.name);total+=member.size
        print(member.name)
    if total>1024*1024*1024:raise SystemExit('Archive exceeds handoff size limit')
    archive.extractall(root,members=members,filter='data')
PY
  cd /root/r5s-flash/router
  sha256sum -c SHA256SUMS
  python3 scripts/check-router-deps
  python3 scripts/verify-image r5s-gmac-dma-preflight.img
)
```

Required Ubuntu packages are python3, dosfstools (fsck.fat), mtools
(mcopy/mdir/mtype), util-linux (lsblk/findmnt/blkid/swapon/mountpoint/umount),
coreutils, tar and gzip. Python>=3.11 is required. If missing, the operator may
choose to install them with `apt-get install python3 dosfstools mtools util-linux
coreutils tar gzip`. Nothing installs automatically. The writer uses Python GPT
checks; sgdisk is a development assembler dependency, not a write prerequisite.

## First image: real cache preflight with DMA disabled

Check the target identity/usage and normally unmount the old read-only FAT mount:

```sh
(
  set -eu
  cd /root/r5s-flash/router
  lsblk --bytes --paths -o NAME,TYPE,SIZE,MODEL,SERIAL,MOUNTPOINTS /dev/mmcblk0 /dev/mmcblk2
  cat /sys/block/mmcblk0/device/type /sys/block/mmcblk0/device/name /sys/block/mmcblk0/device/cid
  swapon --show
  if mountpoint -q /mnt/r5s-boot; then umount /mnt/r5s-boot; fi
  python3 scripts/flash-sd --device /dev/mmcblk0 --image /root/r5s-flash/router/r5s-gmac-dma-preflight.img --profile r5s --expect-model NCard --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488
)
```

Dry-run is the default. The writer rejects other mounts, swap, hidden OverlayFS
root/data/source dependencies, unsupported topology, wrong identities/capacity,
emMC or any unproven condition. Do not force/lazy-unmount, change fstab, repair
GPT or substitute dd. The exact FriendlyElec model, both compatible strings and
hidden early-boot eMMC OverlayFS resolver are preserved. Full CID is required,
not short serial0x0d3aa313. All mmcblk2 partitions/boot/RPMB remain protected.

Only after reviewing the successful dry-run, explicitly choose this separate
command. It asks for typed confirmation and reinspects the target. Do not run
it as part of an automatic test loop:

```sh
(
  set -eu
  cd /root/r5s-flash/router
  python3 scripts/flash-sd --device /dev/mmcblk0 --image /root/r5s-flash/router/r5s-gmac-dma-preflight.img --profile r5s --expect-model NCard --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488 --execute
)
```

Type exactly when prompted:
`OVERWRITE /dev/mmcblk0 8903034e43617264100d3aa313016c07 62534975488`.
The writer holds O_RDWR|O_EXCL through writing, fsync, BLKFLSBUF and byte comparison
of the actual image length (100663296bytes), without an unprotected reopen. A
failed flush/readback is a failure. Its “Written, flushed and byte-compared”
message is the bounded readback result; no whole-card scan is needed.

After successful verification, the operator may cleanly shut down Ubuntu with
`shutdown -h now`. Use the same physical SD-selection procedure that produced
the known-good diagnostic and link patterns. No new MASK timing or eMMC change.
Record SYS for60s from power application, not a claimed boot deadline. With a
link, READY must be an isolated200ms tick each4s. Preflight never proves packets.
Then power off, remove experimental SD, normal power-on and verify Ubuntu SSH.
Stop on recovery failure; never automatically erase/reinstall eMMC.

## Promote to raw I/O only after preflight and recovery

Repeat the identity/unmount/checksum procedure. The exact new dry-run and
separate explicit-write commands are:

```sh
python3 /root/r5s-flash/router/scripts/flash-sd --device /dev/mmcblk0 --image /root/r5s-flash/router/r5s-gmac-rawio.img --profile r5s --expect-model NCard --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488
```

```sh
python3 /root/r5s-flash/router/scripts/flash-sd --device /dev/mmcblk0 --image /root/r5s-flash/router/r5s-gmac-rawio.img --profile r5s --expect-model NCard --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488 --execute
```

Use the same typed confirmation, verification, shutdown/selection and recovery.
On the isolated Linux peer, extract `router-test-tools.tar.gz` into a new working
directory. Raw socket privilege permits capture/transmit only; it does not
permit silently changing host networking. Select a dedicated interface by name:

```sh
read -r -p 'Dedicated isolated Ethernet interface: ' PEER_IFACE
sudo python3 tools/net-peer --interface "$PEER_IFACE" --target 02:52:35:53:00:01 --build-id 870b9c61dbf5bf2523b9bc0bc3b8263e --count 1000 --rate 1 --report rawio-1000.json
```

The build ID above is the literal identity of this raw-I/O payload.
The peer varies payload lengths including the maximum, uses fresh unpredictable
nonces and rejects outgoing copies/stale/mismatched frames. Require1000 validated
responses and ACK sends, no mismatches/loss, retained JSON, board IO_VERIFIED,
lease expiry within5s plus the10ms diagnostic poll bound after peer stops, cable
removal/reconnect and cold-start rerun. A partial frame is inconclusive. Host RTT
is end-to-end diagnostic RTT, not forwarding latency. TX completion, link or
READY alone is not native bidirectional proof. ACK-sent is not independently
proof of board ACK acceptance; the live board state supplies that observation.

## Promote WAN DHCP only after genuine raw exchange and recovery

Use an isolated lab DHCP gateway first. Start a bounded passive observation on
that gateway's dedicated peer interface before boot (it changes no addresses or
routes):

```sh
read -r -p 'Dedicated isolated gateway Ethernet interface: ' PEER_IFACE
sudo python3 tools/wan-observer --interface "$PEER_IFACE" --seconds 120 --report wan-lease-ipv4.json
```

If using a switch, observation requires a genuinely configured capture point;
a normal unrelated switch port will not see unicast traffic. Providing a DHCP
server/link partner is an operator-controlled lab setup, not a command this
bundle executes on a management interface. Then, following the same R5S checks:

```sh
python3 /root/r5s-flash/router/scripts/flash-sd --device /dev/mmcblk0 --image /root/r5s-flash/router/r5s-wan-dhcp.img --profile r5s --expect-model NCard --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488
```

```sh
python3 /root/r5s-flash/router/scripts/flash-sd --device /dev/mmcblk0 --image /root/r5s-flash/router/r5s-wan-dhcp.img --profile r5s --expect-model NCard --expect-serial 8903034e43617264100d3aa313016c07 --expect-bytes 62534975488 --execute
```

A current valid lease plus fresh gateway ARP gives READY; no cable/lease is WAIT.
No IO_VERIFIED is emitted. Retain the observed DHCP exchange, gateway ARP and
nonce-matching echo request/reply plus optional serial runtime count. The
observer reports wire evidence, not board receive completion or Internet access.
A gateway may prohibit ping, so absence alone does not identify a DMA fault.
Use a controlled gateway that permits these stage-B probes. Exercise short
lease renewal, NAK/expiry, upstream address/subnet/DNS changes, conflict, overlap
refusal, cable reconnect and cold start. LAN sockets stay disconnected. No DHCP
OFFER/ACK may originate from the board. Then test Ubuntu recovery again.

Future routed acceptance remains pending: each physical LAN's native raw frames,
DHCP gateway/DNS on separate subnets, concurrent same-source-port NAT flows,
controlled upstream translated-source capture, interface-bound IPv4 DNS/HTTPS
with valid TLS and known content hash, unsolicited/inter-LAN rejection,
PMTU/fragments/exhaustion and recovery. No commands or image here claim those
unimplemented results. Existing upstream10.0.1.0/24 is valid private WAN later;
never assign native WAN Ubuntu's10.0.1.81 by assumption.
