from pathlib import Path
from tempfile import TemporaryDirectory
import json
import subprocess
import sys
import unittest
from xml.etree import ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/"scripts"))
from lan_gated_profile import system, validate_manifest, PROFILE, GATE, SERVICES

class RouterLanGate(unittest.TestCase):
    def test_capabilities_and_manifest(self):
        baseline=ROOT/"out/router-wan-dhcp-repair"
        manifest=json.loads((baseline/"wan-manifest.json").read_text())
        manifest.update(profile=PROFILE,lan_dma=GATE,services=SERVICES)
        validate_manifest(manifest)
        for key,value in [("lan_dma",dict(GATE,lan1="enabled")),("services",SERVICES+["pcie"])]:
            with self.assertRaises(ValueError):
                validate_manifest(dict(manifest,**{key:value}))
        old=ET.parse(baseline/"build-c/wan-dhcp/system.system").getroot()
        xml=ET.fromstring(system(ET.tostring(old,encoding="unicode")))
        physical=lambda root:{int(r.attrib["phys_addr"],0) for r in root.findall("memory_region") if "phys_addr" in r.attrib}
        self.assertEqual(physical(xml),physical(old)|{0xfe750000})
        mappings=lambda name:{m.get("mr"):m.get("perms") for m in xml.find(f"protection_domain[@name=\x27{name}\x27]").findall("map")}
        self.assertEqual(mappings("router"),dict(probe_status="r",link_status="r",router_status="rw",router_ingress="rw"))
        self.assertEqual([p.get("name") for p in xml.findall("protection_domain") if "gpio2" in mappings(p.get("name"))],["gmac"])
        self.assertTrue({"router_ingress","queues","probe_status"} <= mappings("wan").keys())

    def test_led_register_models_milestones_and_full_pools(self):
        with TemporaryDirectory() as path:
            path=Path(path)
            def run(*cmd): subprocess.run(cmd,check=True,cwd=ROOT,timeout=120)
            run("rustc","--edition=2024","--crate-name=r5s_router_core","--crate-type=rlib",
                "router-core/src/lib.rs","-o",path/"libr5s_router_core.rlib")
            for source in ["tests/router_lan_pool.rs","tests/router_gated_led.rs","pds/wan_dhcp_diagnostic/milestones.rs"]:
                run("rustc","--edition=2024","--test",source,"--extern",
                    f"r5s_router_core={path}/libr5s_router_core.rlib","-o",path/"test")
                run(path/"test")
            src=ROOT/"drivers/gmac0"
            wrapper=path/"phy.c"
            wrapper.write_text((src/"link.c").read_text()+(src/"jack-led.c.inc").read_text())
            run("cc","-std=c11","-Wall","-Wextra","-Werror","-fsanitize=address,undefined",
                "-I"+str(src),"tests/router_jack_led.c",wrapper,src/"mdio.c","-o",path/"phy")
            run(path/"phy")

    def test_wan_arp_and_local_demultiplex(self):
        subprocess.run(["cargo","test","--offline","--locked","--manifest-path",
                        "tests/wan_lan_ingress/Cargo.toml"],cwd=ROOT,check=True,timeout=120)
