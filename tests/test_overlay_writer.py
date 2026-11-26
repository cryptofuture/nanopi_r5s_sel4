import errno
import json
import os
from pathlib import Path
import stat
import types
import unittest
from unittest.mock import patch

import test_tools as fixtures

flash = fixtures.flash


class OverlayWriterTests(unittest.TestCase):
    def setUp(self):
        self.base = fixtures.WriterTests()
        self.base.setUp()
        self.addCleanup(self.base.doCleanups)
        self.cmdline = 'root=/dev/mmcblk2p8 data=/dev/mmcblk2p9 storagemedia=emmc'
        self.parts = [dict(name=f'/dev/mmcblk2p{n}', type='part', size=1024**3,
                           fstype='ext4', uuid=f'00000000-0000-0000-0000-{n:012x}',
                           **{'maj:min': f'179:{64+n}'}) for n in (8, 9)]
        self.base.root = dict(name='/dev/mmcblk2', type='disk', size=15634268160,
                              children=self.parts, **{'maj:min': '179:64'})
        self.base.mounts[0].update(source='overlay', fstype='overlay',
                                   options='rw,relatime,lowerdir=/root,upperdir=/data/root,workdir=/data/work',
                                   **{'maj:min': '0:42'})
        self.base.source_device = '0:42'
        self.probes = {p['name']: dict(TYPE='ext4', UUID=p['uuid']) for p in self.parts}
        self.reads = {
            '/proc/cmdline': self.cmdline,
            '/sys/firmware/devicetree/base/model': 'FriendlyElec NanoPi R5S\0',
            '/sys/firmware/devicetree/base/compatible': 'friendlyelec,nanopi-r5s\0rockchip,rk3568\0',
            '/sys/class/block/mmcblk2/device/type': 'MMC',
            '/sys/class/block/mmcblk2/device/cid': 'a' * 32,
            f'/sys/class/block/{self.base.device.name}/device/type': 'SD',
            f'/sys/class/block/{self.base.device.name}/device/cid': 'b' * 32,
            f'/sys/class/block/{self.base.device.name}/device/name': 'NCard',
        }
        for n in (8, 9):
            self.reads[f'/sys/class/block/mmcblk2p{n}/dev'] = f'179:{64+n}'
            self.reads[f'/sys/class/block/mmcblk2p{n}/partition'] = str(n)
        self.busy = False
        self.expected_cid = 'b' * 32
        self.missing = set()
        self.swaps = []
        self.inactive = set()
        self.command_calls = []
        self.open_calls = []

    def inspect(self):
        original_stat, original_resolve = Path.stat, Path.resolve
        original_exists, original_read, original_is_dir = Path.exists, Path.read_text, Path.is_dir
        original_read_bytes = Path.read_bytes

        def fake_stat(path, *args, **kwargs):
            if path == self.base.device:
                return types.SimpleNamespace(st_mode=stat.S_IFBLK, st_rdev=os.makedev(8, 16))
            if str(path) in self.missing:
                raise FileNotFoundError(str(path))
            for p in self.parts:
                if str(path) == p['name']:
                    return types.SimpleNamespace(st_mode=stat.S_IFBLK, st_rdev=os.makedev(179, int(p['maj:min'].split(':')[1])))
            return original_stat(path, *args, **kwargs)

        def resolve(path, *args, **kwargs):
            if str(path) in self.missing:
                raise FileNotFoundError(str(path))
            if str(path).startswith('/dev/mmcblk2p'):
                return path
            if str(path).startswith('/sys/class/block/mmcblk2p'):
                return Path('/sys/class/block/mmcblk2') / path.name
            if str(path) == '/sys/class/block/mmcblk2':
                return path
            return original_resolve(path, *args, **kwargs)

        def read(path, *args, **kwargs):
            if str(path) == '/proc/cmdline':
                return self.cmdline
            if str(path) in self.reads:
                return self.reads[str(path)]
            return original_read(path, *args, **kwargs)

        def command(*args):
            self.command_calls.append(args)
            if args[0] == 'blkid':
                return '\n'.join(f'{k}={v}' for k, v in self.probes[args[-1]].items())
            if args[0] == 'swapon':
                return '\n'.join(self.swaps)
            return self.base.command(*args)

        def open_probe(name, flags):
            self.open_calls.append((name, flags))
            self.assertEqual(name, str(self.base.device))
            self.assertEqual(flags, os.O_RDONLY | os.O_EXCL | os.O_NOFOLLOW)
            if self.busy:
                raise OSError(errno.EBUSY, 'Hidden filesystem holds target')
            return 9876

        with patch.object(flash, 'command', command), \
                patch.object(Path, 'stat', fake_stat), \
                patch.object(Path, 'resolve', resolve), \
                patch.object(Path, 'read_text', read), \
                patch.object(Path, 'read_bytes', lambda p: self.reads[str(p)].encode() if str(p) in self.reads else original_read_bytes(p)), \
                patch.object(Path, 'exists', lambda p: str(p) in self.reads or original_exists(p)), \
                patch.object(Path, 'is_dir', lambda p: str(p) not in self.inactive if str(p).startswith('/sys/fs/ext4/mmcblk2p') else original_is_dir(p)), \
                patch.object(flash.os, 'readlink', return_value='mnt:[123]'), \
                patch.object(flash.os, 'open', side_effect=open_probe), \
                patch.object(flash.os, 'fstat', return_value=types.SimpleNamespace(st_mode=stat.S_IFBLK, st_rdev=os.makedev(8, 16))), \
                patch.object(flash.os, 'close'):
            return flash.inspect(self.base.device, self.base.image, 'r5s', (self.expected_cid, 'NCard', 62534975488))

    def test_valid_hidden_emmc_overlay_and_read_only_claim(self):
        metadata = self.inspect()
        self.assertEqual([p['role'] for p in metadata['overlay_backing']], ['root', 'data'])
        self.assertEqual(len(self.open_calls), 1)
        self.assertTrue(all(args[1:4] == ('-p', '-o', 'export') for args in self.command_calls if args[0] == 'blkid'))

    def test_unsupported_board_identity_refused(self):
        model = '/sys/firmware/devicetree/base/model'
        compatible = '/sys/firmware/devicetree/base/compatible'
        for path, value in ((model, 'NanoPi R5S\0'),
                            (model, 'FriendlyElec NanoPi R5S\n\0'),
                            (compatible, 'friendlyelec,nanopi-r5s\0'),
                            (compatible, 'rockchip,rk3568\0'),
                            (compatible, 'friendlyelec,nanopi-r5s-extra\0rockchip,rk3568\0')):
            original = self.reads[path]
            with self.subTest(value=value):
                self.reads[path] = value
                with self.assertRaisesRegex(ValueError, 'Unsupported board identity'):
                    self.inspect()
                self.reads[path] = original
        self.assertFalse(self.open_calls)

    def test_sd_backed_boot_arguments_refused(self):
        for key in ('root', 'data'):
            with self.subTest(key=key):
                self.cmdline = self.reads['/proc/cmdline'].replace(f'{key}=/dev/mmcblk2', f'{key}=/dev/mmcblk0')
                with self.assertRaises((ValueError, FileNotFoundError)):
                    self.inspect()
        self.assertFalse(self.open_calls)

    def test_stale_emmc_arguments_do_not_allow_hidden_sd_overlay(self):
        self.busy = True
        with self.assertRaises(OSError) as error:
            self.inspect()
        self.assertEqual(error.exception.errno, errno.EBUSY)

    def test_missing_backing_device(self):
        self.missing.add('/dev/mmcblk2p9')
        with self.assertRaises(FileNotFoundError):
            self.inspect()
        self.assertFalse(self.open_calls)

    def test_ambiguous_or_incomplete_boot_arguments(self):
        baseline = self.cmdline
        for commandline in (baseline + ' root=/dev/mmcblk2p8', baseline + ' data=/dev/mmcblk0p9',
                            baseline.replace(' data=/dev/mmcblk2p9', ''),
                            baseline + ' androidboot.storagemedia=sd', baseline.replace('emmc', 'sd')):
            with self.subTest(commandline=commandline):
                self.cmdline = commandline
                with self.assertRaises(ValueError):
                    self.inspect()
        self.assertFalse(self.open_calls)

    def test_inconsistent_filesystem_identities(self):
        for changes in (dict(TYPE='vfat'), dict(UUID='11111111-1111-1111-1111-111111111111'), dict(UUID='')):
            original = dict(self.probes['/dev/mmcblk2p9'])
            self.probes['/dev/mmcblk2p9'].update(changes)
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                self.inspect()
            self.probes['/dev/mmcblk2p9'] = original
        self.assertFalse(self.open_calls)

    def test_missing_lsblk_backing_and_wrong_emmc_type(self):
        self.base.root['children'] = self.parts[:1]
        with self.assertRaisesRegex(ValueError, 'backing device'):
            self.inspect()
        self.base.root['children'] = self.parts
        self.reads['/sys/class/block/mmcblk2/device/type'] = 'SD'
        with self.assertRaisesRegex(ValueError, 'eMMC block identity'):
            self.inspect()

    def test_unknown_or_sd_image_dependency(self):
        for major_minor in ('0:999', '8:16'):
            self.base.source_device = major_minor
            with self.subTest(major_minor=major_minor), self.assertRaises(ValueError):
                self.inspect()

    def test_additional_overlay_or_sd_mount_refused(self):
        for extra in (dict(target='/other', fstype='overlay', **{'maj:min': '0:99'}),
                      dict(target='/card', fstype='vfat', **{'maj:min': '8:16'})):
            self.base.mounts.append(extra)
            with self.subTest(extra=extra), self.assertRaises(ValueError):
                self.inspect()
            self.base.mounts.pop()

    def test_raw_block_loop_dependency_refused(self):
        self.base.root['children'].append(dict(name='/dev/loop0', type='loop', size=1024**3,
                                               **{'maj:min': '7:0'}))
        with self.assertRaisesRegex(ValueError, 'Unsupported active block topology'):
            self.inspect()
        self.assertFalse(self.open_calls)

    def test_sd_swap_refused_before_exclusive_probe(self):
        self.swaps = [str(self.base.device)]
        with self.assertRaisesRegex(ValueError, 'swap'):
            self.inspect()
        self.assertFalse(self.open_calls)

    def test_inactive_ext4_and_inconsistent_major_minor_refused(self):
        self.inactive.add('/sys/fs/ext4/mmcblk2p9')
        with self.assertRaisesRegex(ValueError, 'inactive ext4'):
            self.inspect()
        self.inactive.clear()
        self.reads['/sys/class/block/mmcblk2p9/dev'] = '179:999'
        with self.assertRaisesRegex(ValueError, 'block identity'):
            self.inspect()

    def test_exclusive_probe_checks_descriptor_identity_and_closes_it(self):
        metadata = dict(name='/dev/mmcblk0', major_minor='179:0')
        with patch.object(flash.os, 'open', return_value=9876), \
                patch.object(flash.os, 'fstat', return_value=types.SimpleNamespace(st_mode=stat.S_IFBLK, st_rdev=os.makedev(179, 64))), \
                patch.object(flash.os, 'close') as close:
            with self.assertRaisesRegex(ValueError, 'different block device'):
                flash.require_unused_target(metadata)
            close.assert_called_once_with(9876)


if __name__ == '__main__':
    unittest.main()
