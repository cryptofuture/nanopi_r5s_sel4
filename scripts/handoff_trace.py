import hashlib
import inspect
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys

from policy import check_layout, check_ram, elf_segments, sha256
from image import assemble_files, verify
from trace_profile import PROFILES, STAGES, ERRORS

ROOT = Path(__file__).resolve().parents[1]
BASELINES = {'out/r5s-microkit.img': 'c9ba7c07b801bb1d0beaa7cd4fdfe56f941435b723aad3477e9f90df03406304',
             'out/diag-uboot/r5s-uboot-diag.img': '7c231a8b7680e06fb8e0c48433f76fb68d9e047ada839fe07b1782bdfac93769',
             'sources.lock': '55f1cd7f5fb18ac1e62155ddff27c600b68b233b8a0036440f8d389e0d75c7aa'}


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def run(args, log, **kwargs):
    with log.open('w') as stream:
        subprocess.run([str(a) for a in args], stdout=stream, stderr=subprocess.STDOUT, check=True, **kwargs)


def replace(path, before, after):
    text = path.read_text()
    require(text.count(before) == 1, f'Patch anchor not unique: {path}: {before[:60]}')
    path.write_text(text.replace(before, after))


def clone_inputs(work, lock):
    for name in ('microkit', 'sel4', 'u-boot'):
        destination = work / name
        if not destination.exists():
            subprocess.run(['git', 'clone', '--shared', '--no-hardlinks', str(ROOT / 'vendor' / name),
                            str(destination)], check=True, stdout=subprocess.DEVNULL)
            subprocess.run(['git', '-C', str(destination), 'checkout', '--detach', lock['sources'][name]['commit']],
                           check=True, stdout=subprocess.DEVNULL)
        actual = subprocess.check_output(['git', '-C', str(destination), 'rev-parse', 'HEAD'], text=True).strip()
        require(actual == lock['sources'][name]['commit'], f'Wrong trace source revision: {name}')


def instrument(work, stop):
    microkit, kernel = work / 'microkit', work / 'sel4'
    for repository in (microkit, kernel):
        require(not subprocess.check_output(['git', '-C', str(repository), 'diff', '--name-only']).strip(),
                'Trace worktree already modified; use a fresh output directory')
    header = f'#define R5S_STOP_CHECKPOINT {stop}\n' + (ROOT / 'firmware/r5s_trace.h').read_text()
    for directory in (microkit / 'loader/src', microkit / 'monitor/src', kernel / 'include'):
        (directory / 'r5s_trace.h').write_text(header)
    replace(microkit / 'loader/src/aarch64/crt0.S',
            '    adrp    x1, _stack\n', '    adrp    x1, _stack\n    add x1, x1, :lo12:_stack\n')
    replace(microkit / 'loader/src/loader.c', '#include "loader.h"', '#include "loader.h"\n#include "r5s_trace.h"')
    replace(microkit / 'loader/src/loader.c', '    uart_init();\n    /* After',
            '    r5s_checkpoint(4);\n    uart_init();\n    /* After')
    replace(microkit / 'loader/src/loader.c', '    arch_jump_to_kernel(logical_cpu);',
            '    r5s_checkpoint(5);\n    arch_jump_to_kernel(logical_cpu);')
    replace(kernel / 'src/arch/arm/64/kernel/vspace.c', '    map_kernel_devices();',
            '    map_kernel_devices();\n    map_kernel_frame(0xfe770000, KDEV_BASE + 0x1f0000, VMKernelOnly,\n'
            '                     vm_attributes_new(true, false, false));')
    replace(kernel / 'src/arch/arm/kernel/boot.c', '#include <plat/machine/hardware.h>',
            '#include <plat/machine/hardware.h>\n#define R5S_GPIO_BASE (KDEV_BASE + 0x1f0000)\n#include "r5s_trace.h"')
    replace(kernel / 'src/arch/arm/kernel/boot.c', '    /* debug output via serial port is only available from here */',
            '    r5s_checkpoint(6);\n    /* debug output via serial port is only available from here */')
    replace(microkit / 'monitor/src/main.c', 'void main(void)\n{',
            '#define R5S_GPIO_BASE 0x40000000UL\n#include "r5s_trace.h"\n\nvoid main(void)\n{\n'
            '    r5s_checkpoint(7);\n    __atomic_store_n((unsigned int *)0x40001000UL, 1, __ATOMIC_RELEASE);')
    replace(microkit / 'tool/microkit/src/capdl/builder.rs',
            '    // Step 3. Create the PDs\' spec',
            '''    // Trace monitor shares the PD's single GPIO frame and owner page.
    if let (Some(gpio), Some(sync)) = (mr_name_to_frames.get("gpio4"), mr_name_to_frames.get("trace_sync")) {
        for (id, cached, vaddr) in [(gpio[0], false, 0x40000000), (sync[0], true, 0x40001000)] {
            let cap = capdl_util_make_frame_cap(id, true, true, false, cached);
            monitor_elf_spec.address_space.map_page(&mut spec_container, kernel_config, cap, 4096, vaddr)?;
        }
    }
    // Step 3. Create the PDs' spec''')


