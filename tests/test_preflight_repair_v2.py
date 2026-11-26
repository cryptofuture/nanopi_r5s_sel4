import hashlib,json,tarfile,resource,subprocess,sys,tempfile,unittest,xml.etree.ElementTree as ET
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];BASE=ROOT/'out/preflight-repair-v2'
sys.path.insert(0,str(ROOT/'scripts'))
class RepairV2Preflight(unittest.TestCase):
 def test_actual_components_and_old_phy_regression(self):
  folder=BASE/'tests';folder.mkdir(exist_ok=True)
  prefix=(ROOT/'drivers/gmac0/link.c').read_text().split('int link_poll(unsigned *speed,unsigned *duplex) {')[0]
  generated=folder/'link-fixed.c';generated.write_text(prefix+(ROOT/'pds/preflight_repair_v2/src/poll.c').read_text())
  self.assertEqual(generated.read_bytes(),(BASE/'compiled-link.c').read_bytes())
  common=['cc','-std=c11','-Wall','-Wextra','-Werror','-I'+str(ROOT/'drivers/gmac0')]
  objects=[]
  for name,p in [('fixture',ROOT/'tests/preflight_link_fixture.c'),('link',generated),('mdio',ROOT/'drivers/gmac0/mdio.c')]:
   o=folder/(name+'.o');subprocess.run(common+['-c',str(p),'-o',str(o)],check=True);objects.append(str(o))
  binary=folder/'interactions';args=['rustc','--edition','2021','--test',str(ROOT/'tests/preflight_repair_v2_test.rs'),'-o',str(binary)]
  for o in objects:args+=['-C','link-arg='+o]
  subprocess.run(args,check=True,capture_output=True,text=True)
  with (folder/'interactions-rerun.log').open('w') as output:subprocess.run([binary,'--test-threads=1'],stdout=output,stderr=subprocess.STDOUT,check=True,timeout=60)
  for name,source in [('fixed',generated),('old',ROOT/'drivers/gmac0/link.c')]:
   binary=folder/('phy-'+name);subprocess.run(common+[str(ROOT/'tests/preflight_phy_test.c'),str(ROOT/'tests/preflight_link_fixture.c'),str(source),str(ROOT/'drivers/gmac0/mdio.c'),'-o',str(binary)],check=True)
   resource.setrlimit(resource.RLIMIT_CORE,(0,0));r=subprocess.run([binary],capture_output=True,text=True)
   if name=='fixed':self.assertEqual(r.returncode,0,r.stderr)
   else:
    self.assertNotEqual(r.returncode,0)
    (folder/'old-phy-regression-rerun.log').write_text(f'Expected failure in original linked source: exit {r.returncode}\n'+r.stderr)
 def test_actual_image_capabilities_and_isolation(self):
  from image import verify
  from preflight_repair_profile import validate_fixed_manifest
  m=verify(BASE/'r5s-gmac-dma-preflight-repair-v2.img');validate_fixed_manifest(m)
  for change in [dict(dma_enabled=True),dict(io_verified_allowed=True),dict(packet_io=True),dict(status_abi=1),dict(profile='gmac-rawio')]:
   with self.assertRaises(ValueError):validate_fixed_manifest(dict(m,**change))
  output=BASE/'gmac-dma-preflight-repair-v2';root=ET.parse(output/'system.system').getroot()
  self.assertEqual({p.attrib['name'] for p in root.findall('protection_domain')},{'heartbeat','health','gmac'})
  self.assertEqual(sum(x.attrib.get('mr')=='gpio4' and x.attrib['perms']=='rw' for p in root.findall('protection_domain') for x in p.findall('map')),1)
  for p in root.findall('protection_domain'):
   maps={x.attrib['mr']:x.attrib for x in p.findall('map')}
   if p.attrib['name']=='gmac':
    self.assertEqual(maps['dma_pool']['cached'],'true');self.assertNotIn('gpio4',maps);self.assertNotIn('queues',maps)
   else:self.assertNotIn('dma_pool',maps)
  layout=json.loads((BASE/'layout-report.json').read_text())
  for address in range(0x24000000,0x24040000,4096):self.assertEqual(sum(o['physical']==address for o in layout['capdl_objects']),1)
  report=(output/'microkit-report.txt').read_text();self.assertIn("* Slot: 3\n\t\t\t-> Object: 'pud_gmac'",report);self.assertIn("-> IPC Buffer: 'frame_ipcbuf_gmac'",report)
  symbols=subprocess.check_output(['readelf','-sW',output/'driver.elf'],text=True)
  for name in ['dma_start','dma_send','dma_receive','__sel4_ipc_buffer']:self.assertFalse(any(line.split()[-1:]==[name] for line in symbols.splitlines()))
  self.assertIn('link_inactive',symbols);self.assertIn('IPC_BUFFER',symbols)
  for label,directory in [('first',ROOT/'out/preflight-fix'),('second',BASE)]:
   fingerprints=[json.loads(p.read_text()) for p in (directory/'work/fixed-driver/target').rglob('run-build-script-build-script-build.json') if 'link-config.h' in p.read_text()]
   self.assertEqual(len(fingerprints),1)
   paths=fingerprints[0]['local'][0]['RerunIfChanged']['paths']
   for header in ('link.h','mdio.h'):self.assertEqual(str(ROOT/'drivers/gmac0'/header) in paths,label=='second')
  disassembly=subprocess.check_output(['aarch64-none-elf-objdump','-d',output/'driver.elf'],text=True)
  self.assertTrue('svc' in disassembly,'Missing actual seL4 SVC instruction')
  with tarfile.open(BASE/'r5s-preflight-repair-v2-handoff.tar.gz') as archive:
   for p,h in json.loads((BASE/'source-hashes.json').read_text()).items():
    data=archive.extractfile('preflight-repair-v2/scripts/image.py').read() if p=='scripts/image.py' else (ROOT/p).read_bytes()
    self.assertEqual(hashlib.sha256(data).hexdigest(),h,p)
 def test_preserved_artifacts_and_two_assemblies(self):
  for p,h in json.loads((BASE/'preserved.json').read_text()).items():self.assertEqual(hashlib.sha256((ROOT/p).read_bytes()).hexdigest(),h,p)
  self.assertEqual((BASE/'r5s-gmac-dma-preflight-repair-v2.img').read_bytes(),(BASE/'r5s-gmac-dma-preflight-repair-v2.reproduced.img').read_bytes())
