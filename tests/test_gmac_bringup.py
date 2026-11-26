import io,json,struct,subprocess,sys,tarfile,tempfile,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'scripts'))
from gmac_evidence import safe_import
from fdt_evidence import Fdt,cells

class Evidence(unittest.TestCase):
 def test_image_profile_and_capabilities(self):
  from image import verify
  from gmac_profile import validate_gmac_manifest
  import xml.etree.ElementTree as ET
  base=ROOT/'out/gmac-bringup'
  if not (base/'r5s-gmac-link.img').exists():self.skipTest('Build native link image first')
  manifest=verify(base/'r5s-gmac-link.img')
  self.assertFalse(manifest['dma_enabled'])
  with self.assertRaises(ValueError):validate_gmac_manifest(dict(manifest,dma_enabled=True))
  with self.assertRaises(ValueError):validate_gmac_manifest(dict(manifest,profile='gmac-rawio'))
  root=ET.parse(base/'system.system').getroot()
  for pd in root.findall('protection_domain'):
   mappings={m.attrib['mr']:m.attrib for m in pd.findall('map')}
   if pd.attrib['name']=='gmac':
    self.assertNotIn('gpio4',mappings);self.assertEqual(pd.attrib['budget'],'200')
   elif pd.attrib['name']=='heartbeat':
    self.assertNotIn('gmac',mappings);self.assertEqual(mappings['link_status']['perms'],'r')
  report=(base/'gmac-link/microkit-report.txt').read_text()
  self.assertRegex(report,r"CNode: 'cnode_gmac'\s+\* Slot: 1[\s\S]*?\* Slot: 3\s+-> Object: 'pud_gmac'")
  symbols=subprocess.check_output(['aarch64-none-elf-nm',base/'driver.elf'],text=True)
  for name in ('link_init','link_poll','hw_read','hw_write','mdio_read','mdio_write'):self.assertRegex(symbols,r'\b'+name+r'\b')
  for name in ('gmac_tx_submit','gmac_rx_prepare','gmac_rx_copy'):self.assertNotIn(name,symbols)
 def test_archive_rejections(self):
  for name,kind in [('/absolute',tarfile.REGTYPE),('../escape',tarfile.REGTYPE),('link',tarfile.SYMTYPE),('dev',tarfile.CHRTYPE)]:
   with self.subTest(name=name),tempfile.TemporaryDirectory() as temp:
    archive=Path(temp)/'x.tar'
    with tarfile.open(archive,'w') as t:
     m=tarfile.TarInfo(name);m.type=kind;m.size=0;t.addfile(m,io.BytesIO())
    with self.assertRaises(ValueError):safe_import(archive,Path(temp)/'new')
    self.assertFalse((Path(temp)/'new').exists())
 def test_capture_and_reservations(self):
  capture=ROOT/'evidence/gmac-captured-1e089e3a/inner/gmac-evidence'
  if not capture.exists():self.skipTest('Captured evidence not imported')
  dt=Fdt((capture/'runtime-original.dtb').read_bytes())
  self.assertEqual(dt.reservations,[[0x8300000,0x832c000],[0xa200000,0xa9fdbc0]])
  p='/ethernet@fe2a0000';phy=dt.resolve(cells(dt.nodes[p]['phy-handle'])[0])
  self.assertEqual(phy,p+'/mdio/phy@1')
  self.assertEqual(dt.refs(p,'resets','#reset-cells'),[dict(node='/clock-controller@fdd20000',args=[215])])
  self.assertEqual(dt.regs(p),[[0xfe2a0000,65536]])
  with self.assertRaises(ValueError):dt.resolve(0xffffffff)
  with self.assertRaises(ValueError):Fdt((capture/'runtime-original.dtb').read_bytes()[:50])
 def test_generated_setup(self):
  d=json.loads((ROOT/'out/gmac-bringup/gmac0-resolved.json').read_text())
  writes=json.loads((ROOT/'out/gmac-bringup/platform-writes.json').read_text())
  self.assertEqual((d['delay']['tx'],d['delay']['rx']),(60,47))
  self.assertEqual(d['reset']['pin'],21)
  self.assertTrue(all(v['value']&~v['mask']==0 for v in writes))
  self.assertIn(dict(region=1,offset=0x380,mask=0x7f7f,value=0x2f3c,reason='captured MAC delays'),writes)
 def test_conflicting_configuration_rejected(self):
  import runpy
  original=json.loads((ROOT/'out/gmac-bringup/gmac0-resolved.json').read_text())
  for key,value in [('clock_direction','input'),('assigned_rates',[0,25000000]),('phy',dict(original['phy'],mode='rgmii-id'))]:
   with self.subTest(key=key),tempfile.TemporaryDirectory() as temp:
    root=Path(temp);(root/'out/gmac-bringup').mkdir(parents=True)
    (root/'out/gmac-bringup/gmac0-resolved.json').write_text(json.dumps(dict(original,**{key:value})))
    ns=runpy.run_path(str(ROOT/'scripts/generate-gmac-config'));ns['main'].__globals__['ROOT']=root
    with self.assertRaises(ValueError):ns['main']()
 def test_dma_qualification_and_runtime_unchanged(self):
  if not (ROOT/'out/gmac-bringup/gmac-link/sel4.elf').exists():self.skipTest('Build native link image first')
  dma=json.loads((ROOT/'out/gmac-bringup/dma-map.json').read_text())
  self.assertFalse(dma['dma_enabled']);self.assertEqual(dma['allocated_areas'],[])
  self.assertEqual(dma['current_descriptor_stride'],16);self.assertEqual(dma['descriptor_stride_required'],64)
  from policy import sha256
  for name in ('loader','sel4','monitor','initialiser'):
   self.assertEqual(sha256(ROOT/f'out/gmac-bringup/gmac-link/{name}.elf'),sha256(ROOT/f'out/native-net/runtime-baseline-v2/{name}.elf'))
 def test_driver_compiled_sequence(self):
  with tempfile.TemporaryDirectory() as temp:
   binary=Path(temp)/'test'
   subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-fsanitize=undefined,address','-I',ROOT/'drivers/gmac0',ROOT/'tests/gmac_link_test.c',ROOT/'drivers/gmac0/link.c',ROOT/'drivers/gmac0/mdio.c','-o',binary],check=True)
   subprocess.run([binary],check=True)
 def test_fresh_link_and_mapping_contract(self):
  with tempfile.TemporaryDirectory() as temp:
   p=Path(temp)/'health.rs';binary=Path(temp)/'test'
   p.write_text(f'''#[path="{ROOT}/pds/gmac_supervisor/src/link_health.rs"] mod health;
#[test] fn states() {{
let mut h=health::Health::new(1000);assert_eq!(h.check(None,1000),Ok(false));
let mut v=[2,1,0,0,1000,0,0,0,0,0,0];assert_eq!(h.check(Some(v),1001),Ok(false));
v=[4,1,2,0,1050,1050,1000,1,0,0,0];assert_eq!(h.check(Some(v),1050),Ok(true));
assert_eq!(h.check(None,1051),Ok(true));
v[0]=6;v[2]=1;v[4]=1100;v[5]=1100;v[6]=0;v[7]=0;assert_eq!(h.check(Some(v),1100),Ok(false));
v=[8,1,2,0,1150,1150,100,1,0,0,0];assert_eq!(h.check(Some(v),1150),Ok(true));
assert_eq!(h.check(None,1251),Err(6));
v[0]=10;v[4]=1800;assert_eq!(h.check(Some(v),1800),Err(5));
v[0]=12;v[2]=3;assert_eq!(h.check(Some(v),1801),Err(4));
}}
''')
   subprocess.run(['rustc','--edition=2024','--test',p,'-o',binary],check=True,capture_output=True)
   subprocess.run([binary],check=True,capture_output=True)
 def test_cache_page_bounds_and_errors(self):
  with tempfile.TemporaryDirectory() as temp:
   p=Path(temp)/'cache.c';binary=Path(temp)/'test'
   p.write_text('''#include <assert.h>
#include "cache.h"
static unsigned calls,fail;
int flush(unsigned op,uintptr_t start,uintptr_t end) {assert(op<=2);assert(end>start);assert(start/4096==(end-1)/4096);calls++;return fail?7:0;}
int main(void) {assert(gmac_cache_range(0x40000000,8192,0x40000fc0,128,0)==0);assert(calls==2);
assert(gmac_cache_range(0x40000000,8192,0x40000001,64,0)==-1);
assert(gmac_cache_range(0x40000000,8192,0x40001fc0,128,0)==-1);
fail=1;calls=0;assert(gmac_cache_range(0x40000000,8192,0x40000fc0,128,2)==7);assert(calls==1);return 0;}
''')
   subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-DR5S_CACHE_TEST','-I',ROOT/'drivers/gmac0',p,ROOT/'drivers/gmac0/cache.c','-o',binary],check=True)
   subprocess.run([binary],check=True)
if __name__=='__main__':unittest.main()
