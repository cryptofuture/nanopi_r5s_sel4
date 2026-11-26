from pathlib import Path
import runpy
import socket
import struct
import sys
import threading
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
with patch.object(sys, "path", [str(SCRIPTS), *sys.path]):
    client = runpy.run_path(str(SCRIPTS / "read-b51-udp"))


class WanStatusClient(unittest.TestCase):
    def test_client_retries_live_report_and_decodes_terminal_snapshot(self):
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as peer:
            peer.bind(("127.0.0.1", 0))
            peer.settimeout(3)
            requests = []

            def serve():
                for state in (1, 2):
                    request, addr = peer.recvfrom(1500)
                    requests.append(request)
                    words = list(range(248))
                    words[7] = 12 if state == 1 else 255
                    words[25] = 1
                    header = struct.pack("!4sHH8s", b"B51R", 1, state, request[8:16])
                    peer.sendto(header + struct.pack("!248I", *words), addr)

            thread = threading.Thread(target=serve)
            thread.start()
            try:
                fetch = client["fetch"]
                with patch.dict(fetch.__globals__, PORT=peer.getsockname()[1]):
                    result = fetch("127.0.0.1", 3, False)
            finally:
                thread.join(4)
        self.assertEqual(result["state"], "terminal")
        self.assertEqual(len(result["words"]), 248)
        self.assertEqual(result["words"][247], "000000f7")
        self.assertEqual(len(requests), 2)
        for req in requests:
            self.assertEqual(req[:8], b"B51Q\0\1\0\0")
            # Zero request padding at either supported schema width.
            self.assertIn(len(req) - 16, (992, 1072, 1216))
            self.assertEqual(req[16:], bytes(len(req) - 16))
        self.assertNotEqual(requests[0][8:16], requests[1][8:16])

    def test_stale_nonce_truncation_and_preinit(self):
        parse = client["parse_reply"]
        nonce = b"snapshot"
        reply = struct.pack("!4sHH8s", b"B51R", 1, 0, nonce) + bytes(992)
        self.assertEqual(parse(reply, nonce)["state"], "waiting")
        self.assertIsNone(parse(reply[:-4], nonce))
        self.assertIsNone(parse(reply, b"oldnonce"))
        with self.assertRaises(ValueError):
            parse(reply[:7] + b"\x02" + reply[8:], nonce)

    def test_schema_three_reports_running_ports_and_wan_ack(self):
        words = [0] * 248
        words[7], words[25], words[26] = 6, 3, 3
        words[34], words[38], words[44], words[45] = 1 << 6, 0x13, 1, 7
        words[152], words[154], words[162], words[166] = 7, 0x107, 2, 1000
        nonce = b"snapshot"
        response = struct.pack("!4sHH8s", b"B51R", 1, 1, nonce) + struct.pack("!248I", *words)
        report = client["parse_reply"](response, nonce)
        self.assertEqual(report["state"], "live")
        self.assertEqual(report["lan1"]["rx_packets"], 7)
        self.assertEqual(report["lan1"]["speed_mbps"], 1000)
        self.assertIsNone(report["lan2"]["speed_mbps"])
        self.assertIn("matching ACK accepted", report["wan"]["dhcp_stages"])
        self.assertIsNone(report["wan"]["lease"])
        self.assertFalse(report["wan"]["app_snapshot_available"])
        self.assertEqual(report["wan"]["speed_mbps"], 1000)
        self.assertNotIn("internet_echo", report["wan"])
        self.assertNotIn("supervisor", report)

    def test_schema_four_preserves_port_fields_and_decodes_pipeline(self):
        words = [0] * 248
        words[7], words[25], words[26] = 6, 4, 3
        words[8:20] = [2, 63, 4, 0, 5, 16, 0, 9, 0, 1, 8, 3]
        words[98], words[102], words[50], words[51] = 10, 7, 402, 400
        nonce = b"snapshot"
        header = struct.pack("!4sHH8s", b"B51R", 1, 1, nonce)
        report = client["parse_reply"](header + struct.pack("!248I", *words), nonce)
        self.assertEqual(report["version"], 4)
        lan1, lan2 = report["lan1"], report["lan2"]
        self.assertEqual((lan1["tx_submitted"], lan1["tx_completed"], lan1["tx_ring_inflight"]), (402, 400, 2))
        self.assertEqual((lan1["tx_ring_peak_inflight"], lan1["tx_queue_drops"]), (63, 10))
        self.assertEqual((lan2["tx_errors"], lan2["tx_reclaim_max_batch"], lan2["tx_queue_drops"]), (1, 3, 7))

    def test_schema_five_exports_worker_counters(self):
        words = [0] * 248
        words[7], words[25], words[26] = 6, 5, 3
        words[20:24] = [100, 2, 8 << 14 | 3 << 10 | 12, 25]
        words[247], words[31] = 7, 301
        nonce = b"snapshot"
        header = struct.pack("!4sHH8s", b"B51R", 1, 1, nonce)
        report = client["parse_reply"](header + struct.pack("!248I", *words), nonce)
        self.assertEqual(report["version"], 5)
        self.assertEqual(report["multicore"]["worker_count"], 4)
        self.assertEqual(report["multicore"]["dispatch_failures"], 7)
        self.assertEqual(report["multicore"]["dispatch_total"], 301)
        self.assertEqual(report["multicore"]["workers"][0],
                         dict(cpu_id=0, packets_received=100, packets_dropped=2,
                              active_nat_flows=12, queue_depth=3, queue_peak=8, processing_batches=25))

    def test_optional_ipv4_header_offload_counters(self):
        words = [0] * 248
        words[7], words[25], words[26] = 6, 5, 3
        words[41:44] = [11, 1, 22]
        words[73:76] = [0, 0, 1]
        nonce = b"snapshot"
        header = struct.pack("!4sHH8s", b"B51R", 1, 1, nonce)
        report = client["parse_reply"](header + struct.pack("!248I", *words), nonce)
        self.assertEqual(report["tx_ipv4_checksum"], {
            "lan1": dict(requests=11, active=True),
            "lan2": dict(requests=0, active=False),
            "wan": dict(requests=22, active=True),
        })

    def test_schema_six_appends_zero_copy_pool_counters(self):
        words = [0] * 268
        words[7], words[25], words[26] = 6, 6, 3
        words[248:266] = range(18)
        words[264], words[266], words[267] = 12, 16, 496
        nonce = b"schema6!"
        header = struct.pack("!4sHH8s", b"B51R", 1, 1, nonce)
        report = client["parse_reply"](header + struct.pack("!268I", *words), nonce)
        self.assertEqual(report["zc"], dict(
            enabled=True, packets_received=0, packets_sent=1, buffers_completed=2,
            packets_forwarded=3, packets_dropped=4, copied_packets=5, copied_bytes=6,
            queue_drops=7, mapping_failures=8, ownership_errors=9, rx_recycled=10,
            allocation_failures=11, rx_dropped=17,
            fallback=dict(pool_exhausted=12, mapping=13, layout=14, queue_full=15),
            pool=dict(in_use=16, free=496, peak=12, buffers=512)))
        # A 248-word schema 5 report stays accepted and carries no zc block.
        old = [0] * 248
        old[7], old[25] = 6, 5
        self.assertNotIn("zc", client["parse_reply"](
            struct.pack("!4sHH8s", b"B51R", 1, 1, nonce) + struct.pack("!248I", *old), nonce))

    def test_worker_and_dma_capabilities_survive_zero_copy_schema(self):
        words = [0] * 268
        words[0:7] = [0x14000, 0x40000, (2 << 18) | (1 << 12), 0, 3, 16, 128 | (64 << 8)]
        words[7], words[25], words[26] = 6, 6, 3
        words[20] = 100
        words[27] = 1 << 31 | 2 << 16 | 1 << 19 | 3 << 22 | 0x13
        words[34] = words[66] = 1 << 7
        nonce = b"snapshot"
        header = struct.pack("!4sHH8s", b"B51R", 1, 1, nonce)
        report = client["parse_reply"](header + struct.pack("!268I", *words), nonce)
        self.assertEqual(report["multicore"]["workers"][0]["packets_received"], 100)
        self.assertEqual(report["lan1"]["ring_entries"], 128)
        self.assertEqual(report["lan2"]["ring_entries"], 64)
        self.assertEqual(report["lan1"]["pcie"]["endpoint"],
                         dict(generation=3, lanes=1, max_payload_supported_bytes=512,
                              max_payload_bytes=256, max_read_request_bytes=1024))
        self.assertNotIn("pcie", report["lan2"])
        caps = report["wan"]["dma_capabilities"]
        self.assertEqual((caps["tx_channels"], caps["rx_channels"], caps["pbl"]), (3, 2, 16))
        self.assertTrue(caps["tso"] and caps["rx_checksum"] and caps["tx_checksum"])
        words[5:7] = [256, 1 | (2 << 8)]
        report = client["parse_reply"](header + struct.pack("!268I", *words), nonce)
        self.assertEqual((report["lan1"]["ring_entries"], report["lan2"]["ring_entries"]), (256, 512))
        self.assertEqual(report["wan"]["dma_capabilities"]["pbl"], 256)
        self.assertNotIn("axi", report["wan"]["dma_capabilities"])
        words[6] |= (1 << 31) | (1 << 24) | (4 << 20) | (8 << 16)
        report = client["parse_reply"](header + struct.pack("!268I", *words), nonce)
        self.assertEqual(report["lan2"]["ring_entries"], 512)
        self.assertEqual(report["wan"]["dma_capabilities"]["axi"],
                         dict(read_outstanding_field=8, write_outstanding_field=4, mixed_burst=True))
        self.assertFalse(report["wan"]["dma_capabilities"]["eame"])
        words[5] |= 1 << 31
        report = client["parse_reply"](header + struct.pack("!268I", *words), nonce)
        self.assertEqual(report["wan"]["dma_capabilities"]["pbl"], 256)
        self.assertTrue(report["wan"]["dma_capabilities"]["eame"])
        words[5] &= ~(1 << 31)
        report = client["parse_reply"](header + struct.pack("!268I", *words), nonce)
        self.assertNotIn("rx_interrupt", report["wan"])
        words[6] |= (1 << 27) | (1 << 25) | (1 << 26)
        report = client["parse_reply"](header + struct.pack("!268I", *words), nonce)
        self.assertEqual(report["wan"]["rx_interrupt"], dict(active=True, observed=True))

    def test_rss_schema_reports_queue_activity_on_both_ports(self):
        words = [0] * 346
        words[7], words[25], words[26] = 6, 9, 3
        words[336:346] = [4, 13, 17, 28, 8, 2, 35, 42, 0, 0]
        nonce = b"rss-test"
        header = struct.pack("!4sHH8s", b"B51R", 1, 1, nonce)
        report = client["parse_reply"](header + struct.pack("!346I", *words), nonce)
        self.assertEqual(report["version"], 9)
        self.assertEqual(report["lan1"]["rss"], dict(active=True, rx_queues=4, packets_per_queue=[13, 17, 28, 8]))
        self.assertEqual(report["lan2"]["rss"], dict(active=True, rx_queues=2, packets_per_queue=[35, 42]))

    def test_benchmark_schema_reports_pmu_cycles_and_budget_exhaustions(self):
        words = [0] * 336
        words[7], words[25], words[26] = 6, 8, 3
        words[20:24] = [180, 3, 9 << 14 | 4 << 10 | 13, 25]
        words[304:310] = [1000, 1, 240, 0, 3, 0]
        nonce = b"cycles00"
        header = struct.pack("!4sHH8s", b"B51R", 1, 1, nonce)
        report = client["parse_reply"](header + struct.pack("!336I", *words), nonce)
        self.assertEqual(report["cpu"]["time_unit"], "pmu_cycles")
        first, second, *_ = report["cpu"]["cores"]
        self.assertEqual((first["busy_time"], first["idle_time"], first["budget_exhaustions"]),
                         ((1 << 32) + 1000, 240, 3))
        self.assertEqual((first["worker_packets"], first["queue_depth"]), (180, 4))
        self.assertIsNone(second["busy_time"])
        self.assertNotIn("gro", report)
        self.assertNotIn("ddr_phy_source", report["platform_clocks"])
        words[333:336] = [29, 3 | (25 << 8), 16]
        report = client["parse_reply"](header + struct.pack("!336I", *words), nonce)
        self.assertEqual(report["gro"]["worker_pairs_published"], 29)
        self.assertEqual(report["gro"]["driver_pairs_published"], 16)
        self.assertEqual(report["platform_clocks"]["ddr_phy_source"], "dedicated_dpll_ddr")
        self.assertTrue(report["platform_clocks"]["ddr_phy_clock_enabled"])
        self.assertIsNone(report["platform_clocks"]["ddr_phy_1x_hz"])

    def test_schema_seven_appends_per_port_tso_counters(self):
        words = [0] * 304
        words[7], words[25], words[26] = 6, 7, 3
        words[248:266] = range(18)
        words[264], words[266], words[267] = 12, 16, 496
        # wan / lan1 / lan2 TSO blocks: 12 words each.
        words[268:280] = [5, 4, 40, 1, 2, 3, 0, 1, 0, 1, 1, 1]
        words[280:292] = [9, 7, 70, 2, 4, 5, 1, 2, 1, 1, 1, 1]
        words[292:304] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        nonce = b"schema7!"
        header = struct.pack("!4sHH8s", b"B51R", 1, 1, nonce)
        report = client["parse_reply"](header + struct.pack("!304I", *words), nonce)
        self.assertEqual(report["version"], 7)
        self.assertIn("zc", report)
        words[279] = (3 << 2) | 3
        newer = client["parse_reply"](header + struct.pack("!304I", *words), nonce)
        self.assertEqual(newer["tso"]["wan"]["hw_completed"], 3)
        self.assertEqual(newer["tso"]["wan"]["hw_frames"], 4)
        self.assertNotIn("hw_completed", newer["tso"]["lan1"])
        words[25] = 10
        extended = words + [0] * 44
        extended[346] = (1 << 31) | 3 | (8 << 7) | (127 << 21)
        newer = client["parse_reply"](header + struct.pack("!348I", *extended), nonce)
        self.assertEqual(newer["lan1"]["direct_worker_tx_pending"], [3, 8, 0, 127])
        self.assertNotIn("direct_worker_tx_pending", newer["lan2"])
        extended[25] = 11
        extended[6] |= (1 << 28) | (1 << 29) | (1 << 30)
        extended += [2, 85, 62, 1, 58, 0, 97, 75, 120, 43]
        queues = client["parse_reply"](header + struct.pack("!358I", *extended), nonce)
        self.assertEqual(queues["lan1"]["tx_queue_retired_descriptors"], [85, 62])
        self.assertEqual(queues["wan"]["tx_channel_retired_descriptors"], [97, 75])
        self.assertEqual(queues["wan"]["rx_channel_retired_descriptors"], [120, 43])
        self.assertEqual(queues["lan1"]["direct_worker_tx_pending"], [3, 8, 0, 127])
        self.assertEqual(report["tso"], dict(
            wan=dict(requests=5, hw_frames=4, hw_segments=40, fallback_frames=1,
                     fallback_segments=2, invalid=3, desc_errors=0,
                     completion_failures=1, queue_drops=0, active=True,
                     capability=True, ipv6=True),
            lan1=dict(requests=9, hw_frames=7, hw_segments=70, fallback_frames=2,
                      fallback_segments=4, invalid=5, desc_errors=1,
                      completion_failures=2, queue_drops=1, active=True,
                      capability=True, ipv6=True),
            lan2=dict(requests=0, hw_frames=0, hw_segments=0, fallback_frames=0,
                      fallback_segments=0, invalid=0, desc_errors=0,
                      completion_failures=0, queue_drops=0, active=False,
                      capability=False, ipv6=False)))
        # A schema 6 report stays accepted and carries no tso block.
        old = [0] * 268
        old[7], old[25] = 6, 6
        self.assertNotIn("tso", client["parse_reply"](
            struct.pack("!4sHH8s", b"B51R", 1, 1, nonce) + struct.pack("!268I", *old), nonce))

    def test_native_echo_counts_queue_admission_separately_from_replies(self):
        words = [0] * 248
        words[7], words[25] = 6, 3
        words[139], words[140], words[147] = 0xffffff00, 0x0a000101, 5
        words[155:160] = [1, 0, 0, 0, 0]
        nonce = b"snapshot"
        header = struct.pack("!4sHH8s", b"B51R", 1, 1, nonce)

        def report():
            return client["parse_reply"](header + struct.pack("!248I", *words), nonce)["wan"]

        self.assertEqual(report()["internet_echo"]["state"], "waiting_for_lease")
        words[152] = 8
        self.assertEqual(report()["internet_echo"]["state"], "waiting_for_gateway_arp")
        words[152] = 24
        self.assertEqual(report()["internet_echo"]["state"], "running")
        words[156:160] = [3, 2, 1, 1]
        self.assertEqual(report()["internet_echo"], dict(target="8.8.8.8", state="complete",
                                                        socket_queued=3, tx_queued=2, replies=1))
        self.assertEqual(report()["gateway"], "10.0.1.1")
        self.assertEqual(report()["netmask"], "255.255.255.0")
        self.assertEqual(report()["gateway_echo_replies"], 5)

    def test_status_keeps_supervisor_shutdown_separate_from_poll_gaps(self):
        words = [0] * 248
        words[25], words[163] = 3, 120
        words[180:187] = [1, 7, 2, 360_000, 3, 12 << 8 | 5, 4]
        words[208:213] = [1, 7, (-23) & 0xffffffff, 1, 33595]
        nonce = b"snapshot"
        response = struct.pack("!4sHH8s", b"B51R", 1, 1, nonce) + struct.pack("!248I", *words)
        report = client["parse_reply"](response, nonce)
        self.assertEqual(report["wan"]["gmac_fault_name"], "Supervisor requested WAN shutdown")
        self.assertEqual(report["wan"]["shutdown"], {
            "supervisor_request": 7, "dma_stop_result": -23,
            "dma_was_running": True, "started_ms": 33595,
        })
        self.assertEqual(report["supervisor"]["shutdown_reason_name"], "Application status or freshness")
        self.assertEqual(report["supervisor"]["poll_gap_count"], 2)
        self.assertEqual(report["supervisor"]["first_poll_gap"]["elapsed_ms"], 15)
        self.assertEqual(report["supervisor"]["first_poll_gap"]["lan2_phase"], 12)

    def test_forwarded_echo_trace_shows_original_id_and_gmac_handoff(self):
        words = [0] * 248
        words[25] = 3
        words[213:221] = [4, 1, 0x0a2c0164, 0x08080808, 41319 << 16 | 3,
                          2 << 8 | 17, 0x02523553, 2]
        words[242:246] = [2, 1, 50000 << 16 | 3, 1]
        words[246] = 512 << 20 | 2 << 10 | 10
        nonce = b"snapshot"
        header = struct.pack("!4sHH8s", b"B51R", 1, 1, nonce)
        report = client["parse_reply"](header + struct.pack("!248I", *words), nonce)
        self.assertEqual(report["nat_table"], dict(capacity=512, active=10, retired=2, available=500))
        trace = report["icmp_trace"]
        self.assertEqual(trace["lan_rx"]["last"], dict(source="10.44.1.100",
            destination="8.8.8.8", id=41319, sequence=3, result="checksum",
            lan_ports=[2], destination_mac="02:52:35:53:00:02"))
        self.assertEqual(trace["wan_gmac_queue"]["last"], dict(id=50000, sequence=3, result="accepted"))
        self.assertIsNone(trace["wan_relay_rx"]["last"])
