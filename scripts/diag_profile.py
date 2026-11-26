NORMAL_SHA256 = 'c9ba7c07b801bb1d0beaa7cd4fdfe56f941435b723aad3477e9f90df03406304'
NORMAL_UBOOT_SHA256 = 'a637f021bea8c86f4cd564a5fe8631a0213e40a861559c926ac5b6ec69a6dd29'
LOCK_SHA256 = '55f1cd7f5fb18ac1e62155ddff27c600b68b233b8a0036440f8d389e0d75c7aa'
PROFILE = 'uboot-diag'
PATTERN = dict(initial_off_ms=1000, repeat=[['on', 750], ['off', 250],
                                           ['on', 750], ['off', 2250]])


def validate_manifest(manifest):
    if (manifest.get('format') != 2 or manifest.get('profile') != PROFILE
            or manifest.get('pattern') != PATTERN or manifest.get('loader', 'absent') is not None
            or manifest.get('payload_handoff') is not False
            or manifest.get('hardware_tested') is not False
            or manifest.get('lock_sha256') != LOCK_SHA256
            or manifest.get('normal_image') != dict(sha256=NORMAL_SHA256, size=96 * 1024**2)
            or manifest.get('uboot', {}).get('sha256') == NORMAL_UBOOT_SHA256
            or any(k in manifest for k in ('load_address', 'ram', 'negative_test', 'negative_sha256'))):
        raise ValueError('Invalid diagnostic profile manifest')
