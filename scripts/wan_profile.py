import re
from diagnostics import SPEC
from router_config import validate

def validate_wan_manifest(m):
 if (m.get('format')!=7 or m.get('profile')!='wan-dhcp' or m.get('hardware_tested') is not False or
     m.get('dma_enabled') is not True or m.get('diagnostics')!=SPEC or m.get('services')!=['wan-dhcp-client','arp','finite-gateway-icmp'] or
     m.get('pool')!=dict(virtual=0x60000000,physical=0x24000000,bus=0x24000000,size=0x40000,attributes='Normal-WB',alignment=4096) or
     not re.fullmatch('[a-f0-9]{32}',m.get('build_id','')) or 'negative_test' in m):raise ValueError('Invalid WAN profile')
 validate(m['configuration'])
