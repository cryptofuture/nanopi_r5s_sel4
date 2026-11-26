import re
from diagnostics import SPEC

def validate_gmac_manifest(m):
    if (m.get('format')!=5 or m.get('profile')!='gmac-link' or m.get('hardware_tested') is not False or
        m.get('dma_enabled') is not False or m.get('ethernet_enabled') is not True or
        m.get('diagnostics')!=SPEC or m.get('dma_guard')!='MAC RE/TE, DMA ST/SR and descriptor-base readback zero; write whitelist' or
        not re.fullmatch('[a-f0-9]{64}',m.get('evidence_sha256','')) or not m.get('patches_sha256') or 'negative_test' in m):
        raise ValueError('Invalid native GMAC link profile')
