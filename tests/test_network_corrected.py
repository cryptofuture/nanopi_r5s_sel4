import json,subprocess,tempfile,unittest,sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];BASE=ROOT/'out/network-corrected'
sys.path.insert(0,str(ROOT/'scripts'))
class CorrectedNetwork(unittest.TestCase):
 def test_health(self):
  with tempfile.TemporaryDirectory() as folder:
   p=Path(folder)/'test';subprocess.run(['rustc','--edition','2021','--test',ROOT/'tests/network_health_test.rs','-o',p],check=True);subprocess.run([p],check=True)
 def test_compiled_phy(self):
  paths=list((BASE/'rawio/work/driver-gmac-rawio/target').rglob('link-profile.c'));self.assertEqual(len(paths),1)
  with tempfile.TemporaryDirectory() as folder:
   p=Path(folder);stub=p/'stub.c';stub.write_text('int dma_enabled(void) {return 0;} int dma_prepare_link(unsigned s,unsigned d) {(void)s;(void)d;return 0;}')
   exe=p/'test';subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-fsanitize=undefined,address','-I'+str(ROOT/'drivers/gmac0'),ROOT/'tests/preflight_phy_test.c',ROOT/'tests/preflight_link_fixture.c',paths[0],ROOT/'drivers/gmac0/mdio.c',stub,'-o',exe],check=True);subprocess.run([exe],check=True)
 def test_images_and_caps(self):
  from image import verify
  from policy import sha256
  raw=BASE/'rawio';wan=BASE/'wan'
  for base,name in [(raw,'gmac-rawio'),(wan,'wan-dhcp')]:
   m=verify(base/f'r5s-{name}.img');assert 'vccio1-boot-policy' in m['corrections'];assert m['hardware_tested'] is False
   assert sha256(base/f'r5s-{name}.img')==sha256(base/f'r5s-{name}.reproduced.img')
   output=base/name;report=(output/'microkit-report.txt').read_text();assert "* Slot: 3\n\t\t\t-> Object: 'pud_gmac'" in report
   assert sum(o['physical'] is not None and 0x24000000<=o['physical']<0x24040000 for o in (json.loads((base/'layout-report.json').read_text())[name] if name=='gmac-rawio' else json.loads((base/'layout-report.json').read_text()))['capdl_objects'])==64
  assert (raw/'gmac-rawio/driver.elf').read_bytes()==(wan/'wan-dhcp/driver.elf').read_bytes()
 def test_snapshot_before_time_and_wan_no_proof(self):
  text=(ROOT/'pds/network_supervisor/src/main.rs').read_text()
  assert text.index('let driver=shared::read')<text.index('let time=shared::millis()')
  assert 'if policy::WAN {link &=' in text
  assert 'else if link && a[4]==1' in text
  assert (ROOT/'pds/network_supervisor/src/wan_policy.rs').read_text()=='pub const WAN:bool=true;\n'
if __name__=='__main__':unittest.main()
