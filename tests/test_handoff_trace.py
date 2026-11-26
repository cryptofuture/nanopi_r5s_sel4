import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
from policy import sha256
from trace_profile import PROFILES, validate_trace_manifest


class HandoffControlTests(unittest.TestCase):
    def test_real_uboot_control_flow_errors_and_preflight_stop(self):
        with tempfile.TemporaryDirectory() as temporary:
            temporary = Path(temporary)
            source = (ROOT / 'firmware/r5sboot-trace.c').read_text()
            for name in re.findall(r'#include [<"]([^>"]+)', source):
                path = temporary / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('')
            (temporary / 'r5s_trace.h').write_bytes((ROOT / 'firmware/r5s_trace.h').read_bytes())
            (temporary / 'r5s_payload.h').write_text('#define R5S_PAYLOAD_SIZE 42\n#define R5S_PAYLOAD_SHA256 {0}\n')
            for stop in (0, 3):
                executable = temporary / f'command-{stop}'
                subprocess.run(['cc', '-std=gnu11', f'-DR5S_STOP_CHECKPOINT={stop}', '-I', str(temporary),
                                str(ROOT / 'tests/trace_uboot_harness.c'), '-o', str(executable)], check=True)
                scenarios = (0,) if stop else (0, 1, 2, 3, 4, 5, 6, 8, 9)
                for scenario in scenarios:
                    subprocess.run([str(executable), str(scenario)], check=True, timeout=5,
                                   stdout=subprocess.DEVNULL)

    def test_actual_writer_model_compatible_and_full_cid(self):
        import test_overlay_writer as fixtures
        case = fixtures.OverlayWriterTests()
        case.setUp()
        self.addCleanup(case.doCleanups)
        key = f'/sys/class/block/{case.base.device.name}/device/cid'
        actual = '8903034e43617264100d3aa313016c07'
        case.reads[key] = actual
        case.expected_cid = actual
        meta = case.inspect()
        self.assertEqual(meta['serial'], actual)
        import test_tools
        test_tools.flash.validate(meta, 'r5s', (actual, 'NCard', 62534975488), 96 * 1024**2)
        with self.assertRaises(ValueError):
            test_tools.flash.validate(meta, 'r5s', ('0x0d3aa313', 'NCard', 62534975488), 96 * 1024**2)


@unittest.skipUnless((ROOT / 'out/handoff-trace/r5s-boot-trace.img').exists(), 'Build trace images first')
class HandoffArtifactTests(unittest.TestCase):
    def test_profiles_payload_binding_and_reproducibility(self):
        from image import verify
        base = ROOT / 'out/handoff-trace'
        for profile, stop in PROFILES.items():
            with self.subTest(profile=profile):
                path = base / f'r5s-{profile}.img'
                manifest = verify(path)
                self.assertEqual(manifest['stop_checkpoint'], stop)
                payload = base / profile / 'loader.img'
                self.assertEqual(manifest['loader']['sha256'], sha256(payload))
                self.assertEqual(sha256(path), sha256(base / f'r5s-{profile}.reproduced.img'))
                for change in (dict(stop_checkpoint=9), dict(format=1), dict(profile='uboot-diag')):
                    with self.assertRaises(ValueError):
                        validate_trace_manifest(dict(manifest, **change))

    def test_component_mappings_and_counter_permissions(self):
        base = ROOT / 'out/handoff-trace'
        layouts = json.loads((base / 'layout-report.json').read_text())
        for report in layouts.values():
            self.assertEqual(report['userspace_gpio']['frame_objects'], 1)
            self.assertFalse(report['userspace_gpio']['cached'])
            self.assertEqual(report['loader_symbols']['_start']['address'], 0x30000000)
        from handoff_trace import symbols
        sdk = base / 'work/microkit/release/microkit-sdk-2.4.0/board/nanopi_r5s_trace/debug/elf'
        from elftools.elf.elffile import ELFFile
        for component in ('loader', 'sel4', 'monitor'):
            with self.subTest(component=component), (sdk / f'{component}.elf').open('rb') as stream:
                elf = ELFFile(stream)
                concrete = []
                for cu in elf.get_dwarf_info().iter_CUs():
                    for die in cu.iter_DIEs():
                        if die.tag != 'DW_TAG_inlined_subroutine' or 'DW_AT_abstract_origin' not in die.attributes:
                            continue
                        origin = die.get_DIE_from_attribute('DW_AT_abstract_origin')
                        name = origin.attributes.get('DW_AT_name')
                        if name and name.value == b'r5s_checkpoint':
                            self.assertTrue('DW_AT_low_pc' in die.attributes or 'DW_AT_ranges' in die.attributes)
                            concrete.append(die.offset)
                self.assertTrue(concrete, f'No linked inline checkpoint code ranges in {component}')

    def test_compiled_stop_paths_have_no_later_handoff(self):
        base = ROOT / 'out/handoff-trace'
        uboot_symbols = subprocess.check_output(['aarch64-none-elf-nm', str(base / 'preflight-stop/u-boot.elf')], text=True)
        self.assertNotIn(' r5s_enter\n', uboot_symbols)
        loader_main = subprocess.check_output(['aarch64-none-elf-objdump', '-d', '--disassemble=main',
                                                str(base / 'loader-entry-stop/loader.elf')], text=True)
        self.assertNotRegex(loader_main, r'\bbl\s+[^\n]*<(uart_init|arch_init|start_kernel|memcpy)>')

    def test_stale_embedded_hash_and_length_refused(self):
        from handoff_trace import layout
        base = ROOT / 'out/handoff-trace'
        sdk = base / 'work/microkit/release/microkit-sdk-2.4.0'
        profile = base / 'boot-trace'
        original = (profile / 'r5s_payload.h').read_text()
        first = re.search(r'0x([0-9a-f]{2})', original)
        bad_hash = original[:first.start()] + f'0x{int(first[1], 16) ^ 1:02x}' + original[first.end():]
        bad_size = re.sub(r'(R5S_PAYLOAD_SIZE )([0-9]+)', lambda m: m[1] + str(int(m[2]) + 1), original)
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            for name in ('build', 'microkit-report.txt', 'u-boot-rockchip.bin', 'u-boot.elf'):
                (directory / name).symlink_to(profile / name)
            for text in (bad_hash, bad_size):
                (directory / 'r5s_payload.h').write_text(text)
                with self.assertRaisesRegex(ValueError, 'Stale embedded payload'):
                    layout(base, sdk, profile / 'loader.img', profile / 'loader.elf', directory)
