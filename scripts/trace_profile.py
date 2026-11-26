PROFILES = {'preflight-stop': 3, 'loader-entry-stop': 4, 'boot-trace': 0}
STAGES = {
    1: ('U-Boot', 'do_r5sboot after proven LED setup'),
    2: ('U-Boot', 'exact SD payload read and SHA-256 validated'),
    3: ('U-Boot', 'EL2 and live-memory/FIFO preconditions checked, before teardown'),
    4: ('Microkit loader', 'main entry after corrected stack setup and relocation, before UART/arch_init'),
    5: ('Microkit loader', 'start_kernel after arch_mmu_enable, before arch_jump_to_kernel'),
    6: ('seL4', 'try_init_kernel after init_cpu, before init_plat'),
    7: ('Microkit monitor', 'main entry with CapDL-installed GPIO/shared owner page'),
    8: ('Rust PD', 'init after owner handoff and GPIO direction setup'),
}
ERRORS = {1: 'wrong runtime EL', 2: 'SD identity/init', 3: 'GPT/FAT/file/read',
          4: 'length/range/capacity', 5: 'SHA-256 mismatch', 6: 'handoff setup', 7: 'unexpected return'}


def validate_trace_manifest(manifest):
    profile = manifest.get('profile')
    stop = manifest.get('stop_checkpoint')
    if (manifest.get('format') != 3 or profile not in PROFILES or
            not isinstance(stop, int) or stop not in range(9) or
            (profile != 'boot-trace' and stop != PROFILES[profile]) or
            manifest.get('hardware_tested') is not False or
            not manifest.get('patches_sha256') or 'negative_test' in manifest):
        raise ValueError('Invalid handoff trace profile')
