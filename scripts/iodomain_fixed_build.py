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
    instrument(source, payload, output)
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


def instrument(source, payload, output):
    import struct
    marker=struct.pack('<I',0x494f5231)*64
    data=payload.read_bytes();require(data.count(marker)==1,'Handoff marker not unique in real payload')
    offset=data.index(marker);require(offset%4==0,'Handoff alignment')
    (output/'handoff.json').write_text(json.dumps(dict(payload_offset=offset,size=256,input_physical=0x30000000+offset,transport='supervisor initialized data copied by existing loader',marker=marker.hex()),indent=2)+'\n')
    p=source/'drivers/misc/rockchip-io-domain.c'
    original=subprocess.check_output(['git','-C',str(source),'show','HEAD:drivers/misc/rockchip-io-domain.c'],text=True)
    s=original
    at='static int rk3568_iodomain_write('
    declarations='''
u32 r5s_io_results[4][10];
static int r5s_io_slot(int idx) { return idx==1?0:idx==2?1:idx==5?2:idx==7?3:-1; }
static u32 *r5s_io_row(int idx) { int n=r5s_io_slot(idx); return n<0?NULL:r5s_io_results[n]; }
'''
    s=s.replace(at,declarations+at)
    s=s.replace('u32 is_3v3 = uV > MAX_VOLTAGE_1_8;','u32 *observation=r5s_io_row(idx);\n\tu32 is_3v3 = uV > MAX_VOLTAGE_1_8;')
    for reg,val,field in [('2','val0',4),('2','val1',5),('0','val0',4),('1','val1',5)]:
        old=f'regmap_write(grf, RK3568_PMU_GRF_IO_VSEL{reg}, {val});'
        new=f'{{ int result=regmap_write(grf, RK3568_PMU_GRF_IO_VSEL{reg}, {val}); if (observation) observation[{field}]=result; }}'
        require(s.count(old)==1,'Unexpected selector write source');s=s.replace(old,new)
    s=s.replace('struct udevice *reg;\n\t\tint uV;', 'struct udevice *reg;\n\t\tu32 *observation=soc_data->write==rk3568_iodomain_write?r5s_io_row(i):NULL;\n\t\tint uV;\n\t\tif (observation) { memset(observation,0,40); observation[0]=1; }')
    s=s.replace('ret = device_get_supply_regulator(dev, supply_name, &reg);','ret = device_get_supply_regulator(dev, supply_name, &reg);\n\t\tif (observation) { observation[1]=ret; observation[0]=2; }')
    s=s.replace('ret = regulator_autoset(reg);','ret = regulator_autoset(reg);\n\t\tif (observation) { observation[2]=ret; observation[0]=3; }')
    s=s.replace('uV = regulator_get_value(reg);','uV = regulator_get_value(reg);\n\t\tif (observation) { observation[3]=uV; observation[0]=4; }')
    s=s.replace('ret = soc_data->write(grf, soc_data->grf_offset, i, uV);','''ret = soc_data->write(grf, soc_data->grf_offset, i, uV);
        if (observation) {
            observation[0]=5;
            observation[9]=regmap_read(grf,0x148,&observation[6]);
            observation[9]|=regmap_read(grf,0x140,&observation[7]);
            observation[9]|=regmap_read(grf,0x144,&observation[8]);
        }''')
    p.write_text(s)
    import difflib
    (output/'iodomain.patch').write_text(''.join(difflib.unified_diff(original.splitlines(True),s.splitlines(True),fromfile='a/drivers/misc/rockchip-io-domain.c',tofile='b/drivers/misc/rockchip-io-domain.c')))
    p=source/'cmd/r5sboot-trace.c';s=p.read_text()
    s='extern unsigned int r5s_io_results[4][10];\n'+s
    hook=f'''
    {{
        u32 *record=(u32 *)(LOAD+{offset}UL); unsigned int i,j; u32 checksum=0;
        for (i=0;i<64;i++) if (record[i]!=0x494f5231U) return error(6,"IO observation placeholder mismatch");
        memset(record,0,256); record[0]=0x494f5232; record[1]=1;
        record[2]=readl((void *)0xfdc20148UL); record[3]=readl((void *)0xfdc20140UL); record[4]=readl((void *)0xfdc20144UL);
        for(i=0;i<4;i++) for(j=0;j<10;j++) record[8+i*10+j]=r5s_io_results[i][j];
        for(i=0;i<63;i++) checksum=((checksum<<1)|(checksum>>31))^record[i];
        record[63]=checksum;
    }}
'''
    require(s.count('    R5S_MASK_INTERRUPTS();')==1,'Handoff anchor')
    s=s.replace('    R5S_MASK_INTERRUPTS();',hook+'    R5S_MASK_INTERRUPTS();');p.write_text(s)

def verify_dt_policy(new, old):
    def dts(p): return subprocess.check_output(['dtc','-q','-I','dtb','-O','dts',p],text=True)
    before=dts(old);after=dts(new)
    import re
    pattern=r'(LDO_REG4 \{)(.*?)(\n\s*regulator-state-mem \{)'
    a=re.search(pattern,after,re.S);b=re.search(pattern,before,re.S)
    require(a is not None and b is not None,'Missing LDO_REG4')
    require('regulator-boot-on;' not in b[2] and 'regulator-always-on;' not in b[2],'Old policy differs')
    body=a[2]
    for prop in ('regulator-boot-on','regulator-always-on'):
        require(body.count(prop+';')==1,'New policy missing/duplicated')
        body=re.sub(r'\n[ \t]*'+prop+r';','',body)
    normalized=after[:a.start(2)]+body+after[a.end(2):]
    require(normalized==before,'Unexpected DT change beyond LDO_REG4 boot policy')
