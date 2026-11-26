import contextlib
import importlib.machinery
import importlib.util
import io
import json
from pathlib import Path
import stat
import struct
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
from policy import RAM, check_layout, check_ram, decode_ram, intersect
from image import gpt, DISK_SIZE

loader = importlib.machinery.SourceFileLoader('flash_sd', str(ROOT / 'scripts/flash-sd'))
spec = importlib.util.spec_from_loader(loader.name, loader)
flash = importlib.util.module_from_spec(spec)
loader.exec_module(flash)


class MemoryTests(unittest.TestCase):
    def test_decode_observed_ram(self):
        cells = [0, 0x200000, 0, 0x8200000, 0, 0x9400000, 0, 0xe6c00000,
                 1, 0xf0000000, 0, 0x10000000, 0, 0, 0, 0]
        self.assertEqual(decode_ram(cells), RAM)
        self.assertEqual(sum(b - a for a, b in RAM), 4078 * 1024 * 1024)

    def test_bad_cells(self):
        for cells in ([0], [-1, 0, 0, 1], [0xffffffff, 0xffffffff, 0, 2]):
            with self.assertRaises(ValueError):
                decode_ram(cells)

    def test_reserved_and_half_open(self):
        self.assertFalse(intersect((0, 2), (2, 4)))
        self.assertTrue(intersect((0x10000000, 0x10800000), (0x107fffff, 0x10900000)))
        check_ram((0x20000000, 0x40000000))
        for region in ((0, 0x30000000), (0x8400000, 0x9400000),
                       (0x10000000, 0x10800000), (0xf0000000, 0x200000000)):
            with self.assertRaises(ValueError):
                check_ram(region)

    def test_overlap(self):
        check_layout([('GPT', 0, 34 * 512), ('loader', 64 * 512, 16 * 1024**2)])
        with self.assertRaises(ValueError):
            check_layout([('loader', 32768, 17 * 1024**2), ('FAT', 16 * 1024**2, 80 * 1024**2)])


class WriterTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.device = Path(self.temp.name) / 'sdcard'
        self.device.touch()
        self.image = Path(self.temp.name) / 'image.img'
        self.image.write_bytes(b'image')
        self.target = dict(name=str(self.device), type='disk', size=62534975488,
                           model='Reader', serial='serial', tran='usb', rm=True,
                           **{'maj:min': '8:16'}, mountpoints=[None], children=[])
        self.root = dict(name='/dev/rootdisk', type='disk', size=100000000000,
                         **{'maj:min': '8:0'})
        self.mounts = [dict(target='/', source='/dev/rootdisk', fstype='ext4',
                            options='rw', **{'maj:min': '8:0'})]
        self.source_device = '8:0'

    def command(self, *args):
        if args[0] == 'lsblk':
            return json.dumps(dict(blockdevices=[self.root, self.target]))
        if args[0] == 'findmnt':
            if '--target' in args:
                return json.dumps(dict(filesystems=[{'maj:min': self.source_device}]))
            return json.dumps(dict(filesystems=self.mounts))
        if args[0] == 'swapon':
            return ''
        raise AssertionError(args)

    def inspect(self, profile='usb', expected=('serial', 'Reader', 62534975488)):
        original = Path.stat

        def fake_stat(path, *args, **kwargs):
            if path == self.device:
                return types.SimpleNamespace(st_mode=stat.S_IFBLK | 0o600)
            return original(path, *args, **kwargs)

        with patch.object(flash, 'command', self.command), patch.object(Path, 'stat', fake_stat):
            return flash.inspect(self.device, self.image, profile, expected)

    def test_r5s_sysfs_sd_identity_and_emmc_refusal(self):
        sysdev = Path('/sys/class/block') / self.device.name / 'device'
        values = {sysdev / 'type': 'SD', sysdev / 'cid': 'card-cid', sysdev / 'name': 'SDCARD'}
        exists, read_text = Path.exists, Path.read_text

        def fake_exists(path):
            return path in values or exists(path)

        def fake_read(path, *args, **kwargs):
            return values[path] if path in values else read_text(path, *args, **kwargs)

        with patch.object(Path, 'exists', fake_exists), patch.object(Path, 'read_text', fake_read):
            result = self.inspect('r5s', ('card-cid', 'SDCARD', 62534975488))
            self.assertEqual((result['card_type'], result['serial']), ('SD', 'card-cid'))
            values[sysdev / 'type'] = 'MMC'
            with self.assertRaisesRegex(ValueError, 'eMMC'):
                self.inspect('r5s', ('card-cid', 'SDCARD', 62534975488))

    def test_accept_explicit_usb_identity(self):
        self.assertEqual(self.inspect()['serial'], 'serial')

    def test_source_on_target(self):
        self.source_device = '8:16'
        with self.assertRaisesRegex(ValueError, 'Source image'):
            self.inspect()

    def test_mounted_descendant(self):
        self.target['children'] = [dict(name=str(self.device) + '1', type='part', **{'maj:min': '8:17'})]
        self.mounts.append(dict(target='/mnt/storage', fstype='ext4', **{'maj:min': '8:17'}))
        with self.assertRaisesRegex(ValueError, 'mounted'):
            self.inspect()

    def test_root_on_target(self):
        self.mounts[0]['maj:min'] = '8:16'
        with self.assertRaisesRegex(ValueError, 'mounted'):
            self.inspect()

    def test_loop_root_refused(self):
        self.root['type'] = 'loop'
        with self.assertRaisesRegex(ValueError, 'dependency chain'):
            self.inspect()

    def test_source_on_target_partition(self):
        self.target['children'] = [dict(name=str(self.device) + '1', type='part', **{'maj:min': '8:17'})]
        self.source_device = '8:17'
        with self.assertRaisesRegex(ValueError, 'Source image'):
            self.inspect()

    def test_overlay_refused(self):
        self.mounts[0].update(fstype='overlay', options='lowerdir=/lower,upperdir=/upper')
        with self.assertRaisesRegex(ValueError, 'Overlay'):
            self.inspect()

    def test_unknown_root_refused(self):
        self.mounts[0]['maj:min'] = '0:42'
        with self.assertRaisesRegex(ValueError, 'Root backing'):
            self.inspect()

    def test_emmc_and_bad_identity(self):
        valid = dict(name='/dev/mmcblk7', kind='disk', size=1024,
                     model='SD', serial='abc', card_type='SD', parent=False,
                     transport=None, removable=True)
        flash.validate(valid, 'r5s', ('abc', 'SD', 1024), 512)
        for updates in (dict(card_type='MMC'), dict(kind='part'), dict(serial=''),
                        dict(name='/dev/mmcblk7boot0'), dict(size=128), dict(parent=True)):
            with self.assertRaises(ValueError):
                flash.validate(dict(valid, **updates), 'r5s', ('abc', 'SD', 1024), 512)

    def test_regular_file_refused(self):
        with self.assertRaisesRegex(ValueError, 'whole block'):
            flash.inspect(self.device, self.image, 'usb', ('serial', 'Reader', 62534975488))

    def test_dry_run_never_opens_target(self):
        args = ['flash-sd', '--device', str(self.device), '--image', str(self.image),
                '--profile', 'usb', '--expect-serial', 'serial', '--expect-model', 'Reader',
                '--expect-bytes', '62534975488']
        with patch.object(sys, 'argv', args), patch.object(flash.subprocess, 'run'), \
                patch.object(flash, 'inspect', return_value={'name': str(self.device)}), \
                patch.object(flash.os, 'open') as opened, contextlib.redirect_stdout(io.StringIO()):
            flash.main()
            opened.assert_not_called()

    def test_confirmed_write_to_mocked_regular_target(self):
        args = ['flash-sd', '--device', str(self.device), '--image', str(self.image),
                '--profile', 'usb', '--expect-serial', 'serial', '--expect-model', 'Reader',
                '--expect-bytes', '62534975488', '--execute']
        metadata = dict(name=str(self.device), serial='serial', size=62534975488,
                        major_minor='8:16')
        original_open, original_stat = flash.os.open, flash.os.fstat
        descriptor = original_open(self.device, flash.os.O_RDWR)

        def fake_stat(fd):
            if fd == descriptor:
                return types.SimpleNamespace(st_mode=stat.S_IFBLK, st_rdev=flash.os.makedev(8, 16))
            return original_stat(fd)

        confirmation = f'OVERWRITE {self.device} serial 62534975488'
        with patch.object(sys, 'argv', args), patch.object(flash.subprocess, 'run'), \
                patch.object(flash, 'inspect', return_value=metadata), \
                patch('builtins.input', return_value=confirmation), \
                patch.object(flash.os, 'open', return_value=descriptor), \
                patch.object(flash.os, 'fstat', side_effect=fake_stat), \
                patch.object(flash.fcntl, 'ioctl') as flush, contextlib.redirect_stdout(io.StringIO()):
            flash.main()
            flush.assert_called_once_with(descriptor, 0x1261)
        self.assertEqual(self.device.read_bytes(), self.image.read_bytes())


if __name__ == '__main__':
    unittest.main()
