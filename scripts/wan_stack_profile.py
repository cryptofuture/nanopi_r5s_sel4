from pathlib import Path
from xml.etree import ElementTree as ET

WAN_STACK = 0x8000

def check_stack(system, report):
    wan = ET.parse(system).find("protection_domain[@name='wan']")
    if wan is None or int(wan.get('stack_size', '0x2000'), 0) != WAN_STACK:
        raise ValueError('WAN stack configuration changed')
    frames = [line for line in Path(report).read_text().splitlines()
              if "Page(4 KiB): 'frame_wan_stack_" in line]
    if len(frames) * 4096 != WAN_STACK:
        raise ValueError('WAN stack allocation differs from system description')
