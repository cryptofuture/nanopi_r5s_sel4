import re
from diagnostics import SPEC
PROFILE='gmac-dma-preflight-repair-v2'
DEADLINES=dict(first_publication_ms=100,cache_phase_ms=3000,link_init_ms=1500,execution_ms=100,phy_ms=500,poll_ms=10)
def validate_fixed_manifest(m):
 if (m.get('format')!=8 or m.get('profile')!=PROFILE or m.get('hardware_tested') is not False or
     m.get('dma_enabled') is not False or m.get('packet_io') is not False or m.get('io_verified_allowed') is not False or
     m.get('diagnostics')!=SPEC or m.get('status_abi')!=2 or m.get('deadlines')!=DEADLINES or
     m.get('pool')!=dict(virtual=0x60000000,physical=0x24000000,size=0x40000,attributes='Normal-WB',cache_line=64,page_size=4096,owner='gmac') or
     not re.fullmatch('[a-f0-9]{32}',m.get('build_id','')) or 'negative_test' in m):raise ValueError('Invalid fixed preflight profile')
