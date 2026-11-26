import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import tempfile

from elftools.elf.elffile import ELFFile
from diag_profile import LOCK_SHA256, NORMAL_SHA256, PATTERN, PROFILE
from image import assemble_files, verify
from policy import sha256


def output(*args):
    return subprocess.check_output([str(a) for a in args], text=True).strip()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def command_evidence(path):
    with path.open('rb') as stream:
        elf = ELFFile(stream)
        symbols = {}
        for line in output('aarch64-none-elf-nm', '-S', '--defined-only', path).splitlines():
            fields = line.split()
            if len(fields) == 4:
                address, size, kind, name = fields
                symbols[name] = dict(st_value=int(address, 16), st_size=int(size, 16))

        def data_at(address, size):
            for segment in elf.iter_segments():
                start = segment['p_vaddr']
                if start <= address and address + size <= start + segment['p_filesz']:
                    return segment.data()[address - start:address - start + size]
            raise ValueError('ELF address has no section')

        table = symbols['_u_boot_list_2_cmd_2_r5sboot']
        entry = data_at(table['st_value'], table['st_size'])
        name, maximum, repeat, handler = struct.unpack_from('<QI4xQQ', entry)
        require(data_at(name, 8) == b'r5sboot\0', 'Wrong registered command name')
        require(maximum == 1 and handler == symbols['do_r5sboot']['st_value'],
                'Command table does not reference do_r5sboot')
        environment = symbols['default_environment']
        env = data_at(environment['st_value'], environment['st_size']).split(b'\0')
        require(b'bootcmd=r5sboot' in env and b'bootdelay=-2' in env, 'Default environment differs')
        require(not any(x.startswith(b'preboot=') for x in env), 'Unexpected preboot environment')
        return dict(command_table=table['st_value'], handler=handler,
                    default_environment=[x.decode() for x in env if x])


