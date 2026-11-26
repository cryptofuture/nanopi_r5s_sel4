#!/usr/bin/env python3
import json
from pathlib import Path
import struct
import subprocess
import tempfile
import uuid
import zlib

from policy import check_layout, sha256
from diag_profile import PROFILE, validate_manifest
from trace_profile import PROFILES, validate_trace_manifest

SECTOR = 512
START = 16 * 1024 * 1024
FAT_SIZE = 64 * 1024 * 1024
DISK_SIZE = 96 * 1024 * 1024
BOOT = 64 * SECTOR


def run(*args):
    subprocess.run([str(a) for a in args], check=True)


def normalize_fat_times(path):
    with path.open('r+b') as stream:
        bpb = stream.read(512)
        sector = struct.unpack_from('<H', bpb, 11)[0]
        reserved = struct.unpack_from('<H', bpb, 14)[0]
        fat_count = bpb[16]
        entries = struct.unpack_from('<H', bpb, 17)[0]
        fat_sectors = struct.unpack_from('<H', bpb, 22)[0]
        if sector != 512 or not entries or not fat_sectors:
            raise ValueError('Expected fixed FAT16 root directory')
        root = (reserved + fat_count * fat_sectors) * sector
        for i in range(entries):
            stream.seek(root + i * 32)
            entry = bytearray(stream.read(32))
            if not entry[0]:
                break
            if entry[0] == 0xe5 or entry[11] == 0x0f:
                continue
            entry[13] = 0
            struct.pack_into('<HHH', entry, 14, 0, 33, 33)
            struct.pack_into('<HH', entry, 22, 0, 33)
            stream.seek(root + i * 32)
            stream.write(entry)


