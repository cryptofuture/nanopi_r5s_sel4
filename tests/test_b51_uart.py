import runpy
import unittest
from pathlib import Path


decode = runpy.run_path(str(Path(__file__).resolve().parents[1] / "scripts/decode-b51-uart"))["decode"]


class UartReport(unittest.TestCase):
    def test_complete_and_truncated_reports(self):
        words = [0] * 248
        words[2], words[25], words[27] = 1 << 22, 1, 1
        words[128], words[136], words[22] = 3, 1, 30
        rows = [f"B51v1 {i:03d} " + " ".join(f"{w:08x}" for w in words[i:i+4])
                for i in range(0, len(words), 4)]
        result = decode(["U-Boot", *rows])
        self.assertEqual(result["milestones"], [23])
        self.assertEqual((result["rx_off"]["stage"], result["rx_on"]["stage"]), (3, 1))
        self.assertEqual(result["rx_on"]["sys"], 30)
        for broken in (rows[:-1], rows[1:], rows + rows, [rows[0], *rows[2:]]):
            with self.assertRaises(ValueError):
                decode(broken)