def audit(root, destination):
    normal = root / 'out/r5s-microkit.img'
    build = destination / 'build'
    reference = root / 'out/uboot'
    require(sha256(normal) == NORMAL_SHA256, 'Normal image identity changed')
    require(sha256(root / 'sources.lock') == LOCK_SHA256, 'Original source lock changed')
    config = (build / '.config').read_text()
    require(config == (reference / '.config').read_text(), 'Diagnostic changed board/earlier-stage configuration')
    for line in config.splitlines():
        if line.startswith('CONFIG_ENV_IS_IN_'):
            raise ValueError('Persistent environment enabled')
    for symbol in ('USE_PREBOOT', 'OF_ENV_SETUP', 'BOOTCOUNT_LIMIT', 'BOOTSTD', 'NET',
                   'USB', 'PCI', 'MMC_WRITE', 'SPL_MMC_WRITE', 'MMC_SDHCI', 'WDT',
                   'WATCHDOG', 'BUTTON_CMD', 'UPDATE_TFTP', 'EFI_LOADER', 'FAT_WRITE'):
        require(f'CONFIG_{symbol}=y' not in config, f'Unsafe configuration: {symbol}')
    for line in ('CONFIG_ENV_IS_NOWHERE=y', 'CONFIG_BOOTCOMMAND="r5sboot"',
                 'CONFIG_BOOTDELAY=-2', 'CONFIG_ROCKCHIP_BOOT_MODE_REG=0'):
        require(line in config.splitlines(), f'Missing {line}')
    compiled = (build / 'cmd/.r5sboot.o.cmd').read_text()
    require('-DR5S_UBOOT_DIAGNOSTIC' in compiled, 'Diagnostic define missing from compiler command')
    (destination / 'logs/command-compile.txt').write_text(compiled)
    undefined = output('aarch64-none-elf-nm', '-u', build / 'cmd/r5sboot.o')
    require({line.split()[-1] for line in undefined.splitlines()} == {'udelay', 'cmd_never_repeatable'},
            'Diagnostic command has unexpected dependencies')
    (destination / 'logs/command-undefined.txt').write_text(undefined + '\n')
    disassembly = output('aarch64-none-elf-objdump', '-dr', build / 'cmd/r5sboot.o')
    (destination / 'logs/command-disassembly.txt').write_text(disassembly + '\n')
    linked = output('aarch64-none-elf-objdump', '-d', build / 'u-boot', '--disassemble=do_r5sboot')
    (destination / 'logs/linked-command-disassembly.txt').write_text(linked + '\n')
    for path in ('idbloader.img', 'spl/u-boot-spl.bin', 'spl/u-boot-spl.dtb', 'u-boot.dtb'):
        require(sha256(build / path) == sha256(reference / path), f'Earlier-stage/DT changed: {path}')
    binmap = (build / 'simple-bin.map').read_text()
    fit = re.search(r'^\s*([0-9a-f]+)\s+[0-9a-f]+\s+[0-9a-f]+\s+fit\s*$', binmap, re.M)
    require(fit is not None and int(fit[1], 16) + 32768 == 0x800000, 'Unexpected FIT placement')
    dtb = build / 'u-boot.dtb'
    nodes = output('fdtget', '-l', dtb, '/').splitlines()
    require('config' not in nodes, 'FDT /config could override automatic boot')
    for node in ('/pinctrl', '/pinctrl/gpio@fe770000', '/pinctrl/gpio-leds/sys-led-pin'):
        output('fdtget', '-p', dtb, node)
    require(output('fdtget', '-t', 'x', dtb, '/pinctrl/gpio-leds/sys-led-pin', 'rockchip,pins').split()[:3]
            == ['4', '1a', '0'], 'SYS pinmux differs')
    require(output('fdtget', '-t', 'x', dtb, '/pinctrl/gpio@fe770000', 'reg') == '0 fe770000 0 100',
            'GPIO4 register range differs')
    require(output('fdtget', dtb, '/mmc@fe310000', 'status') == 'disabled', 'eMMC node is enabled')
    inventory = output(build / 'tools/dumpimage', '-l', build / 'u-boot.itb')
    (destination / 'logs/fit-inventory.txt').write_text(inventory + '\n')
    image_names = output('fdtget', '-l', build / 'u-boot.itb', '/images').splitlines()
    require(image_names == output('fdtget', '-l', reference / 'u-boot.itb', '/images').splitlines(),
            'FIT component list changed')
    components = {}
    with tempfile.TemporaryDirectory() as temporary:
        temporary = Path(temporary)
        for index, name in enumerate(image_names):
            paths = []
            for label, directory in (('normal', reference), ('diagnostic', build)):
                extracted = temporary / label
                subprocess.run([str(build / 'tools/dumpimage'), '-T', 'flat_dt', '-p', str(index),
                                '-o', str(extracted), str(directory / 'u-boot.itb')],
                               check=True, stdout=subprocess.DEVNULL)
                paths.append(sha256(extracted))
                advertised = output('fdtget', '-t', 'bx', directory / 'u-boot.itb',
                                    f'/images/{name}/hash', 'value')
                require(bytes(int(x, 16) for x in advertised.split()).hex() == paths[-1],
                        f'FIT hash mismatch: {label}/{name}')
                if label == 'diagnostic' and name == 'u-boot':
                    require(paths[-1] == sha256(build / 'u-boot-nodtb.bin'), 'FIT contains stale U-Boot')
            require((paths[0] != paths[1]) == (name == 'u-boot'), f'Unexpected FIT change: {name}')
            components[name] = dict(normal_sha256=paths[0], diagnostic_sha256=paths[1],
                                    changed=paths[0] != paths[1])
    raw = (destination / 'u-boot-rockchip.bin').read_bytes()
    require(raw == (build / 'u-boot-rockchip.bin').read_bytes(), 'Copied firmware differs from build')
    start = int(fit[1], 16)
    require(raw[start:start + (build / 'u-boot.itb').stat().st_size] == (build / 'u-boot.itb').read_bytes(),
            'Raw image does not contain this FIT')
    return dict(normal_command=command_evidence(reference / 'u-boot'),
                diagnostic_command=command_evidence(build / 'u-boot'),
                fit_components=components, configuration_sha256=sha256(build / '.config'),
                identical_inputs={p: sha256(build / p) for p in
                                  ('idbloader.img', 'spl/u-boot-spl.bin', 'spl/u-boot-spl.dtb', 'u-boot.dtb')},
                firmware_audit='No command storage calls; earlier BootROM probing is outside command control',
                binman_map=binmap)