def build_pd(base, sdk, stop):
    project = base / 'work/heartbeat'
    shutil.copytree(ROOT / 'pds/heartbeat', project, ignore=shutil.ignore_patterns('target'), dirs_exist_ok=True)
    replace(project / 'Cargo.toml', '../../vendor/rust-sel4/crates/sel4-microkit',
            str(ROOT / 'vendor/rust-sel4/crates/sel4-microkit'))
    source = project / 'src/main.rs'
    replace(source, 'mod gpio;', 'mod gpio;\nmod trace;')
    replace(source, 'fn init() -> impl Handler {', 'fn init() -> impl Handler {\n    trace::wait_owner();')
    replace(source, '    if cfg!(feature = "fault-injection") {',
            '    trace::checkpoint(8);\n    if cfg!(feature = "fault-injection") {')
    text = source.read_text().replace('delay(100_000_000)', 'delay(150)').replace('delay(1_000_000_000)', 'delay(2000)')
    text = text.replace('    for _ in 0..iterations {\n        core::hint::spin_loop();\n    }', '    trace::delay(iterations);')
    source.write_text(text)
    (project / 'src/trace.rs').write_text(f'''use core::sync::atomic::{{AtomicU32, Ordering}};
pub fn wait_owner() {{
    while unsafe {{ &*(0x40001000 as *const AtomicU32) }}.load(Ordering::Acquire) != 1 {{
        core::hint::spin_loop();
    }}
}}
fn ticks() -> u64 {{
    let value;
    unsafe {{ core::arch::asm!("isb", "mrs {{0}}, cntvct_el0", out(reg) value, options(nostack)); }}
    value
}}
pub fn delay(ms: u32) {{
    let start = ticks();
    while ticks().wrapping_sub(start) < u64::from(ms) * 24000 {{ core::hint::spin_loop(); }}
}}
fn led(on: bool) {{
    unsafe {{
        core::ptr::write_volatile(0x40000004 as *mut u32, 0x04000000 | if on {{ 0x400 }} else {{ 0 }});
        core::arch::asm!("dsb sy", options(nostack));
    }}
}}
pub fn checkpoint(code: u32) {{
    loop {{
        led(true); delay(1000); led(false); delay(500);
        for _ in 0..code {{ led(true); delay(150); led(false); delay(250); }}
        delay(2000);
        if code != {stop} {{ break; }}
    }}
}}
''')
    target = base / 'aarch64-sel4.json'
    shutil.copyfile(ROOT / 'vendor/rust-sel4/support/targets/aarch64-sel4.json', target)
    env = dict(os.environ, SEL4_INCLUDE_DIRS=str(sdk / 'board/nanopi_r5s_trace/debug/include'))
    run(['cargo', 'build', '--manifest-path', project / 'Cargo.toml', '--release', '--locked',
         '-Zbuild-std=core,alloc,compiler_builtins', '-Zbuild-std-features=compiler-builtins-mem',
         '-Zjson-target-spec', '--target', target], base / 'logs/pd.log', env=env)
    shutil.copyfile(project / 'target/aarch64-sel4/release/r5s-heartbeat.elf', base / 'heartbeat.elf')
    xml = (ROOT / 'microkit/system.system').read_text().replace(
        '    <protection_domain', '    <memory_region name="trace_sync" size="0x1000" />\n    <protection_domain')
    xml = xml.replace('name="heartbeat" priority="100"', 'name="heartbeat" priority="100" budget="1000" period="1000"')
    xml = xml.replace('        <program_image', '        <map mr="trace_sync" vaddr="0x40001000" perms="rw" cached="true" />\n        <program_image')
    (base / 'system.system').write_text(xml)


