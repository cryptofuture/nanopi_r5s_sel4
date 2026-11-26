# RTL8125 B51 physical validation

For the current DMA investigation, use variant B and collect UART and peer capture from the same boot. The earlier trials below retain their original image paths. [Current build, decoder and guarded deployment steps](../out/b51-dma-debug-report.md)

The diagnostic image runs WAN DHCP and brings up LAN1 with the pinned B51 firmware. It enables only LAN1 TX, uses one descriptor and one packet buffer from the existing checked DMA ring allocation, sends one padded broadcast ARP announcement, and waits up to one second for TX completion. LAN1 DHCP and RX are disabled in this image. WAN retains its normal path. LAN2 stays disabled.

## Isolating the failed service trial

Both TX descriptor layouts have now been reported to show the same failure. For the next board trial use the DMA tally diagnostic at `out/b51-dma-tally/r5s-rtl8125-lan-repair.img`. Verify it with `scripts/verify-image` and the adjacent SHA-256 file, then use the guarded SD workflow below with that image path. Keep the broad peer capture and record the SYS preamble and all codes from the same boot.

For No Close v3, a terminal tail/close of `1/0` means the tail register accepted the doorbell and the close pointer did not advance. A terminal `0/0` after reported submission calls for checking the doorbell write and device state first. A terminal `1/1` establishes queue completion. SYS 28 means the separate tally dump modified RAM, 29 means the command cleared with unchanged counter words, and 30 means the command timed out or a register access failed. For the exact limits of these observations, see [the DMA audit](rtl8125-b51-dma-audit.md).

### Earlier descriptor-format trial

Only the service image was physically tried. First run the existing one-frame v3 control from `out/b51-no-close.SVPTlhXR/b51-no-close/diagnostic.img`. The 32-byte v3 candidate is `out/b51-tx32.NgxFaQ8S/a/r5s-rtl8125-lan-repair.img`. The candidate manifest identifies `lan1_tx_descriptor_bytes=32`. Both images send the same ARP frame. Use identical cables, peer, capture and WAN hookup on each trial. Record the image hash with each capture.

| Packaged image | SHA-256 |
|---|---|
| `control-16.img` | `28ea5f1652a0504a41c6509f13ea0919b52bf4f3b0c5b3c96dad506b76634aaf` |
| `candidate-32.img` | `dc63593df36852d50157c84036fb120f8f8c4747f341fd239b1914d343f4e863` |

From the repository root set `image` to the selected path above, run `python3 scripts/verify-image "$image"`, and use the guarded SD procedure below with that path. Start a broad capture before boot so a damaged address or unexpected packet type is visible too:

```bash
sudo ip link set enp4s0 up
sudo tcpdump -U -n -s 0 -i enp4s0 -w /tmp/r5s-lan1-control.pcap
```

Stop the capture after the diagnostic finishes. Use a different filename for the candidate. Inspect with `tcpdump -ennvv -r /tmp/r5s-lan1-control.pcap`. A matching ARP on the control establishes physical TX on that run and directs investigation toward RX or service processing. If the control reaches submission and fails, compare with the 32-byte candidate. A capture only on the candidate warrants a repeat of both trials. A close timeout in both leaves engine activation and descriptor DMA reads open. A close advance in both without the frame warrants verifying the same cable and capture with FriendlyARM Linux TX.

A supervisor fault before the two-second startup notification can prevent LAN1 from running. A later supervisor fault takes over SYS and can hide LAN progress. Record the preamble and the full code. With a raw dump, LAN1 status word 7 gives the provider phase and word 2 bit 20 records submission. A five-second steady preamble identifies a LAN record. A flutter preamble alone does not establish a TX timeout.

## TX No Close v3 trial

Set `R5S_LAN1_TX_NO_CLOSE=1` when building `--lan-repair=lan1` to select the v3 protocol. Add `R5S_LAN1_TX_DIAGNOSTIC=1` for the single-frame diagnostic. Successful manifests identify `lan1_tx_mode: no-close-v3`. The build with the TX mode variable unset uses classic OWN/TxPoll.

