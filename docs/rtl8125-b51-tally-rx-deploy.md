# LAN1 tally RX comparison

Build from the project root in a fresh directory.

```bash
. scripts/env
build=$(mktemp -d out/b51-tally-rx.XXXXXXXX)
R5S_LAN1_TX_NO_CLOSE=1 R5S_LAN1_TX_DIAGNOSTIC=1 \
  python3 scripts/build-lan1-bringup "$build" --lan-repair=lan1
python3 scripts/verify-image "$build/r5s-rtl8125-lan-repair.img"
(cd "$build" && sha256sum -c r5s-rtl8125-lan-repair.sha256)
```

For the packaged build, extract the archive and run `sha256sum -c r5s-rtl8125-lan-repair.sha256` and `python3 scripts/verify-image r5s-rtl8125-lan-repair.img` in the extracted directory. Use recovery Linux and independently identify the removable test SD with lsblk and its full CID. Set SD_DEVICE, SD_MODEL, SD_CID and SD_BYTES from that card. Run the guard in dry-run mode first.

```bash
python3 scripts/flash-sd --profile r5s --device "${SD_DEVICE:?}" \
  --image r5s-rtl8125-lan-repair.img --expect-model "${SD_MODEL:?}" \
  --expect-serial "${SD_CID:?}" --expect-bytes "${SD_BYTES:?}"
```

After verifying the target, backing it up and unmounting it, repeat with `--execute` and complete the typed confirmation. Wait for verified readback. Leave recovery eMMC untouched. To return to recovery after the test, power down and remove the test SD, then boot the existing eMMC installation.

Connect the peer directly to LAN1 before boot. Keep the normal WAN connection and leave LAN2 disconnected.

```bash
sudo ip link set enp4s0 up
sudo tcpdump -U -n -s 0 -i enp4s0 -w /tmp/r5s-tally-rx.pcap
```

The expected frame is a broadcast ARP request from 02:52:35:53:00:02 announcing 10.44.1.1. Capture from before boot and record SYS. A five-second steady preamble identifies LAN status. The next eight pulses encode seven data bits and even parity. Long means 1. Short means 0.

| SYS | Meaning |
|---|---|
| 21 | Frame submitted |
| 22 / 23 | TX completion / timeout |
| 24–27 | Existing MAC and PCI error signals |
| 28 / 29 / 30 | Tally write observed / command cleared with unchanged counters / timeout or access failure |
| 31 | Tally guard modified |
| 32–45, 65–75 | Existing terminal startup and shutdown results. See the physical validation guide. |

SYS reports the final tally phase. Raw LAN status at 0x41000000 includes words 9–12 for completion and MAC/PCI status, 13–16 for initial and terminal v3 tail/close, 17–18 for counter words, 19–20 for ChipCmd before each tally phase and 21–22 for the phase outcomes. The native status page requires platform debug access. When phase A timed out, phase B observes the original request after RX enable. The capture and SYS outcome are still useful without access to the raw page.