def build_uboot(base, profile, payload, stop, trace_header=None):
    source = base / 'work/u-boot'
    output = base / profile
    output.mkdir(exist_ok=True)
    build = output / 'build'
    (source / 'configs/nanopi-r5s-trace_defconfig').write_text(
        (source / 'configs/nanopi-r5s-rk3568_defconfig').read_text() + '\n' + (ROOT / 'firmware/uboot-config').read_text() +
        '\nCONFIG_EFI_PARTITION=y\n# CONFIG_SPL_EFI_PARTITION is not set\n# CONFIG_SPL_PARTITIONS is not set\n')
    original = subprocess.check_output(['git', '-C', str(source), 'show', 'HEAD:arch/arm/dts/rk3568-nanopi-r5s-u-boot.dtsi'])
    (source / 'arch/arm/dts/rk3568-nanopi-r5s-u-boot.dtsi').write_bytes(original + b'\n' + (ROOT / 'firmware/sd-only.dtsi').read_bytes())
    for name in ('r5sboot-trace.c', 'r5s_enter.S'):
        shutil.copyfile(ROOT / 'firmware' / name, source / 'cmd' / name)
    (source / 'cmd/r5s_trace.h').write_text(f'#define R5S_STOP_CHECKPOINT {stop}\n' +
        (trace_header if trace_header is not None else (ROOT / 'firmware/r5s_trace.h').read_text()))
    digest = ','.join('0x' + sha256(payload)[i:i + 2] for i in range(0, 64, 2))
    (source / 'cmd/r5s_payload.h').write_text(f'#define R5S_PAYLOAD_SHA256 {{{digest}}}\n#define R5S_PAYLOAD_SIZE {payload.stat().st_size}\n')
    original = subprocess.check_output(['git', '-C', str(source), 'show', 'HEAD:cmd/Makefile'])
    (source / 'cmd/Makefile').write_bytes(original + b'\nifndef CONFIG_XPL_BUILD\nobj-y += r5sboot-trace.o r5s_enter.o\nendif\n')
    env = dict(os.environ, CROSS_COMPILE='aarch64-none-elf-',
               BL31=str(ROOT / 'vendor/rkbin/bin/rk35/rk3568_bl31_v1.46.elf'),
               ROCKCHIP_TPL=str(ROOT / 'vendor/rkbin/bin/rk35/rk3568_ddr_1560MHz_v1.26.bin'),
               SOURCE_DATE_EPOCH='1767225600', KBUILD_BUILD_USER='r5s', KBUILD_BUILD_HOST='builder')
    run(['make', '-C', source, f'O={build}', 'nanopi-r5s-trace_defconfig'], base / f'logs/{profile}-config.log', env=env)
    def options(path):
        result = {}
        for line in path.read_text().splitlines():
            if line.startswith('CONFIG_'):
                key, value = line.split('=', 1)
                result[key] = value
        return result
    before = options(ROOT / 'out/diag-uboot/build/.config')
    after = options(build / '.config')
    delta = {k: [before.get(k), after.get(k)] for k in before.keys() | after.keys() if before.get(k) != after.get(k)}
    allowed = {'CONFIG_PARTITIONS', 'CONFIG_EFI_PARTITION', 'CONFIG_LIB_UUID', 'CONFIG_PARTITION_UUIDS',
               'CONFIG_EFI_PARTITION_ENTRIES_NUMBERS', 'CONFIG_EFI_PARTITION_ENTRIES_OFF'}
    require(set(delta) <= allowed and after.get('CONFIG_EFI_PARTITION') == 'y',
            f'Unexpected configuration delta beyond the GPT parser fix: {delta}')
    (output / 'config-delta.json').write_text(json.dumps(delta, indent=2) + '\n')
    run(['make', '-C', source, f'O={build}', '-j8'], base / f'logs/{profile}-uboot.log', env=env)
    for name in ('idbloader.img', 'spl/u-boot-spl.bin', 'u-boot.dtb'):
        require(sha256(build / name) == sha256(ROOT / 'out/diag-uboot/build' / name), f'Earlier firmware changed: {name}')
    shutil.copyfile(build / 'u-boot-rockchip.bin', output / 'u-boot-rockchip.bin')
    shutil.copyfile(source / 'cmd/r5s_payload.h', output / 'r5s_payload.h')
    shutil.copyfile(source / 'cmd/r5s_trace.h', output / 'uboot-r5s_trace.h')
    shutil.copyfile(build / 'u-boot', output / 'u-boot.elf')
    shutil.copyfile(build / '.config', output / 'uboot.config')
    return output


