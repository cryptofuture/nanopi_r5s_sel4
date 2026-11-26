from handoff_trace import *
def build_uboot(base, profile, payload, stop, trace_header=None):
    source = base / 'work/u-boot'
    output = base / profile
    output.mkdir(exist_ok=True)
    build = output / 'build'
    (source / 'configs/nanopi-r5s-trace_defconfig').write_text(
        (source / 'configs/nanopi-r5s-rk3568_defconfig').read_text() + '\n' + (ROOT / 'firmware/uboot-config').read_text() +
        '\nCONFIG_EFI_PARTITION=y\n# CONFIG_SPL_EFI_PARTITION is not set\n# CONFIG_SPL_PARTITIONS is not set\n')
    original = subprocess.check_output(['git', '-C', str(source), 'show', 'HEAD:arch/arm/dts/rk3568-nanopi-r5s-u-boot.dtsi'])
    (source / 'arch/arm/dts/rk3568-nanopi-r5s-u-boot.dtsi').write_bytes(original + b'\n' + (ROOT / 'firmware/sd-only.dtsi').read_bytes() + b'\n' + (ROOT / 'firmware/vccio1-boot-policy.dtsi').read_bytes())
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
    for name in ('idbloader.img', 'spl/u-boot-spl.bin'):
        require(sha256(build / name) == sha256(ROOT / 'out/diag-uboot/build' / name), f'Earlier firmware changed: {name}')
    verify_dt_policy(build / 'u-boot.dtb', ROOT / 'out/iodomain-observe/iodomain-observe/build/u-boot.dtb')
    shutil.copyfile(build / 'u-boot-rockchip.bin', output / 'u-boot-rockchip.bin')
    shutil.copyfile(source / 'cmd/r5s_payload.h', output / 'r5s_payload.h')
    shutil.copyfile(source / 'cmd/r5s_trace.h', output / 'uboot-r5s_trace.h')
    shutil.copyfile(build / 'u-boot', output / 'u-boot.elf')
    shutil.copyfile(build / '.config', output / 'uboot.config')
    return output


from iodomain_fixed_build import verify_dt_policy