FriendlyARM CFG_METHOD_5 enables TxConfig bit 6 and uses queue-0 software tail `0x2800` and hardware close `0x2802`, both 16-bit. Its ring initialization zeros the software producer, consumer and close accounting. The native driver checks both hardware pointers are zero after reset before transmitting. Pointers count descriptors modulo 65536. The descriptor index separately wraps at the ring size. FriendlyARM publishes address and options, executes a write barrier, and writes opts1 with OWN, first/last fragment, length and EOR at the end of the ring before advancing the tail. The native driver uses the same descriptor data and existing ARM64 barriers. With one outstanding TX, completion means the close pointer equals the submitted tail, including across wrap. Timeout leaves the descriptor and payload reserved while the existing shutdown path stops DMA. FriendlyARM schedules a reset and quiesces the device before clearing its rings. [Pinned FriendlyARM source](https://github.com/friendlyarm/r8125/blob/3fda70ebcba22f281ec4e3a69c355992c8823fc0/r8125_n.c)

In v3, SYS code 22 means the close pointer reached the submitted tail. Code 23 means it remained outstanding at the one-second deadline. Code 38 includes nonzero startup pointers or a TX mode readback mismatch. Codes 24 through 27 retain the MAC and PCI status decoding below. Raw-only LAN1 status words 13 and 14 hold the initial tail and close. Words 15 and 16 hold the terminal pair. All four are 32-bit slots containing 16-bit MMIO samples, with `0xffffffff` recording a failed read. A one-frame completion should report 0/0 then 1/1. The peer capture establishes delivery.

## Host reproduction

```bash
. scripts/env
bash scripts/test-lan1-bringup
bash scripts/test-router-integrated
python3 -m unittest discover -s tests
trials=$(mktemp -d out/b51-tx32.XXXXXXXX)
for trial in a b; do
  R5S_LAN1_TX_NO_CLOSE=1 R5S_LAN1_TX_DIAGNOSTIC=1 \
    python3 scripts/build-lan1-bringup "$trials/$trial" --lan-repair=lan1
done
cmp "$trials/a/r5s-rtl8125-lan-repair.img" "$trials/b/r5s-rtl8125-lan-repair.img"
```

Run the builds in fresh output directories. The image verifier requires the distinct `lan1_tx_diagnostic` manifest marker, the WAN + LAN1 diagnostic service list, and LAN2 disabled when that marker is present.

## Guarded SD handoff

From the directory containing the archive and checksum:

```bash
sha256sum -c r5s-b51-tx-format-test.tar.gz.sha256
tar -xzf r5s-b51-tx-format-test.tar.gz
cd r5s-b51-tx-format-test
sha256sum -c SHA256SUMS
image=control-16.img
python3 scripts/verify-image "$image"
lsblk -b -o NAME,TYPE,SIZE,MODEL,MOUNTPOINTS
```

The second trial uses `image=candidate-32.img` and the same verification steps. Use verified external storage or tmpfs to stage the bundle. Set `SD_DEVICE`, `SD_MODEL`, `SD_CID` and `SD_BYTES` from the independently verified test SD identity. On the R5S recovery system the SD has historically been `/dev/mmcblk0` and recovery eMMC `/dev/mmcblk2`. Verify the current enumeration and full 32-hex SD CID before proceeding. Leave eMMC untouched. Run this dry-run from recovery Linux:

```bash
python3 scripts/flash-sd --profile r5s --device "${SD_DEVICE:?}" \
  --image "$image" --expect-model "${SD_MODEL:?}" \
  --expect-serial "${SD_CID:?}" --expect-bytes "${SD_BYTES:?}"
```

After the operator confirms the test SD is backed up, unmounted and separate from eMMC, repeat with `--execute` and complete the writer’s typed confirmation. Wait for verified readback before booting the test SD. No target was flashed or rebooted during this work.

## Peer capture

Connect the Linux peer directly to the R5S LAN1 jack. Leave LAN2 disconnected and connect WAN to the normal upstream network. Start the capture before the test boot and keep it running through negotiation.

```bash
sudo ip link set enp4s0 up
sudo tcpdump -U -eni enp4s0 -w /tmp/r5s-b51-tx.pcap \
  'ether src 02:52:35:53:00:02 and ether broadcast and arp'
```

Inspect the saved file with `tcpdump -ennvv -r /tmp/r5s-b51-tx.pcap`. Accept wire TX only if the captured frame has source `02:52:35:53:00:02`, broadcast destination, ARP request opcode, and both sender and target IPv4 `10.44.1.1`. The peer does not have to transmit a request. Check that WAN obtains its lease and completes the gateway probes.

## SYS LED decoder

LAN1 records have a steady five-second preamble. Read two groups of four pulses. Short is 0 and long is 1. The last bit gives even parity. The leading seven bits give the decimal code. Keep the complete video because WAN and LAN1 records alternate and shutdown can report both the terminal and prior fault.

| Code | Meaning |
|---:|---|
| 65-75 | PCIe clock, reset, training, config, identity or BAR stage failure. See bundled fault codes for individual values. |
| 32-34 | DMA pool or inbound iATU setup failure. |
| 35-37 | B51 MAC/PHY initialization, firmware or carrier failure. |
| 38 | Ring or queue readback failure. |
| 45 | PCI mastering did not enable with the required readback. |
| 21 | Fixed frame submitted and queue-0 doorbell written. |
| 22 | TX completed. Check the peer capture for physical TX. |
| 23 / 41 | TX completion timed out. |
| 24 | MAC TX-error or system-error bit was set in the terminal snapshot. |
| 25 | Root PCI Status showed a conventional PCI error. |
| 26 | Endpoint PCI Status showed a conventional PCI error. |
| 27 | At least one terminal status read failed. |
| 42 | Shutdown/quiescence failed. Port reset fallback was attempted. |
| 43 | TX setup, submission or status failure. Check concurrent codes 24-27. |

Raw 32-bit words in the LAN1 status page at `0x41000000` are indexed 9 (opts1 with OWN synthesized from close-pointer progress in v3), 10 (MAC InterruptStatus), 11 (root PCI Command/Status), and 12 (endpoint PCI Command/Status). A successful diagnostic may also display the existing generic completion code 13. On timeout, the LED preserves code 23 even if shutdown later fails.

If 22 appears with a matching capture, repeat from a cold start. To qualify normal LAN service, use a separate WAN + LAN1 service build with the diagnostic feature off, capture directed ARP and the DHCP Discover/Offer/Request/ACK exchange, and check continued WAN operation. If 22 appears without a matching packet, verify the peer capture and physical jack before investigating MAC/PHY wire TX. If 23 appears, retain the PCI status codes and investigate descriptor fetch/writeback and TX activation before changing more registers.

## Service trial

Boot the separate service image with the same WAN and direct LAN1 wiring. Start the peer capture before boot.

```bash
sudo tcpdump -U -eni enp4s0 -w /tmp/r5s-b51-lan1.pcap 'arp or (udp port 67 or udp port 68)'
```

Request a fresh lease using one DHCP client on the peer. Verify Discover, Offer, Request and ACK in the capture and a lease in 10.44.1.100 through 10.44.1.199. Run `sudo arping -I enp4s0 -c 5 10.44.1.1` and check for replies from `02:52:35:53:00:02`. Verify WAN still obtains its upstream lease.