def symbols(path):
    values = {}
    for line in subprocess.check_output(['aarch64-none-elf-nm', '-S', '--defined-only', str(path)], text=True).splitlines():
        fields = line.split()
        if len(fields) == 4:
            values[fields[3]] = dict(address=int(fields[0], 16), size=int(fields[1], 16))
        elif len(fields) == 3:
            values[fields[2]] = dict(address=int(fields[0], 16), size=0)
    return values


def layout(base, sdk, payload, loader_elf, output, allowed_mmio=(), cores=1, sdk_config='debug'):
    board = sdk / f'board/nanopi_r5s_trace/{sdk_config}'
    memory = json.loads((board / 'platform_gen.json').read_text())['memory']
    require(memory == [dict(start=0x20000000, end=0x40000000)], 'Changed RAM policy')
    config = (board / 'include/kernel/gen_config.h').read_text()
    require('#define CONFIG_EXPORT_VCNT_USER  1' in config or '#define CONFIG_EXPORT_VCNT_USER 1' in config,
            'EL0 virtual counter is not exported by this kernel')
    require(re.search(rf'#define CONFIG_MAX_NUM_NODES\s+{cores}\b', config), 'Wrong kernel CPU count')
    require(cores == 1 or re.search(r'#define CONFIG_ENABLE_SMP_SUPPORT\s+1\b', config),
            'SMP is disabled')
    data = payload.read_bytes()
    loader_entry, loader_segments = elf_segments(loader_elf)
    require(loader_entry == 0x30000000, 'Loader entry differs from branch address')
    syms = symbols(loader_elf)
    require(syms['_start']['address'] == loader_entry, 'ELF entry is not _start')
    from elftools.elf.elffile import ELFFile
    with loader_elf.open('rb') as stream:
        elf = ELFFile(stream)
        load = next(segment for segment in elf.iter_segments() if segment['p_type'] == 'PT_LOAD')
        require(load['p_vaddr'] == loader_entry and data[:16] == load.data()[:16], 'Raw entry bytes differ from ELF')
    marker = struct.pack('<Q', 0x5e14dead14de5ead)
    found = []
    for match in re.finditer(re.escape(marker), data):
        offset = match.start()
        if offset + 64 <= len(data):
            head = struct.unpack_from('<8Q', data, offset)
            if head[1] == len(data) - offset and 0 < head[7] < 100:
                found.append((offset, head))
    require(len(found) == 1, 'Ambiguous loader metadata')
    offset, head = found[0]
    require(0x30000000 + offset == syms['_bss_end']['address'], 'Loader data/BSS boundary differs')
    table = syms['boot_lvl1_lower']['address'] - loader_entry
    device_entry = struct.unpack_from('<Q', data, table + 3 * 8)[0]
    require(device_entry & 3 == 1 and device_entry >> 2 & 7 == 0 and
            device_entry & 0x0000ffffc0000000 == 0xc0000000,
            'Loader GPIO identity window is not Device nGnRnE')
    input_end = max(0x30000000 + len(data), *(s['physical'] + s['size'] for s in loader_segments))
    regions = [('loader complete input/code/BSS/stack/page tables', 0x30000000, input_end)]
    for i in range(head[7]):
        address, size, source, kind = struct.unpack_from('<4Q', data, offset + 64 + i * 32)
        require(kind == 1 and size > 0 and offset + 64 + head[7] * 32 + source + size <= len(data), 'Invalid loader region')
        check_ram((address, address + size))
        regions.append((f'loader copy destination {i}', address, address + size))
    check_layout(regions)
    check_ram((0x30000000, input_end))
    elf_reports = {}
    for name in ('sel4', 'initialiser', 'monitor'):
        path = board / f'elf/{name}.elf'
        entry, segments = elf_segments(path)
        elf_reports[name] = dict(entry=entry, segments=segments)
        if name == 'sel4':
            for segment in segments:
                check_ram((segment['physical'], segment['physical'] + segment['size']))
    report = output.joinpath('microkit-report.txt').read_text()
    objects = []
    for line in report.splitlines():
        match = re.search(r'@ 0x([0-9a-f]+)', line)
        if match:
            address = int(match[1], 16)
            if address != 0xfe770000 and address not in allowed_mmio:
                check_ram((address, address + 4096))
            objects.append(dict(description=line.strip(), physical=address))
    require(sum(o['physical'] == 0xfe770000 for o in objects) == 1, 'GPIO frame duplicated or unavailable')
    require('pd_monitor_vaddr_0x40000000' in report and 'pd_heartbeat_vaddr_0x40000000' in report,
            'Monitor/PD MMIO mapping missing')
    build = base / f'work/microkit/build/nanopi_r5s_trace/{sdk_config}/sel4/build'
    generated = (build / 'gen_headers/plat/machine/devices_gen.h').read_text()
    offsets = [int(value, 16) for value in re.findall(r'KDEV_BASE \+ 0x([0-9a-f]+)', generated)]
    require(max(offsets) < 0x1f0000 and 0x1f0000 + 4096 <= 0x200000, 'Kernel diagnostic device slot overlaps')
    binary = (output / 'u-boot-rockchip.bin').read_bytes()
    binmap = (output / 'build/simple-bin.map').read_text()
    fit = re.search(r'^\s*([0-9a-f]+)\s+[0-9a-f]+\s+[0-9a-f]+\s+fit\s*$', binmap, re.M)
    require(fit and int(fit[1], 16) + 32768 == 0x800000, 'Changed FIT raw placement')
    require(32768 + len(binary) < 16 * 1024**2, 'Firmware overlaps FAT')
    require(binary[int(fit[1], 16):] == (output / 'build/u-boot.itb').read_bytes(), 'Packed FIT differs from new build')
    header = (output / 'r5s_payload.h').read_text()
    embedded = bytes(int(x, 16) for x in re.findall(r'0x([0-9a-f]{2})', header)).hex()
    require(embedded == sha256(payload), 'Stale embedded payload hash')
    require(f'#define R5S_PAYLOAD_SIZE {len(data)}\n' in header, 'Stale embedded payload length')
    return dict(loader_regions=regions, loader_symbols=syms, loader_elf=loader_segments,
                kernel_and_userspace_elf=elf_reports, capdl_objects=objects,
                live_uboot=dict(status='UNKNOWN until runtime; checked before checkpoint 3',
                                linked_elf=elf_segments(output / 'u-boot.elf')[1],
                                stack_guard='start_addr_sp >= 0x41000000; current SP in top 16MiB stack margin',
                                heap='[mem_malloc_start, mem_malloc_end), intersection checked',
                                dtb='[gd->fdt_blob, +fdt_totalsize), intersection checked',
                                relocated_code='gd->relocaddr >= 0x40000000; ram_top >= relocaddr'),
                diagnostic_ram_record='Only an 8-byte U-Boot BSS return-SP slot, not persistent logs',
                kernel_gpio_alias='KDEV_BASE + 0x1f0000, Device nGnRnE, non-owning debug alias',
                userspace_gpio=dict(physical=0xfe770000, virtual=0x40000000, cached=False, frame_objects=1),
                sync_page=dict(virtual=0x40001000, cached=True, ownership='monitor release store, PD acquire load'),
                flattened_dt_reservations='MISSING: no complete captured runtime DTB supplied',
                excluded=[[0, 0x200000], [0x8400000, 0x9400000], [0x10000000, 0x10800000], [0xf0000000, 0x1f0000000]],
                binman_map=binmap, hardware_verified=False)


