import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
from diag_profile import LOCK_SHA256, NORMAL_SHA256, PATTERN, PROFILE, validate_manifest
from image import FAT_SIZE, START, normalize_fat_times, verify
from policy import sha256

IMAGE = ROOT / 'out/diag-uboot/r5s-uboot-diag.img'


class DiagnosticCommandTests(unittest.TestCase):
    def test_actual_c_dispatch_masks_and_two_pattern_cycles(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            for name in ('command.h', 'asm/io.h', 'linux/kernel.h', 'linux/delay.h'):
                path = directory / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('')
            (directory / 'linux/delay.h').write_bytes(
                (ROOT / 'vendor/u-boot/include/linux/delay.h').read_bytes())
            (directory / 'linux/kernel.h').write_text('#define DIV_ROUND_UP(n,d) (((n)+(d)-1)/(d))\n')
            executable = directory / 'command-test'
            subprocess.run(['cc', '-std=gnu11', '-Werror=implicit-function-declaration',
                            '-DR5S_UBOOT_DIAGNOSTIC', '-I', str(directory),
                            str(ROOT / 'tests/diag_harness.c'), '-o', str(executable)], check=True)
            subprocess.run([str(executable)], check=True, timeout=5)

    def test_normal_image_and_source_lock_unchanged(self):
        self.assertEqual(sha256(ROOT / 'out/r5s-microkit.img'), NORMAL_SHA256)
        self.assertEqual(sha256(ROOT / 'sources.lock'), LOCK_SHA256)


@unittest.skipUnless(IMAGE.exists(), 'Build diagnostic image first')
class DiagnosticImageTests(unittest.TestCase):
    def test_linked_firmware_and_earlier_stages(self):
        from diag import audit
        evidence = audit(ROOT, ROOT / 'out/diag-uboot')
        self.assertEqual([name for name, item in evidence['fit_components'].items() if item['changed']],
                         ['u-boot'])

    def test_manifest_and_profile(self):
        manifest = verify(IMAGE)
        self.assertEqual(manifest['profile'], PROFILE)
        self.assertEqual(manifest['pattern'], PATTERN)
        self.assertIsNone(manifest['loader'])

    def test_wrong_or_disguised_profile_rejected(self):
        manifest = verify(IMAGE)
        for updates in (dict(format=1), dict(profile='unknown'), dict(loader={'size': 1}),
                        dict(payload_handoff=True), dict(load_address=0x30000000),
                        dict(pattern=dict(initial_off_ms=0, repeat=PATTERN['repeat']))):
            with self.assertRaises(ValueError):
                validate_manifest(dict(manifest, **updates))

    def test_stale_firmware_packaging_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            image = Path(directory) / 'stale.img'
            shutil.copyfile(IMAGE, image)
            with image.open('r+b') as target, (ROOT / 'out/r5s-microkit.img').open('rb') as normal:
                normal.seek(32768)
                target.seek(32768)
                target.write(normal.read(9052672))
            with self.assertRaisesRegex(ValueError, 'Bootloader hash mismatch'):
                verify(image)

    def test_payload_on_diagnostic_fat_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            image, fat, payload = directory / 'extra.img', directory / 'boot.fat', directory / 'loader.img'
            shutil.copyfile(IMAGE, image)
            with image.open('rb') as source:
                source.seek(START)
                fat.write_bytes(source.read(FAT_SIZE))
            payload.write_bytes(b'not executable')
            subprocess.run(['mcopy', '-i', str(fat), str(payload), '::loader.img'], check=True)
            normalize_fat_times(fat)
            with image.open('r+b') as target:
                target.seek(START)
                target.write(fat.read_bytes())
            with self.assertRaisesRegex(ValueError, 'only manifest.json and boot.txt'):
                verify(image)

    def test_normal_payload_checks_still_required(self):
        from image import verify_microkit_payload
        manifest = json.loads((ROOT / 'out/r5s-microkit.manifest.json').read_text())
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(subprocess.CalledProcessError):
                verify_microkit_payload(manifest, Path(directory), Path(directory) / 'missing-fat')


if __name__ == '__main__':
    unittest.main()
