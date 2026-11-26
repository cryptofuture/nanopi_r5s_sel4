from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
from image import BOOT, verify


@unittest.skipUnless((ROOT / 'out/r5s-microkit.img').exists(), 'Build real image first')
class ImageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.image = Path(self.temp.name) / 'test.img'
        shutil.copyfile(ROOT / 'out/r5s-microkit.img', self.image)

    def test_real_image(self):
        self.assertFalse(verify(self.image)['hardware_tested'])

    def test_corrupt_primary_gpt(self):
        with self.image.open('r+b') as target:
            target.seek(512 + 24)
            target.write(b'\xff')
        with self.assertRaisesRegex(ValueError, 'checksum'):
            verify(self.image)

    def test_corrupt_backup_gpt(self):
        with self.image.open('r+b') as target:
            target.seek(-512, 2)
            target.write(b'BROKEN!!')
        with self.assertRaisesRegex(ValueError, 'GPT'):
            verify(self.image)

    def test_corrupt_raw_bootloader(self):
        with self.image.open('r+b') as target:
            target.seek(BOOT)
            target.write(b'BROKEN!!')
        with self.assertRaisesRegex(ValueError, 'Bootloader hash'):
            verify(self.image)

    def test_declared_negative_payloads(self):
        for mode in ('missing', 'corrupt'):
            destination = Path(self.temp.name) / f'{mode}.img'
            subprocess.run([sys.executable, str(ROOT / 'scripts/make-negative-image'),
                            str(self.image), str(destination), '--mode', mode], check=True,
                           stdout=subprocess.DEVNULL)
            self.assertEqual(verify(destination)['negative_test'], mode)


if __name__ == '__main__':
    unittest.main()