def build_all(stop=0):
    for path, digest in BASELINES.items():
        require(sha256(ROOT / path) == digest, f'Baseline changed: {path}')
    base = ROOT / 'out/handoff-trace'
    if stop:
        base = base / f'stop-{stop}'
    (base / 'logs').mkdir(parents=True, exist_ok=True)
    work = base / 'work'
    work.mkdir(exist_ok=True)
    lock = json.loads((ROOT / 'sources.lock').read_text())
    subprocess.run([sys.executable, ROOT / 'scripts/sources'], check=True)
    clone_inputs(work, lock)
    stamp = work / 'trace-prepared'
    signature = dict(stop=stop, fingerprint=hashlib.sha256(
        inspect.getsource(instrument).encode() + inspect.getsource(build_pd).encode() +
        (ROOT / 'firmware/r5s_trace.h').read_bytes() + (ROOT / 'scripts/build-trace-sdk').read_bytes() +
        (ROOT / 'microkit/memory-overlay.dts').read_bytes() + (ROOT / 'pds/heartbeat/src/main.rs').read_bytes()).hexdigest())
    if not stamp.exists():
        instrument(work, stop)
        stamp.write_text(json.dumps(signature))
    require(json.loads(stamp.read_text()) == signature, 'Cached trace SDK/PD inputs differ; use a fresh output directory')
    sdk = work / 'microkit/release/microkit-sdk-2.4.0'
    if not (sdk / 'board/nanopi_r5s_trace/debug/elf/initialiser.elf').exists():
        run([sys.executable, ROOT / 'scripts/build-trace-sdk', work], base / 'logs/sdk.log')
    if not (base / 'heartbeat.elf').exists():
        build_pd(base, sdk, stop)
    patches = base / 'patches'
    patches.mkdir(exist_ok=True)
    for name in ('sel4', 'microkit'):
        diff = subprocess.check_output(['git', '-C', str(work / name), 'diff', '--binary'])
        (patches / f'{name}.patch').write_bytes(diff)
    for name in ('r5s_trace.h', 'r5sboot-trace.c', 'r5s_enter.S'):
        shutil.copyfile(ROOT / 'firmware' / name, patches / name)
    for name in ('handoff_trace.py', 'build-trace-sdk', 'build-handoff-trace', 'trace_profile.py'):
        shutil.copyfile(ROOT / 'scripts' / name, patches / name)
    patch_hash = hashlib.sha256(b''.join(p.read_bytes() for p in sorted(patches.iterdir()))).hexdigest()
    (base / 'stage-map.json').write_text(json.dumps(dict(stages=STAGES, errors=ERRORS,
        progress=dict(preamble=[1000, 500], pulse=[150, 250], pause=2000),
        error=dict(preamble=[1500, 500, 1500, 500], pulse=[150, 250], pause=2000)), indent=2) + '\n')
    layouts = {}
    profiles = ['boot-trace'] if stop else list(PROFILES)
    for profile in profiles:
        selected_stop = stop or PROFILES[profile]
        output = base / profile
        output.mkdir(exist_ok=True)
        payload = output / 'loader.img'
        loader_dir = sdk / 'board/nanopi_r5s_trace/debug/elf'
        header = work / 'microkit/loader/src/r5s_trace.h'
        loader_stop = selected_stop if selected_stop >= 4 else 0
        header.write_text(f'#define R5S_STOP_CHECKPOINT {loader_stop}\n' + (ROOT / 'firmware/r5s_trace.h').read_text())
        shutil.copyfile(header, output / 'loader-r5s_trace.h')
        # The upstream make dependency files track loader header changes.
        run(['make', '-C', work / 'microkit/loader', 'ARCH=aarch64', 'BOARD=nanopi_r5s_trace',
             f'BUILD_DIR={work}/microkit/build/nanopi_r5s_trace/debug/loader',
             f'SEL4_SDK={sdk}/board/nanopi_r5s_trace/debug', 'TARGET_TRIPLE=aarch64-none-elf',
             'GCC_CPU=cortex-a55', 'LINK_ADDRESS=0x30000000', 'LLVM=False', 'all'], base / f'logs/{profile}-loader.log')
        shutil.copyfile(work / 'microkit/build/nanopi_r5s_trace/debug/loader/loader.elf', loader_dir / 'loader.elf')
        shutil.copyfile(loader_dir / 'loader.elf', output / 'loader.elf')
        run([sdk / 'bin/microkit', base / 'system.system', '--board', 'nanopi_r5s_trace', '--config', 'debug',
             '--search-path', base, '-o', payload, '-r', output / 'microkit-report.txt'], base / f'logs/{profile}-microkit.log')
        build_uboot(base, profile, payload, selected_stop)
        layouts[profile] = layout(base, sdk, payload, output / 'loader.elf', output)
        policy = output / 'boot.txt'
        policy.write_text(f'R5S {profile}, stop checkpoint {selected_stop}.\n'
                          'Real Microkit payload; size/SHA-256 bound into this U-Boot.\n'
                          'Stage frames: long 1000/500ms preamble, N 150/250ms pulses, OFF 2000ms.\n'
                          'Errors: two 1500/500ms preamble pulses, numbered 150/250ms pulses, OFF 2000ms.\n'
                          'Only Rust emits unprefixed three-short success. No runtime storage writes.\n')
        uboot = output / 'u-boot-rockchip.bin'
        manifest = dict(format=3, profile=profile, stop_checkpoint=selected_stop, hardware_tested=False,
                        load_address=0x30000000, ram=[0x20000000, 0x40000000], patches_sha256=patch_hash,
                        loader=dict(size=payload.stat().st_size, sha256=sha256(payload)),
                        uboot=dict(size=uboot.stat().st_size, sha256=sha256(uboot)),
                        policy_sha256=sha256(policy), lock_sha256=sha256(ROOT / 'sources.lock'), sources=lock['sources'])
        image = base / f'r5s-{profile}.img'
        require(not image.exists(), f'Output exists: {image}; preserve it before rebuilding')
        seed = sha256(payload) + sha256(uboot)
        assemble_files(image, uboot, [(payload, 'loader.img'), (policy, 'boot.txt')], manifest, seed)
        reproduced = base / f'r5s-{profile}.reproduced.img'
        assemble_files(reproduced, uboot, [(payload, 'loader.img'), (policy, 'boot.txt')], manifest, seed)
        require(sha256(image) == sha256(reproduced), 'Final image assemblies are not reproducible')
        print(f'Built and validated {profile}: {sha256(image)}', flush=True)
    (base / 'layout-report.json').write_text(json.dumps(layouts, indent=2) + '\n')
    for path, digest in BASELINES.items():
        require(sha256(ROOT / path) == digest, f'Baseline changed: {path}')
