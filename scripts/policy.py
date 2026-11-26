#!/usr/bin/env python3
"""Host checks shared by image tooling and tests."""
import hashlib
import struct

RAM = [(0x200000, 0x8400000), (0x9400000, 0xf0000000),
       (0x1f0000000, 0x200000000)]
EXCLUDED = [(0, 0x200000), (0x8400000, 0x9400000),
            (0x10000000, 0x10800000), (0xf0000000, 0x1f0000000)]
PROTOTYPE = (0x20000000, 0x40000000)


def sha256(path):
    with open(path, 'rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def intersect(a, b):
    return a[0] < b[1] and b[0] < a[1]


def decode_ram(cells):
    if len(cells) % 4 or any(not 0 <= n <= 0xffffffff for n in cells):
        raise ValueError('Expected pairs of 64-bit address and size cells')
    ranges = []
    for i in range(0, len(cells), 4):
        start = cells[i] << 32 | cells[i + 1]
        size = cells[i + 2] << 32 | cells[i + 3]
        if size:
            if start + size > 1 << 64:
                raise ValueError('RAM overflow')
            ranges.append((start, start + size))
    return ranges


def check_ram(region):
    if not PROTOTYPE[0] <= region[0] < region[1] <= PROTOTYPE[1]:
        raise ValueError(f'Outside prototype RAM: {region}')
    if any(intersect(region, r) for r in EXCLUDED):
        raise ValueError('Reserved RAM intersection')


def check_layout(regions):
    for index, (name, start, end) in enumerate(regions):
        if not 0 <= start < end:
            raise ValueError(f'Invalid region: {name}')
        for other, lo, hi in regions[:index]:
            if intersect((start, end), (lo, hi)):
                raise ValueError(f'Overlap: {name}, {other}')


def elf_segments(path):
    data = path.read_bytes()
    if data[:6] != b'\x7fELF\x02\x01' or len(data) < 64:
        raise ValueError(f'Expected little-endian ELF64: {path}')
    if struct.unpack_from('<H', data, 18)[0] != 183:
        raise ValueError('Expected AArch64 ELF')
    entry, offset = struct.unpack_from('<QQ', data, 24)
    size, count = struct.unpack_from('<HH', data, 54)
    if size != 56 or offset + count * size > len(data):
        raise ValueError('Invalid program headers')
    segments = []
    for i in range(count):
        kind, flags, off, va, pa, filesz, memsz, align = struct.unpack_from(
            '<IIQQQQQQ', data, offset + i * size)
        if kind == 1 and memsz:
            if filesz > memsz or off + filesz > len(data):
                raise ValueError('Truncated ELF segment')
            segments.append(dict(virtual=va, physical=pa, size=memsz,
                                 file_size=filesz, flags=flags))
    if not segments:
        raise ValueError('ELF has no loadable segments')
    return entry, segments