def assemble(root, destination):
    directory = root / 'out/diag-uboot'
    evidence = audit(root, directory)
    (directory / 'audit.json').write_text(json.dumps(evidence, indent=2) + '\n')
    policy = root / 'firmware/diag-boot-policy.txt'
    uboot = directory / 'u-boot-rockchip.bin'
    lock = json.loads((root / 'sources.lock').read_text())
    patch = output('git', '-C', root / 'vendor/u-boot', 'diff', '--binary', '--',
                   'cmd/Makefile', 'arch/arm/dts/rk3568-nanopi-r5s-u-boot.dtsi')
    (directory / 'u-boot.patch').write_text(patch + '\n')
    manifest = dict(format=2, profile=PROFILE, pattern=PATTERN, loader=None,
                    payload_handoff=False, hardware_tested=False, lock_sha256=LOCK_SHA256,
                    normal_image=dict(sha256=NORMAL_SHA256, size=96 * 1024**2),
                    uboot=dict(size=uboot.stat().st_size, sha256=sha256(uboot)),
                    policy_sha256=sha256(policy), sources=lock['sources'],
                    firmware={p: h for p, h in lock['files'].items() if p.startswith('vendor/rkbin/')},
                    config_sha256=evidence['configuration_sha256'],
                    changes={p: sha256(root / p) for p in ('firmware/r5sboot.c', 'firmware/diag-boot-policy.txt',
                                                          'scripts/build-uboot', 'scripts/diag.py', 'scripts/image.py')},
                    patch_sha256=sha256(directory / 'u-boot.patch'))
    seed = hashlib.sha256((PROFILE + sha256(uboot)).encode()).hexdigest()
    assemble_files(destination, uboot, [(policy, 'boot.txt')], manifest, seed)
    return verify(destination)


def record_artifacts(root, destination, manifest):
    import shutil
    import tarfile

    directory = root / 'out/diag-uboot'
    staged = directory / 'handoff'
    (staged / 'scripts').mkdir(parents=True, exist_ok=True)
    for name in ('flash-sd', 'verify-image', 'image.py', 'policy.py', 'diag_profile.py', 'trace_profile.py'):
        shutil.copy2(root / 'scripts' / name, staged / 'scripts' / name)
    for name in ('r5sboot.c', 'diag-boot-policy.txt', 'uboot-config', 'sd-only.dtsi'):
        folder = directory / 'firmware-source'
        folder.mkdir(exist_ok=True)
        shutil.copy2(root / 'firmware' / name, folder / name)
    shutil.copy2(destination, staged / 'r5s-uboot-diag.img')
    (staged / 'r5s-uboot-diag.manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    shutil.copy2(root / 'docs/uboot-diagnostic.md', staged / 'HANDOFF.md')
    shutil.copy2(root / 'sources.lock', staged / 'sources.lock')
    for name in ('audit.json', 'uboot.config', 'u-boot.patch'):
        shutil.copy2(directory / name, staged / name)
    handoff_files = sorted(p for p in staged.rglob('*') if p.is_file() and p.name != 'SHA256SUMS')
    (staged / 'SHA256SUMS').write_text(''.join(f'{sha256(p)}  {p.relative_to(staged)}\n' for p in handoff_files))
    bundle = directory / 'r5s-diag-handoff.tar.gz'
    with tarfile.open(bundle, 'w:gz') as archive:
        archive.add(staged, arcname='diag-uboot')
    bundle_checksum = bundle.with_name(bundle.name + '.sha256')
    bundle_checksum.write_text(f'{sha256(bundle)}  {bundle.name}\n')
    artifacts = [destination, destination.with_suffix('.manifest.json'), directory / 'u-boot.elf',
                 directory / 'u-boot.map', directory / 'u-boot-rockchip.bin', directory / 'uboot.config',
                 directory / 'audit.json', directory / 'u-boot.patch', bundle, bundle_checksum]
    record = dict(profile=PROFILE, built=True, image_validated=True, simulated=False,
                  hardware_tested=False, pattern=PATTERN, normal_image=manifest['normal_image'],
                  prior_recovery='User observed Ubuntu SSH after SD removal; no before/after eMMC hashes',
                  artifacts={str(p.relative_to(directory)): dict(size=p.stat().st_size, sha256=sha256(p))
                             for p in artifacts})
    (directory / 'ARTIFACTS.json').write_text(json.dumps(record, indent=2) + '\n')
    artifacts += [directory / 'ARTIFACTS.json']
    (directory / 'SHA256SUMS').write_text(''.join(f'{sha256(p)}  {p.relative_to(directory)}\n' for p in artifacts))
