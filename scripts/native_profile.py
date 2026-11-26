PROFILES = {'runtime-baseline-v2'}
def validate_native_manifest(m):
    if (m.get('format') != 4 or m.get('profile') not in PROFILES or
        m.get('hardware_tested') is not False or m.get('ethernet_enabled') is not False or
        m.get('dma_enabled') is not False or m.get('diagnostics', {}).get('version') != 2 or
        not m.get('patches_sha256') or 'negative_test' in m):
        raise ValueError('Invalid native profile')
