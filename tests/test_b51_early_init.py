import json
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from b51_status import describe


class EarlyInit(unittest.TestCase):
    def test_variant_w_reports(self):
        for name in ("b51-live.json", "b51-terminal.json"):
            with self.subTest(name=name):
                raw = json.loads((ROOT / name).read_text())
                self.assertEqual(raw["state"], "terminal")
                decoded = describe([int(word, 16) for word in raw["words"]])
                self.assertEqual(decoded["first_error_code"], 161)
                self.assertEqual(decoded["first_failed_stage"], "Inbound iATU")
                self.assertEqual(decoded["milestones"], [1, 2, 3, 16])
                self.assertEqual(decoded["last_completed_stage"], "DMA pool validated")
                self.assertEqual(decoded["tail_close_initial"], [None, None])
                self.assertFalse(decoded["lan1_tx_attempted"])
                self.assertFalse(decoded["dma_diagnostic_attempted"])

    def test_schema_two_retains_mapping_context(self):
        words = [0xffffffff] * 248
        words[:25] = [0] * 25
        words[1], words[2], words[7], words[25] = 162, 0x8007, 255, 2
        words[27], words[128], words[136], words[135], words[143], words[247] = 0, 0, 0, 162, 13, 1
        words[32:44] = [7, 5, 0xc0300104, 3, 0x80000000, 0, 0xc0000000, 3,
                         0x80000000, 0x800fffff, 0x24200000, 0]
        decoded = describe(words)
        self.assertEqual(decoded["first_error_code"], 162)
        self.assertEqual(decoded["inbound_error"]["register"], "0x3c0300104")
        self.assertEqual(decoded["inbound_error"]["operation"], "Enable bank")
        self.assertEqual(decoded["inbound_error"]["observed"], 0)
        self.assertFalse(decoded["lan1_tx_attempted"])
