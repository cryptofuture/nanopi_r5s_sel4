from xml.etree import ElementTree as ET
from wan_profile import validate_wan_manifest

PROFILE = "router-lan-gated"
SERVICES = ["wan-dhcp-client", "arp", "finite-gateway-icmp", "wan-ingress-policy", "wan-case-led"]
GATE = {"lan1": "disabled", "lan2": "disabled", "reason": "pcie-inbound-isolation-unverified"}

def validate_manifest(m):
    if (m.get("profile") != PROFILE or m.get("services") != SERVICES or
            m.get("lan_dma") != GATE or m.get("ethernet_enabled") is not True):
        raise ValueError("Invalid gated router profile")
    validate_wan_manifest(dict(m, profile="wan-dhcp", services=SERVICES[:3]))

def system(xml):
    root = ET.fromstring(xml)
    for name, size, physical in [("gpio2","0x1000","0xfe750000"),
                                 ("router_ingress","0x4000",None), ("router_status","0x1000",None)]:
        attrs = dict(name=name,size=size)
        if physical:
            attrs["phys_addr"] = physical
        root.insert(0,ET.Element("memory_region",attrs))
    router = ET.SubElement(root,"protection_domain",name="router",priority="99",budget="100",period="1000",stack_size="0x40000")
    ET.SubElement(router,"program_image",path="router.elf")
    for owner, name, va, perms in [
        ("router","probe_status","0x40004000","r"), ("router","link_status","0x40002000","r"),
        ("router","router_status","0x40005000","rw"), ("heartbeat","router_status","0x40005000","r"),
        ("router","router_ingress","0x62000000","rw"), ("wan","router_ingress","0x62000000","rw"),
        ("gmac","gpio2","0x50007000","rw")]:
        pd = root.find(f"protection_domain[@name='{owner}']")
        ET.SubElement(pd,"map",mr=name,vaddr=va,perms=perms,cached="false" if name=="gpio2" else "true")
    return ET.tostring(root,encoding="unicode")
