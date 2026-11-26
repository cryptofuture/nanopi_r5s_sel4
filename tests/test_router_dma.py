import hashlib,json,runpy,subprocess,sys,tarfile,tempfile,unittest,xml.etree.ElementTree as ET
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'scripts'))
class NativeDma(unittest.TestCase):
 def test_actual_c_with_separate_device_visibility(self):
  with tempfile.TemporaryDirectory() as td:
   binary=Path(td)/'dma'
   subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-fsanitize=undefined','-I'+str(ROOT/'drivers/gmac0/dma'),str(ROOT/'tests/gmac_dma_test.c'),str(ROOT/'drivers/gmac0/dma/dma.c'),'-o',str(binary)],check=True)
   subprocess.run([binary],check=True,timeout=10)
 def test_gmac_tso_descriptors_and_fallback_gate(self):
  with tempfile.TemporaryDirectory() as td:
   binary=Path(td)/'tso'
   subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-fsanitize=undefined','-DR5S_WAN_TSO',
                   '-I'+str(ROOT/'drivers/gmac0/dma'),str(ROOT/'tests/gmac_tso_test.c'),
                   str(ROOT/'drivers/gmac0/dma/dma.c'),'-o',str(binary)],check=True)
   subprocess.run([binary],check=True,timeout=10)
   gated=Path(td)/'nogate'
   subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-c','-I'+str(ROOT/'drivers/gmac0/dma'),
                   str(ROOT/'drivers/gmac0/dma/dma.c'),'-o',str(gated)],check=True)
 def test_profiles_and_pool_ownership(self):
  from image import verify
  from dma_profile import validate_dma_manifest
  base=ROOT/'out/gmac-rawio'
  for profile,enabled in [('gmac-dma-preflight',False),('gmac-rawio',True)]:
   m=verify(base/f'r5s-{profile}.img');self.assertEqual(m['dma_enabled'],enabled)
   with self.assertRaises(ValueError):validate_dma_manifest(dict(m,dma_enabled=not enabled))
   xml=ET.parse(base/profile/'system.system').getroot();owners={}
   for pd in xml.findall('protection_domain'):
    names={x.attrib['mr'] for x in pd.findall('map')};owners[pd.attrib['name']]=names
   self.assertIn('dma_pool',owners['gmac']);self.assertNotIn('gpio4',owners['gmac'])
   if enabled:self.assertTrue(owners['probe'].isdisjoint({'dma_pool','gmac','gpio4','cru','grf'}))
   cap=json.loads((base/'layout-report.json').read_text())[profile]['capdl_objects']
   for address in range(0x24000000,0x24040000,4096):self.assertEqual(sum(o['physical']==address for o in cap),1)
   elf=base/profile/'driver.elf';symbols=subprocess.check_output(['readelf','-sW',elf],text=True)
   self.assertEqual(' dma_start' in symbols,enabled)
   self.assertIn(' dma_cache' if enabled else 'cache8maintain',symbols)
 def test_source_identity(self):
  original=json.loads((ROOT/'out/gmac-rawio/source-hashes.json').read_text())
  with tarfile.open(ROOT/'out/router/r5s-router-handoff.tar.gz') as archive:
   for p,h in original.items():
    if p in ('drivers/gmac0/dma/dma.c','drivers/gmac0/dma/dma.h'):
     with archive.extractfile('router/sources/'+p) as source:
      self.assertEqual(hashlib.file_digest(source,'sha256').hexdigest(),h,p)
    else:
     self.assertEqual(hashlib.sha256((ROOT/p).read_bytes()).hexdigest(),h,p)
  latest=json.loads((ROOT/'out/network-dma-restart/rawio/source-hashes.json').read_text())
  path='drivers/gmac0/dma/dma.c'
  with tempfile.TemporaryDirectory() as td, tarfile.open(ROOT/'out/router/r5s-router-handoff.tar.gz') as archive:
   source=Path(td)/'dma.c'
   source.write_bytes(archive.extractfile('router/sources/'+path).read())
   subprocess.run(['patch','--fuzz=0',source,'-i',ROOT/'patches/gmac-dma-restart.patch'],check=True)
   self.assertEqual(hashlib.sha256(source.read_bytes()).hexdigest(),latest[path])
 def test_config_rejects_role_and_network_changes(self):
  from router_config import validate
  c=json.loads((ROOT/'config/router-v1.json').read_text());validate(c)
  for key,value in [('version',2),('policy','accept-all')]:
   with self.assertRaises(ValueError):validate(dict(c,**{key:value}))
  for dns in [['127.0.0.1'],['0.0.0.1'],[1],['0.0.0.0'],['10.44.1.1'],['224.0.0.1']]:
   with self.assertRaises(ValueError):validate(dict(c,dns_override=dns))
  validate(dict(c,dns_override=['10.0.1.1']))
 def test_wan_profile_not_router_or_reflector(self):
  from wan_profile import validate_wan_manifest
  from image import verify
  image=ROOT/'out/router/r5s-wan-dhcp.img'
  if not image.exists():self.skipTest('WAN build pending')
  m=verify(image);validate_wan_manifest(m)
  with self.assertRaises(ValueError):validate_wan_manifest(dict(m,services=m['services']+['wan-reflector']))
  with self.assertRaises(ValueError):validate_wan_manifest(dict(m,profile='router-dual-lan'))
 def test_passive_wan_observer_requires_fresh_exchange(self):
  ns=runpy.run_path(str(ROOT/'tools/wan-observer'));inspect=ns['inspect']
  board=bytes.fromhex('025235530001');server=bytes.fromhex('020000000002')
  report=dict(client_messages=0,ack=None,echo_requests=0,matched_gateway_replies=0);pending={}
  def frame(data,proto,src,dst,source=server,destination=board):
   def check(data):
    if len(data)%2:data+=b'\0'
    s=sum(int.from_bytes(data[n:n+2],'big') for n in range(0,len(data),2))
    while s>>16:s=(s&65535)+(s>>16)
    return (65535-s).to_bytes(2,'big')
   ip=bytearray(20);ip[0]=0x45;ip[2:4]=(20+len(data)).to_bytes(2,'big');ip[8]=64;ip[9]=proto;ip[12:16]=src;ip[16:20]=dst;ip[10:12]=check(ip)
   return destination+source+b'\x08\0'+ip+data
  def dhcp(op,kind):
   b=bytearray(240);b[:3]=bytes([op,1,6]);b[4:8]=b'1234';b[16:20]=bytes([10,0,1,50]);b[28:34]=board;b[236:240]=bytes([99,130,83,99]);b+=bytes([53,1,kind,1,4,255,255,255,0,3,4,10,0,1,1,51,4,0,0,14,16,54,4,10,0,1,1,6,4,10,0,1,1,255]);udp=bytearray(8);udp[:4]=bytes([0,68,0,67] if op==1 else [0,67,0,68]);udp[4:6]=(8+len(b)).to_bytes(2,'big');return udp+b
  ack=frame(dhcp(2,5),17,bytes([10,0,1,1]),bytes([255]*4))
  inspect(ack,board,report,pending);self.assertIsNone(report['ack'])
  inspect(frame(dhcp(1,1),17,bytes(4),bytes([255]*4),board,bytes([255]*6)),board,report,pending)
  inspect(ack,board,report,pending);self.assertEqual(report['ack']['gateway'],'10.0.1.1')
  self.assertEqual(report['matched_gateway_replies'],0)
  def echo(kind):
   data=bytearray(24);data[0]=kind;data[4:6]=b'RB';data[8:16]=b'fresh123';total=sum(int.from_bytes(data[n:n+2],'big') for n in range(0,24,2))
   while total>>16:total=(total&65535)+(total>>16)
   data[2:4]=(65535-total).to_bytes(2,'big');return data
  reply=frame(echo(0),1,bytes([10,0,1,1]),bytes([10,0,1,50]))
  inspect(reply,board,report,pending);self.assertEqual(report['matched_gateway_replies'],0)
  inspect(frame(echo(8),1,bytes([10,0,1,50]),bytes([10,0,1,1]),board,server),board,report,pending)
  inspect(reply,board,report,pending);inspect(reply,board,report,pending);self.assertEqual(report['matched_gateway_replies'],1)
  malformed=bytearray(ack);malformed[14]=0x4f;inspect(malformed,board,report,pending);self.assertEqual(report['client_messages'],1)