def gpt(path):
    with path.open('rb') as f:
        mbr = f.read(512)
        if mbr[510:] != b'\x55\xaa' or mbr[450] != 0xee:
            raise ValueError('Missing protective MBR')
        headers = []
        for lba in (1, DISK_SIZE // SECTOR - 1):
            f.seek(lba * SECTOR)
            header = bytearray(f.read(92))
            if header[:8] != b'EFI PART':
                raise ValueError('Missing GPT')
            size, checksum = struct.unpack_from('<II', header, 12)
            struct.pack_into('<I', header, 16, 0)
            if size != 92 or zlib.crc32(header) != checksum:
                raise ValueError('GPT header checksum mismatch')
            current, backup, first, last = struct.unpack_from('<QQQQ', header, 24)
            entries, count, entry_size, crc = struct.unpack_from('<QIII', header, 72)
            if (current != lba or backup != (DISK_SIZE // SECTOR - 1 if lba == 1 else 1)
                    or first != 34 or last != DISK_SIZE // SECTOR - 34
                    or count != 128 or entry_size != 128
                    or entries != (2 if lba == 1 else DISK_SIZE // SECTOR - 33)):
                raise ValueError('Unexpected GPT layout')
            f.seek(entries * SECTOR)
            data = f.read(count * entry_size)
            if zlib.crc32(data) != crc:
                raise ValueError('GPT entries checksum mismatch')
            headers.append((header[56:72], data))
        if headers[0] != headers[1]:
            raise ValueError('GPT backup differs')
        data = headers[0][1]
        first, last = struct.unpack_from('<QQ', data, 32)
        if (first != START // SECTOR or last != (START + FAT_SIZE) // SECTOR - 1
                or any(data[128:])):
            raise ValueError('Unexpected partitions')
        if data[:16] != uuid.UUID('EBD0A0A2-B9E5-4433-87C0-68B6B72699C7').bytes_le:
            raise ValueError('Unexpected partition type')


def verify(path):
    if not path.is_file() or path.stat().st_size != DISK_SIZE:
        raise ValueError('Expected regular 96 MiB image')
    gpt(path)
    with tempfile.TemporaryDirectory() as work:
        work = Path(work)
        fat = work / 'boot.fat'
        with path.open('rb') as source, fat.open('wb') as output:
            source.seek(START)
            output.write(source.read(FAT_SIZE))
        run('fsck.fat', '-n', fat)
        for name in ('manifest.json', 'boot.txt'):
            run('mcopy', '-i', fat, f'::{name}', work / name)
        manifest = json.loads((work / 'manifest.json').read_text())
        profile = manifest.get('profile', 'microkit')
        if profile == PROFILE:
            validate_manifest(manifest)
            names = subprocess.check_output(['mdir', '-a', '-b', '-i', str(fat), '::'], text=True)
            if {name.rsplit('/', 1)[-1].lower() for name in names.splitlines()} != {'manifest.json', 'boot.txt'}:
                raise ValueError('Diagnostic FAT must contain only manifest.json and boot.txt')
        elif profile == 'microkit':
            verify_microkit_payload(manifest, work, fat)
        elif profile in PROFILES:
            validate_trace_manifest(manifest)
            verify_microkit_payload(dict(manifest, format=1), work, fat)
        elif profile == 'runtime-baseline-v2':
            from native_profile import validate_native_manifest
            validate_native_manifest(manifest)
            verify_microkit_payload(dict(manifest, format=1), work, fat)
        elif profile == 'gmac-link':
            from gmac_profile import validate_gmac_manifest
            validate_gmac_manifest(manifest)
            verify_microkit_payload(dict(manifest, format=1), work, fat)
        elif profile == 'iodomain-fixed':
            from iodomain_fixed_profile import validate_fixed_manifest
            validate_fixed_manifest(manifest)
            verify_microkit_payload(dict(manifest, format=1), work, fat)
        elif profile == 'iodomain-observe':
            from iodomain_profile import validate_fixed_manifest
            validate_fixed_manifest(manifest)
            verify_microkit_payload(dict(manifest, format=1), work, fat)
        elif profile == 'gmac-dma-preflight-fault-id':
            from fault_id_profile import validate_fixed_manifest
            validate_fixed_manifest(manifest)
            verify_microkit_payload(dict(manifest, format=1), work, fat)
        elif profile == 'gmac-dma-preflight-repair-v2':
            from preflight_repair_profile import validate_fixed_manifest
            validate_fixed_manifest(manifest)
            verify_microkit_payload(dict(manifest, format=1), work, fat)
        elif profile == 'gmac-dma-preflight-fixed':
            from preflight_profile import validate_fixed_manifest
            validate_fixed_manifest(manifest)
            verify_microkit_payload(dict(manifest, format=1), work, fat)
        elif profile in ('lan-dhcp-dma-repair', 'lan-dhcp-diagnostics-v2', 'rtl8125-lan-repair'):
            from dual_lan_profile import WORKER_SCHEDULES, b51_memory, b51_rx_fits, zc_pool_manifest
            options = manifest.get('optimization_options', {})
            if not isinstance(options, dict):
                raise ValueError('Invalid optimization options')
            if any(options.get(name) == '1' for name in ('R5S_LAN_FAST_PATH', 'R5S_WORKER_WAN_EGRESS',
                                                        'R5S_WORKER_LAN_EGRESS')) and (
                    manifest.get('router_schedule') not in WORKER_SCHEDULES or manifest.get('zc_enabled') is not True):
                raise ValueError('Invalid direct worker layout')
            if options.get('R5S_DRIVER_GRO') == '1' and (
                    manifest.get('zc_enabled') is not True
                    or not any(options.get(name) == '1' for name in ('R5S_WAN_FAST_PATH', 'R5S_LAN_FAST_PATH'))):
                raise ValueError('Invalid driver GRO layout')
            if options.get('R5S_WORKER_GRO') == '1' and manifest.get('zc_enabled') is not True:
                raise ValueError('Invalid worker GRO layout')
            if options.get('R5S_GMAC_HYBRID_IRQ') == '1' and manifest.get('router_schedule') not in WORKER_SCHEDULES:
                raise ValueError('Invalid GMAC IRQ layout')
            if options.get('R5S_GMAC_RX_CHANNELS', '1') not in ('1', '2') or (
                    options.get('R5S_GMAC_RX_CHANNELS') == '2' and manifest.get('zc_enabled') is not True):
                raise ValueError('Invalid GMAC RX channel layout')
            if options.get('R5S_GMAC_TX_CHANNELS', '1') not in ('1', '2') or (
                    options.get('R5S_GMAC_TX_CHANNELS') == '2' and
                    (manifest.get('zc_enabled') is not True or options.get('R5S_WORKER_WAN_EGRESS') != '1')):
                raise ValueError('Invalid GMAC TX channel layout')
            ring = options.get('R5S_B51_RING', '64')
            queues = options.get('R5S_B51_RX_QUEUES', '1')
            tx_queues = options.get('R5S_B51_TX_QUEUES', '1')
            if tx_queues not in ('1', '2') or (tx_queues == '2' and
                    (manifest.get('zc_enabled') is not True or options.get('R5S_WORKER_LAN_EGRESS') != '1')):
                raise ValueError('Invalid B51 TX queue layout')
            cache = options.get('R5S_ZC_CACHE', 'nc')
            if cache not in ('nc', 'writeback') or (cache == 'writeback' and manifest.get('zc_enabled') is not True):
                raise ValueError('Invalid shared payload cache mode')
            if not b51_rx_fits(ring, queues, int(tx_queues)) or ((ring in ('256', '512') or queues != '1')
                    and manifest.get('router_schedule') not in WORKER_SCHEDULES):
                raise ValueError('Invalid B51 ring layout')
            lan1, lan2, _ = b51_memory(ring, options.get("R5S_ZC_CACHE", "nc"))
            if profile != 'lan-dhcp-dma-repair' and manifest.get('fault_diagnostics') != 2:
                raise ValueError('Invalid fault diagnostics version')
            mode = manifest.get('lan_startup')
            ports = {'wan': [], 'lan1': ['lan1'], 'lan2': ['lan2'], 'both': ['lan1', 'lan2']}.get(mode)
            lan_services = [p+'-dhcp' for p in ports or ()]
            if 'lan1_tx_mode' in manifest and (manifest['lan1_tx_mode'] != 'no-close-v3'
                                                or mode != 'lan1' or profile != 'rtl8125-lan-repair'):
                raise ValueError('Invalid LAN1 TX mode')
            if 'lan1_tx_descriptor_bytes' in manifest and (
                    manifest.get('lan1_tx_mode') != 'no-close-v3' or manifest['lan1_tx_descriptor_bytes'] != 32):
                raise ValueError('Invalid LAN1 TX descriptor size')
            if 'b51_experiment' in manifest or 'b51_status_version' in manifest:
                experiment = manifest.get('b51_experiment')
                if (manifest.get('b51_status_version') not in (1, 2) or manifest.get('lan1_tx_diagnostic') is not True
                        or experiment not in ('observe', 'tx-check', 'tally-rx-first')
                        or (experiment != 'observe' and manifest.get('lan1_tx_mode') != 'no-close-v3')):
                    raise ValueError('Invalid B51 diagnostic experiment')
            if 'lan1_tx_diagnostic' in manifest:
                if (manifest['lan1_tx_diagnostic'] is not True or mode != 'lan1'
                        or profile != 'rtl8125-lan-repair'):
                    raise ValueError('Invalid LAN1 TX diagnostic profile')
                lan_services = ['lan1-tx-diagnostic']
            stage = manifest.get('lan_checkpoint')
            if 'lan_b51_udp_port' in manifest and (stage != 'dual-lan-router' or manifest['lan_b51_udp_port'] != 48125):
                raise ValueError('Invalid LAN status diagnostic')
            if stage is not None:
                from dual_lan_profile import ROUTER_SCHEDULE, STAGES, services, valid_dns, valid_fallback
                dns = manifest.get('lan_dns')
                if (not isinstance(stage, str) or STAGES.get(stage) != mode or profile != 'rtl8125-lan-repair'
                        or manifest.get('lan_topology') != 'shared-lan1'
                        or manifest.get('wan_b51_udp_port') not in (48125, 0)
                        or (manifest.get('lan_status_version'), 'router_schedule' in manifest) not in ((3, False), (4, True), (5, True), (6, True))
                        or ('router_schedule' in manifest and (stage != 'dual-lan-router'
                            or manifest['router_schedule'] not in ROUTER_SCHEDULE
                            or (manifest['lan_status_version'] in (5, 6)) != (manifest['router_schedule'] in WORKER_SCHEDULES)
                            or (manifest['lan_status_version'] == 6) != bool(manifest.get('zc_enabled'))))
                        or not valid_fallback(manifest.get('wan_diag_fallback'))
                        or not valid_dns(dns)):
                    raise ValueError('Invalid LAN checkpoint')
                if manifest.get('zc_enabled'):
                    pool = manifest.get('zc_pool') or {}
                    expected_pool = zc_pool_manifest(options)
                    if (not isinstance(pool, dict)
                            or any(pool.get(key) != value for key, value in expected_pool.items())
                            or manifest.get('zc_buffers') != expected_pool['size'] // 2048
                            or manifest.get('zc_buffer_bytes') != 2048):
                        raise ValueError('Invalid zero-copy pool mapping')
                if 'tx_ipv4_checksum_requested' in manifest:
                    offloads = manifest['tx_ipv4_checksum_requested']
                    if (stage != 'dual-lan-router' or not isinstance(offloads, list) or not offloads
                            or offloads != [p for p in ('wan','lan1','lan2') if p in offloads]):
                        raise ValueError('Invalid IPv4 transmit checksum request')
                if 'tso_requested' in manifest:
                    from dual_lan_profile import valid_tso_request
                    if not valid_tso_request(manifest['tso_requested'], manifest.get('tso_ipv6'), stage):
                        raise ValueError('Invalid TSO request')
                    if 'tso_test' in manifest and manifest['tso_test'] is not True:
                        raise ValueError('Invalid TSO test source')
                if options.get('R5S_TSO_TEST_SOFTWARE') == '1' and (
                        stage != 'dual-lan-router' or manifest.get('tso_test') is not True
                        or not manifest.get('tso_requested')):
                    raise ValueError('Invalid software TSO probe mode')
                lan_services = services(stage)
                if manifest.get('wan_b51_udp_port') == 0:
                    # Production flavor: the LAN status endpoint is disabled.
                    lan_services = [name for name in lan_services if name != 'wan-b51-status']
            elif 'wan_b51_udp_port' in manifest:
                if (manifest['wan_b51_udp_port'] != 48125 or manifest.get('b51_experiment') != 'observe'
                        or manifest.get('lan1_tx_mode') != 'no-close-v3' or manifest.get('b51_status_version') not in (1, 2)):
                    raise ValueError('Invalid WAN status diagnostic')
                lan_services.append('wan-b51-status')
            if (ports is None or manifest.get('hardware_tested') is not False or manifest.get('format') != 7
                    or 'negative_test' in manifest or manifest.get('lan2_enabled') is not ('lan2' in ports)
                    or manifest.get('services') != ['wan-dhcp-client','wan-gateway-probes'] + lan_services):
                raise ValueError('Invalid staged LAN repair profile')
            for port, pool in [('lan1', lan1), ('lan2', lan2)]:
                expected = pool if port in ports else None
                if manifest.get(port+'_pool') != expected:
                    raise ValueError('Invalid staged LAN pool')
            verify_microkit_payload(dict(manifest, format=1), work, fat)
        elif profile in ('lan-dhcp', 'lan-dhcp-dma'):
            if (manifest.get('hardware_tested') is not False or manifest.get('lan2_enabled') is not True
                    or manifest.get('format') != 7 or 'negative_test' in manifest
                    or manifest.get('services') != ['wan-dhcp-client', 'wan-gateway-probes', 'lan1-dhcp', 'lan2-dhcp']
                    or manifest.get('lan1_pool') != [0x62000000, 0x24200000, 0x80000000, 0x100000]
                    or manifest.get('lan2_pool') != [0x62000000, 0x24300000, 0x81000000, 0x100000]):
                raise ValueError('Invalid dual LAN DHCP profile')
            verify_microkit_payload(dict(manifest, format=1), work, fat)
        elif profile == 'lan1-bringup':
            if (manifest.get('hardware_tested') is not False or manifest.get('lan2_enabled') is not False
                    or manifest.get('format') != 7 or 'negative_test' in manifest
                    or manifest.get('services') != ['wan-dhcp-client', 'wan-gateway-probes', 'lan1-arp']
                    or manifest.get('lan1_pool') != [0x62000000, 0x24200000, 0x80000000, 0x100000]):
                raise ValueError('Invalid LAN1 bring-up profile')
            verify_microkit_payload(dict(manifest, format=1), work, fat)
        elif profile == 'pcie-diagnostic':
            from pcie_diagnostic_profile import validate_pcie_diagnostic
            validate_pcie_diagnostic(manifest)
            verify_microkit_payload(dict(manifest, format=1), work, fat)
        elif profile == 'router-lan-gated':
            from lan_gated_profile import validate_manifest as validate_gated
            validate_gated(manifest)
            verify_microkit_payload(dict(manifest, format=1), work, fat)
        elif profile == 'wan-dhcp':
            from wan_profile import validate_wan_manifest
            validate_wan_manifest(manifest)
            verify_microkit_payload(dict(manifest, format=1), work, fat)
        elif profile in ('gmac-dma-preflight', 'gmac-rawio'):
            from dma_profile import validate_dma_manifest
            validate_dma_manifest(manifest)
            verify_microkit_payload(dict(manifest, format=1), work, fat)
        else:
            raise ValueError('Unknown image profile')
        if sha256(work / 'boot.txt') != manifest['policy_sha256']:
            raise ValueError('Boot policy mismatch')
        size = manifest['uboot']['size']
        check_layout([('GPT', 0, 34 * SECTOR), ('bootloader', BOOT, BOOT + size),
                      ('FAT', START, START + FAT_SIZE),
                      ('backup GPT', DISK_SIZE - 33 * SECTOR, DISK_SIZE)])
        if size < 1024:
            raise ValueError('Missing bootloader')
        with path.open('rb') as source:
            source.seek(BOOT)
            boot = work / 'bootloader'
            boot.write_bytes(source.read(size))
            source.seek(0x800000)
            if source.read(4) != b'\xd0\x0d\xfe\xed':
                raise ValueError('FIT is absent from the verified binman location')
        if sha256(boot) != manifest['uboot']['sha256']:
            raise ValueError('Bootloader hash mismatch')
        return manifest


def verify_microkit_payload(manifest, work, fat):
    negative = manifest.get('negative_test')
    if negative not in (None, 'missing', 'corrupt'):
        raise ValueError('Unknown negative test')
    if manifest['format'] != 1 or not 0 < manifest['loader']['size'] <= 0x4000000:
        raise ValueError('Invalid manifest')
    if manifest['load_address'] != 0x30000000 or manifest['ram'] != [0x20000000, 0x40000000]:
        raise ValueError('Memory policy mismatch')
    if negative == 'missing':
        result = subprocess.run(['mtype', '-i', str(fat), '::loader.img'],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if result.returncode == 0:
            raise ValueError('Missing-payload test unexpectedly contains payload')
    else:
        run('mcopy', '-i', fat, '::loader.img', work / 'loader.img')
        digest = manifest.get('negative_sha256') if negative else manifest['loader']['sha256']
        if (sha256(work / 'loader.img') != digest or
                (work / 'loader.img').stat().st_size != manifest['loader']['size']):
            raise ValueError('Payload mismatch')
        if negative and digest == manifest['loader']['sha256']:
            raise ValueError('Negative test payload is unchanged')


def assemble(root, destination):
    out = root / 'out'
    loader, uboot = out / 'loader.img', out / 'u-boot-rockchip.bin'
    for path in (loader, uboot, out / 'layout-approved.json'):
        if not path.is_file() or not path.stat().st_size:
            raise ValueError(f'Missing required build artifact: {path}')
    approval = json.loads((out / 'layout-approved.json').read_text())
    if approval['loader_sha256'] != sha256(loader) or approval['uboot_sha256'] != sha256(uboot):
        raise ValueError('Layout approval is stale')
    check_layout([('GPT', 0, 34 * SECTOR),
                  ('bootloader', BOOT, BOOT + uboot.stat().st_size),
                  ('FAT', START, START + FAT_SIZE),
                  ('backup GPT', DISK_SIZE - 33 * SECTOR, DISK_SIZE)])
    policy = root / 'firmware/boot-policy.txt'
    manifest = dict(format=1, load_address=0x30000000, ram=[0x20000000, 0x40000000],
                    loader=dict(size=loader.stat().st_size, sha256=sha256(loader)),
                    uboot=dict(size=uboot.stat().st_size, sha256=sha256(uboot)),
                    lock_sha256=sha256(root / 'sources.lock'),
                    policy_sha256=sha256(policy), hardware_tested=False)
    seed = manifest['loader']['sha256'] + manifest['uboot']['sha256']
    files = [(loader, 'loader.img'), (policy, 'boot.txt')]
    assemble_files(destination, uboot, files, manifest, seed)


def assemble_files(destination, uboot, files, manifest, seed):
    check_layout([('GPT', 0, 34 * SECTOR),
                  ('bootloader', BOOT, BOOT + uboot.stat().st_size),
                  ('FAT', START, START + FAT_SIZE),
                  ('backup GPT', DISK_SIZE - 33 * SECTOR, DISK_SIZE)])
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists():
        raise ValueError('Destination already exists; choose a new output path')
    with tempfile.TemporaryDirectory(dir=destination.parent) as temp:
        temp = Path(temp)
        disk, fat = temp / 'disk.img', temp / 'boot.fat'
        with disk.open('wb') as f:
            f.truncate(DISK_SIZE)
        run('sgdisk', '--clear', f'--disk-guid={uuid.uuid5(uuid.NAMESPACE_URL, seed)}',
            f'--new=1:{START // SECTOR}:+64M', '--typecode=1:0700',
            '--change-name=1:R5S-MICROKIT',
            f'--partition-guid=1:{uuid.uuid5(uuid.NAMESPACE_OID, seed)}', disk)
        with fat.open('wb') as f:
            f.truncate(FAT_SIZE)
        run('mkfs.fat', '--invariant', '-F', '16', '-i', seed[:8], '-n', 'R5SBOOT', fat)
        (temp / 'manifest.json').write_text(json.dumps(manifest, sort_keys=True, indent=2) + '\n')
        for source, target in files + [(temp / 'manifest.json', 'manifest.json')]:
            run('mcopy', '-i', fat, source, f'::{target}')
        normalize_fat_times(fat)
        with disk.open('r+b') as f:
            f.seek(BOOT)
            f.write(uboot.read_bytes())
            f.seek(START)
            f.write(fat.read_bytes())
        verify(disk)
        disk.rename(destination)
    destination.with_suffix('.manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    destination.with_suffix('.sha256').write_text(f'{sha256(destination)}  {destination.name}\n')
