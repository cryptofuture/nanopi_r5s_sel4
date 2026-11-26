import sys
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1] / 'scripts'
sys.path.insert(0, str(SCRIPTS))
from dual_lan_profile import valid_tso_request


class TsoConfigTests(unittest.TestCase):
    def test_accepts_independent_ports_on_the_router_stage(self):
        for requested in (['wan'], ['lan1'], ['lan2'], ['wan', 'lan1'],
                          ['wan', 'lan1', 'lan2'], ['lan1', 'lan2']):
            with self.subTest(requested=requested):
                self.assertTrue(valid_tso_request(requested, True, 'dual-lan-router'))
                self.assertTrue(valid_tso_request(requested, False, 'dual-lan-router'))

    def test_rejects_wrong_stage_or_shape(self):
        self.assertFalse(valid_tso_request(['wan'], True, 'lan2-tx'))
        self.assertFalse(valid_tso_request(['wan'], True, None))
        self.assertFalse(valid_tso_request([], True, 'dual-lan-router'))
        self.assertFalse(valid_tso_request(None, True, 'dual-lan-router'))
        self.assertFalse(valid_tso_request(['wan', 'wan'], True, 'dual-lan-router'))
        self.assertFalse(valid_tso_request(['wan', 'lan2', 'lan1'], True, 'dual-lan-router'))
        self.assertFalse(valid_tso_request(['lan9'], True, 'dual-lan-router'))
        self.assertFalse(valid_tso_request(['wan', 'lan9'], True, 'dual-lan-router'))

    def test_rejects_non_boolean_ipv6_policy(self):
        self.assertFalse(valid_tso_request(['wan'], None, 'dual-lan-router'))
        self.assertFalse(valid_tso_request(['wan'], 1, 'dual-lan-router'))
        self.assertFalse(valid_tso_request(['wan'], 'yes', 'dual-lan-router'))


if __name__ == '__main__':
    unittest.main()
