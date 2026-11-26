import re
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from wan_stack_profile import WAN_STACK, check_stack


class WanStackTests(unittest.TestCase):
    def test_failed_image_has_insufficient_stack_for_linked_calls(self):
        old = ROOT / "out/router-wan-dhcp-poll-fix/build-a/wan-dhcp"
        objdump, = (ROOT / "tools").glob("arm-gnu-toolchain-*/bin/aarch64-none-elf-objdump")
        asm = subprocess.check_output([objdump, "-dC", old / "probe.elf"], text=True)
        functions = dict(re.findall(r"^[\da-f]+ <([^\n]+)>:\n(.*?)(?=\n\n)", asm, re.M | re.S))
        init = functions["r5s_heartbeat::init"]
        target, = [name for name in functions if ">::dispatch_ip::<r5s_heartbeat::device::Tx>" in name]
        self.assertRegex(init, r"bl\s+[\da-f]+ <" + re.escape(target) + ">")

        def frame(body):
            saves = re.search(r"\[sp, #-(\d+)\]!", body).group(1)
            return int(saves) + sum(int(n, 16) << (12 if shift else 0)
                for n, shift in re.findall(r"sub\s+sp, sp, #0x([\da-f]+)(, lsl #12)?",
                                           body.split("\tldar", 1)[0].split("\tldr\tx26", 1)[0]))
        entry = functions["__sel4_runtime_common__rust_entrypoint"]
        tls, = [n for n in functions if n.startswith("sel4_runtime_common::tls::with_tls::<")]
        reserve, = [n for n in functions if n.startswith("<sel4_initialize_tls::TlsImage>::with_initialize_on_stack::<") and f"<{n}>" in functions[tls]]
        self.assertIn(f"<{tls}>", entry)
        self.assertIn(f"<{reserve}>", functions[tls])
        self.assertIn("[sp, #-16]!", entry)
        for name in [tls, reserve]:
            self.assertIn("sub\tsp, sp, #0x60", functions[name])
        self.assertGreater(frame(init) + 16 + 96 + 96, 8192)
        required = frame(init) + frame(functions[target])
        self.assertEqual(required, 10032)
        self.assertLess(8192, required)
        self.assertGreater(WAN_STACK, required)
        with self.assertRaisesRegex(ValueError, "stack configuration"):
            check_stack(old / "system.system", old / "microkit-report.txt")
