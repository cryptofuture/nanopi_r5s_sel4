"""Host checks for the shared zero-copy DMA packet pool and its integration."""
import json
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class ZcPool(unittest.TestCase):
    def test_pool_ownership_and_reference_queues(self):
        """The zc.rs state machine, queues, barriers and DMA translation."""
        with tempfile.TemporaryDirectory() as td:
            binary = Path(td) / "zc"
            subprocess.run(
                ["rustc", "--edition", "2021", "--test",
                 str(ROOT / "pds/gmac_dma_corrected/src/zc.rs"), "-o", str(binary)],
                check=True)
            result = subprocess.run([binary], check=True, capture_output=True, text=True)
            self.assertIn("0 failed", result.stdout + result.stderr)

    def test_gmac_external_buffer_api_stays_sanitized(self):
        """The C driver's external API runs under the sanitizer suite."""
        with tempfile.TemporaryDirectory() as td:
            binary = Path(td) / "dma"
            subprocess.run(
                ["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                 "-fsanitize=undefined", f"-I{ROOT}/drivers/gmac0/dma",
                 str(ROOT / "tests/gmac_dma_test.c"),
                 str(ROOT / "drivers/gmac0/dma/dma.c"), "-o", str(binary)],
                check=True)
            subprocess.run([binary], check=True, timeout=10)

    def test_rtl8125_external_buffer_api(self):
        """The RTL8125 ring's external path keeps the copy path intact."""
        with tempfile.TemporaryDirectory() as td:
            binary = Path(td) / "pcie"
            subprocess.run(
                ["rustc", "--edition", "2021", "--test",
                 str(ROOT / "drivers/rtl8125/lib.rs"), "-o", str(binary)],
                check=True)
            result = subprocess.run([binary], check=True, capture_output=True, text=True)
            self.assertIn("0 failed", result.stdout + result.stderr)

    def test_status_schema_reports_zero_copy_counters(self):
        """Schema 6 carries the pool counters; schema 5 stays byte-compatible."""
        sys.path.insert(0, str(ROOT / "scripts"))
        from b51_status import describe
        words = [0] * 268
        words[25] = 6
        words[7] = 1
        words[248] = 10   # packets received
        words[249] = 8    # packets sent
        words[253] = 2    # copied packets
        words[254] = 3000  # copied bytes
        words[255] = 1    # queue drops
        words[256] = 3    # mapping failures
        words[257] = 4    # ownership errors
        words[260] = 5    # pool-exhausted fallbacks
        words[266] = 16   # in use
        words[267] = 496  # free
        report = describe(words)
        zc = report["zc"]
        self.assertTrue(zc["enabled"], "schema 6 always reports the pool")
        self.assertEqual(zc["packets_received"], 10)
        self.assertEqual(zc["copied_bytes"], 3000)
        self.assertEqual(zc["queue_drops"], 1)
        self.assertEqual(zc["mapping_failures"], 3)
        self.assertEqual(zc["ownership_errors"], 4)
        self.assertEqual(zc["fallback"]["pool_exhausted"], 5)
        self.assertEqual(zc["pool"], dict(in_use=16, free=496, peak=0, buffers=512))
        old = [0] * 248
        old[25] = 5
        self.assertNotIn("zc", describe(old))

    def test_reader_accepts_both_status_schemas(self):
        sys.path.insert(0, str(ROOT / "scripts"))
        import runpy
        client = runpy.run_path(str(ROOT / "scripts/read-b51-udp"))
        header = b"B51R" + (1).to_bytes(2, "big") + (1).to_bytes(2, "big") + b"nonce123"
        for count in (248, 268):
            words = [0] * count
            words[25] = 5 if count == 248 else 6
            body = b"".join(w.to_bytes(4, "big") for w in words)
            report = client["parse_reply"](header + body, b"nonce123")
            self.assertIsNotNone(report, count)
        self.assertIsNone(client["parse_reply"](header + bytes(100), b"nonce123"))


class ZcLayout(unittest.TestCase):
    def test_router_build_maps_the_shared_pool_and_metadata(self):
        """A completed zero-copy build reserves the pool and maps it per role."""
        candidates = sorted(ROOT.glob("out/zc-*/system.system"))
        if not candidates:
            self.skipTest("zero-copy build pending")
        system = ET.parse(candidates[-1]).getroot()
        regions = {r.get("name"): r for r in system.findall("memory_region")}
        pool = regions.get("zc_pool")
        self.assertIsNotNone(pool)
        self.assertEqual(pool.get("phys_addr"), "0x24400000")
        # The default layout absorbs the padding MiB (1024 buffers); the
        # 256/512-slot layouts keep the 1 MiB pool and reserve the padding.
        if pool.get("size") == "0x200000":
            self.assertNotIn("zc_padding", regions, "the second MiB is part of the pool")
        else:
            self.assertEqual(pool.get("size"), "0x100000")
            # Both MiB of the second Normal-NC block stay reserved.
            self.assertEqual(regions["zc_padding"].get("phys_addr"), "0x24500000")
        maps = {}
        for pd in system.findall("protection_domain"):
            maps[pd.get("name")] = {m.get("mr"): m.get("perms") for m in pd.findall("map")}
        for owner in ("gmac", "wan", "lan1", "lan2", "logical_lan", "forward0", "forward3"):
            self.assertIn("zc_pool", maps.get(owner, {}), owner)
        self.assertEqual(maps["wan"]["zc_pool"], "r", "the WAN app cannot write payloads")
        self.assertEqual(maps["gmac"]["zc_pool"], "rw")
        for owner in ("gmac", "wan", "lan1", "lan2", "logical_lan"):
            self.assertIn("zc_meta", maps.get(owner, {}), owner)
        # The pool is mapped non-cacheable everywhere it is written.
        for pd in system.findall("protection_domain"):
            for m in pd.findall("map"):
                if m.get("mr") == "zc_pool":
                    self.assertEqual(m.get("cached"), "false", pd.get("name"))

    def test_manifest_records_dma_addresses_per_device(self):
        candidates = sorted(ROOT.glob("out/zc-*/r5s-rtl8125-lan-repair.manifest.json"))
        if not candidates:
            self.skipTest("zero-copy build pending")
        manifest = json.loads(candidates[-1].read_text())
        self.assertTrue(manifest["zc_enabled"])
        pool = manifest["zc_pool"]
        self.assertEqual(pool["pa"], 0x24400000)
        self.assertEqual(pool["gmac_dma"], 0x24400000, "GMAC uses 32-bit physical identity")
        self.assertEqual(pool["lan1_dma"], 0x82000000, "LAN1 iATU window at bus 0x80000000")
        self.assertEqual(pool["lan2_dma"], 0x81100000, "LAN2 iATU window at bus 0x81000000")
        self.assertIn(manifest["zc_buffers"], (512, 1024))
        self.assertEqual(pool["size"], manifest["zc_buffers"] * 2048)
        self.assertEqual(manifest["lan_status_version"], 6)


if __name__ == "__main__":
    unittest.main()
