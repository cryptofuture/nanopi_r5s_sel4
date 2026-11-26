import hashlib,json,subprocess,tempfile,unittest,sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];BASE=ROOT/'out/preflight-fault-id'
class FaultIdentification(unittest.TestCase):
 def test_actual_reporting_and_waveforms(self):
  with tempfile.TemporaryDirectory() as folder:
   for name in ('fault_report','fault_display'):
    binary=Path(folder)/name
    subprocess.run(['rustc','--edition','2021','-A','dead_code','--test',str(ROOT/'tests'/f'{name}_test.rs'),'-o',binary],check=True)
    with (BASE/'logs'/f'{name}-tests.log').open('w') as log:subprocess.run([binary],check=True,stdout=log,stderr=subprocess.STDOUT)
   binary=Path(folder)/'monitor'
   subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-fsanitize=address,undefined',str(ROOT/'tests/fault_monitor_test.c'),'-o',binary],check=True)
   subprocess.run([binary],check=True)
 def test_existing_scheduling_and_phy_scenarios_on_new_contract(self):
  with tempfile.TemporaryDirectory() as folder:
   objects=[]
   for name,path in [('fixture',ROOT/'tests/preflight_link_fixture.c'),('link',BASE/'compiled-link.c'),('mdio',ROOT/'drivers/gmac0/mdio.c')]:
    obj=Path(folder)/(name+'.o');subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-I'+str(ROOT/'drivers/gmac0'),'-c',path,'-o',obj],check=True);objects.append(obj)
   binary=Path(folder)/'interactions';cmd=['rustc','--edition','2021','-A','dead_code','--test',str(ROOT/'tests/preflight_fault_id_test.rs'),'-o',binary]
   for obj in objects:cmd+=['-C','link-arg='+str(obj)]
   subprocess.run(cmd,check=True)
   with (BASE/'logs/interactions.log').open('w') as log:subprocess.run([binary,'--test-threads=1'],stdout=log,stderr=subprocess.STDOUT,check=True,timeout=60)
 def test_every_registry_entry_reaches_its_word(self):
  registry=json.loads((BASE/'fault-registry.json').read_text());self.assertLessEqual(len(registry['entries']),127)
  cases=[];words=set()
  for entry in registry['entries']:
   reason,op,result=entry['key'];i=entry['id'];word=entry['word'];self.assertNotIn(word,words);words.add(word)
   bits=[(i>>n)&1 for n in range(6,-1,-1)];bits.append(sum(bits)%2)
   self.assertEqual(word,''.join('L' if x else 'S' for x in bits[:4])+' / '+''.join('L' if x else 'S' for x in bits[4:]))
   cases.append(f'assert_eq!(registry::lookup({reason},{op},{result}),{i});')
   if reason<100:
    cases.append(f'{{let p=contract::Producer::new();p.begin(0,ID);p.phase(contract::CACHE,0);p.fail(1,{reason},{0 if result==99 else result},{0 if op==99 else op},0,0,0,0);assert_eq!(report::producer(&p.snapshot(),ID),Some({i}));}}')
   elif reason==100:cases.append(f'assert_eq!(report::select(None,None,{result},ID),{i});')
   elif reason==200:cases.append(f'{{let mut m=[0;report::WORDS];m[1]=1;m[17..21].copy_from_slice(&ID);m[21]=contract::PROFILE;m[2]={op};m[3]={result};assert_eq!(report::select(None,Some(m),103,ID),{i});}}')
  source='\n'.join(f'#[path="{ROOT}/pds/preflight_fault_id/src/{m}.rs"] mod {m};' for m in ('registry','contract','report'))+'\nconst ID:[u32;4]=[1,2,3,4];\nfn main(){'+''.join(cases)+'}'
  with tempfile.TemporaryDirectory() as folder:
   p=Path(folder)/'cases.rs';p.write_text(source);binary=Path(folder)/'cases'
   subprocess.run(['rustc','--edition','2021','-A','dead_code',p,'-o',binary],check=True);subprocess.run([binary],check=True)
 def test_image_and_bound_registry(self):
  sys.path.insert(0,str(ROOT/'scripts'));from image import verify
  image=BASE/'r5s-preflight-fault-id.img';m=verify(image)
  self.assertFalse(m['dma_enabled']);self.assertFalse(m['io_verified_allowed']);self.assertTrue(m['diagnostic_only'])
  self.assertEqual(m['registry_sha256'],hashlib.sha256((BASE/'fault-registry.json').read_bytes()).hexdigest())
  self.assertEqual(image.read_bytes(),(BASE/'r5s-preflight-fault-id.reproduced.img').read_bytes())
  for p,h in json.loads((BASE/'preserved.json').read_text()).items():self.assertEqual(hashlib.sha256((ROOT/p).read_bytes()).hexdigest(),h,p)
  output=BASE/'gmac-dma-preflight-fault-id'
  self.assertEqual((output/'system.system').read_bytes(),(ROOT/'out/preflight-repair-v2/gmac-dma-preflight-repair-v2/system.system').read_bytes())
  report=(output/'microkit-report.txt').read_text()
  for text in ("* Slot: 3\n\t\t\t-> Object: 'pud_gmac'",'pd_monitor_vaddr_0x40000000','pd_heartbeat_vaddr_0x40000000'):self.assertIn(text,report)

