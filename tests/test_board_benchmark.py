import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class BoardBenchmark(unittest.TestCase):
    def test_loaded_diagnostics_and_bidirectional_traffic(self):
        with tempfile.TemporaryDirectory() as directory:
            working = Path(directory)
            runner = working / "benchmark-router-board"
            shutil.copyfile(ROOT / "scripts/benchmark-router-board", runner)
            fake = {
                "ping": 'print("time=0.125 ms\\n4 packets transmitted, 4 received, 0% packet loss")',
                "read-b51-udp": """import json, os, sys
from pathlib import Path
if "--queues" in sys.argv:
    path = Path(__file__).with_suffix(".sample")
    sample = int(path.read_text()) if path.exists() else 0
    path.write_text(str(sample + 1))
    other = 8 - sample if os.environ.get("FIXTURE_COUNTER_RESET") else 8
    print(json.dumps({"state": "live", "wan": {"rx_channel_retired_descriptors": [sample, other]},
         "lan1": {"tx_queue_retired_descriptors": [sample * 3, sample * 5],
                  "rss": {"packets_per_queue": [sample * 7, sample * 9]}}}))
else:
    assert "--lan-tx" in sys.argv
    print(json.dumps({"lan1": {"link_up": True, "direct_worker_tx_pending": [2, 3, 0, 0]}}))
""",
                "iperf3": """import json, sys, time
assert sys.argv[sys.argv.index("-p") + 1] == "5202"
assert sys.argv[sys.argv.index("-B") + 1] == "192.0.2.3"
time.sleep(1.2)
print(json.dumps({"end": {
    "sum_sent": {"retransmits": 2}, "sum_received": {"bits_per_second": 100},
    "sum_sent_bidir_reverse": {"retransmits": 3},
    "sum_received_bidir_reverse": {"bits_per_second": 200}
}}))""",
            }
            for name, body in fake.items():
                path = working / name
                path.write_text(f"#!{sys.executable}\n{body}\n")
                path.chmod(0o755)
            output = working / "results"
            command = [
                sys.executable, runner, "192.0.2.1", output,
                "--ping", "192.0.2.1", "--gateway", "192.0.2.254",
                "--case", "tcp-bidir", "--seconds", "2",
                "--label", "fixture", "--diagnostics", "192.0.2.254", "--lan-tx",
                "--status-interval", "1", "--server-port", "5202", "--bind", "192.0.2.3",
            ]
            env = {**os.environ, "PATH": str(working) + os.pathsep + os.environ["PATH"]}
            subprocess.run(command, env=env, check=True, timeout=10)
            case = json.loads((output / "summary.json").read_text())["cases"]["tcp-bidir"]
            self.assertEqual(case["iperf"]["client_to_server"]["tcp_retransmits"], 2)
            self.assertEqual(case["iperf"]["server_to_client"]["receiver_bps"], 200)
            self.assertEqual(case["gateway"]["p50_ms"], 0.125)
            self.assertEqual(case["status_after_exit_code"], 0)
            self.assertTrue(case["diagnostics_complete"])
            self.assertEqual(len(case["loaded_status"]), 1)
            live = json.loads((output / case["loaded_status"][0]["file"]).read_text())
            self.assertEqual(live["lan1"]["direct_worker_tx_pending"], [2, 3, 0, 0])
            output = working / "queue-results"
            command[3] = output
            command[command.index("--lan-tx")] = "--queues"
            failed = subprocess.run(command, env={**env, "FIXTURE_COUNTER_RESET": "1"}, check=False, timeout=10)
            self.assertEqual(failed.returncode, 1)
            case = json.loads((output / "summary.json").read_text())["cases"]["tcp-bidir"]
            self.assertFalse(case["diagnostics_complete"])
            self.assertEqual(case["exit_code"], 0)
            self.assertEqual(case["queue_counter_deltas"], {
                "wan": {"rx_channel_retired_descriptors": [2, None]},
                "lan1": {"tx_queue_retired_descriptors": [6, 10], "rx_queue_packets": [14, 18]},
            })
            self.assertEqual(case["queue_counter_samples"], ["tcp-bidir-status-before.json", "tcp-bidir-status.json"])
            output = working / "complete-queue-results"
            command[3] = output
            subprocess.run(command, env=env, check=True, timeout=10)
            case = json.loads((output / "summary.json").read_text())["cases"]["tcp-bidir"]
            self.assertTrue(case["diagnostics_complete"])
            self.assertEqual(case["queue_counter_deltas"]["wan"]["rx_channel_retired_descriptors"], [2, 0])
