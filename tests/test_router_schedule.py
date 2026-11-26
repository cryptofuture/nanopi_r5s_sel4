from pathlib import Path
import os
import sys
import unittest
from unittest.mock import patch
from xml.etree import ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from dual_lan_profile import b51_memory, b51_rx_fits, configure_schedule, scheduling_profile


class Schedule(unittest.TestCase):
    def test_wide_writeback_keeps_private_arenas_out_of_peer_apertures(self):
        for slots in ('256', '512'):
            lan1, lan2, (shared, bus1, bus2, pad) = b51_memory(slots, 'writeback')
            self.assertEqual(shared % 0x200000, 0)
            self.assertEqual(lan2[1] % 0x200000, 0)
            self.assertEqual(bus1, lan1[2] + shared - lan1[1])
            self.assertEqual(bus2, lan2[2] - (lan2[1] - shared))
            self.assertLessEqual(lan1[1] + lan1[3], shared)
            self.assertLessEqual(shared + 0x100000, lan2[1])
            self.assertEqual(pad, lan2[1] + lan2[3])

    def test_b51_queue_options_fit_private_dma_regions(self):
        for slots, queues in [("64", "4"), ("128", "2"), ("256", "4"), ("512", "1")]:
            self.assertTrue(b51_rx_fits(slots, queues))
        for slots, queues in [("128", "4"), ("512", "2"), ("512", "4"), ("128", "3")]:
            self.assertFalse(b51_rx_fits(slots, queues))
        self.assertTrue(b51_rx_fits("128", "1", 2))
        self.assertTrue(b51_rx_fits("64", "4", 2))
        self.assertFalse(b51_rx_fits("128", "2", 2))
        self.assertFalse(b51_rx_fits("512", "1", 2))

    def test_router_schedule(self):
        system = ET.Element("system")
        for name, budget in zip(("heartbeat", "health", "gmac", "wan", "lan1", "lan2", "logical_lan"),
                                (200, 100, 200, 200, 50, 50, 100)):
            ET.SubElement(system, "protection_domain", name=name, budget=str(budget), period="1000")
        before = ET.tostring(system)
        configure_schedule(system, "baseline")
        self.assertEqual(ET.tostring(system), before)
        for _ in range(2):
            configure_schedule(system, "throughput")
            self.assertEqual([int(pd.get("budget")) for pd in system], [200, 100, 200, 200, 75, 75, 140])
        quad = ET.fromstring(before)
        configure_schedule(quad, "quad-core")
        self.assertEqual([int(pd.get("cpu")) for pd in quad], [1, 1, 0, 0, 2, 3, 1])
        self.assertEqual([int(pd.get("budget")) for pd in quad], [200, 100, 400, 400, 800, 800, 600])
        quad[0].set("budget", "301")
        with self.assertRaises(ValueError):
            configure_schedule(quad, "quad-core")
        system[0].set("budget", "220")
        with self.assertRaises(ValueError):
            configure_schedule(system, "throughput")
        system[0].set("budget", "0")
        with self.assertRaises(ValueError):
            configure_schedule(system, "throughput")

    def test_parallel_profiles_stay_within_per_core_budget(self):
        system = ET.Element("system")
        names = ["heartbeat", "health", "gmac", "wan", "lan1", "lan2", "logical_lan"]
        for name, budget in zip(names, (200, 100, 400, 400, 800, 800, 600)):
            ET.SubElement(system, "protection_domain", name=name, budget=str(budget), period="1000")
        for core in range(4):
            ET.SubElement(system, "protection_domain", name=f"forward{core}", budget="1", period="1000")
        configure_schedule(system, "multicore")
        totals = [0] * 4
        for pd in system:
            totals[int(pd.get("cpu"))] += int(pd.get("budget"))
        self.assertEqual(totals, [975, 940, 950, 950])
        self.assertEqual([int(system.find(f"protection_domain[@name='forward{i}']").get("cpu"))
                          for i in range(4)], list(range(4)))

        configure_schedule(system, "high-throughput")
        totals = [0] * 4
        for pd in system:
            totals[int(pd.get("cpu"))] += int(pd.get("budget"))
        self.assertEqual(totals, [970, 970, 950, 950])

    def test_profile_selection(self):
        with patch.dict(os.environ, clear=True):
            self.assertEqual(scheduling_profile("dual-lan-router"), "baseline")
            os.environ["R5S_ROUTER_SCHEDULE"] = "throughput"
            self.assertEqual(scheduling_profile("dual-lan-router"), "throughput")
            with self.assertRaises(ValueError):
                scheduling_profile("lan1-rx")
            os.environ["R5S_ROUTER_SCHEDULE"] = "unknown"
            with self.assertRaises(ValueError):
                scheduling_profile("dual-lan-router")
