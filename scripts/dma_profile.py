import re
from diagnostics import SPEC
PROFILES={'gmac-dma-preflight':False,'gmac-rawio':True}
def validate_dma_manifest(m):
 p=m.get('profile')
 if (m.get('format')!=6 or p not in PROFILES or m.get('dma_enabled') is not PROFILES[p] or
     m.get('hardware_tested') is not False or m.get('diagnostics')!=SPEC or
     m.get('pool')!=dict(virtual=0x60000000,physical=0x24000000,bus=0x24000000,size=0x40000,attributes='Normal-WB',alignment=4096) or
     not re.fullmatch('[a-f0-9]{32}',m.get('build_id','')) or not m.get('patches_sha256') or 'negative_test' in m):raise ValueError('Invalid GMAC DMA profile')
