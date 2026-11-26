import importlib.util
import json
from pathlib import Path
import random
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path[:0]=[str(ROOT/'scripts'),str(ROOT/'tools/netprobe')]
import diagnostics
import wire
from native_layout import dma_area
from native_profile import validate_native_manifest

class Diagnostics(unittest.TestCase):
    def test_real_image_and_capabilities(self):
        from image import verify
        base=ROOT/'out/native-net'
        if not (base/'r5s-runtime-baseline-v2.img').exists():self.skipTest('Build baseline first')
        manifest=verify(base/'r5s-runtime-baseline-v2.img')
        self.assertEqual(manifest['diagnostics'],diagnostics.SPEC)
        import xml.etree.ElementTree as ET
        system=ET.parse(base/'system.system').getroot()
        domains=system.findall('protection_domain')
        self.assertEqual({p.attrib['name'] for p in domains},{'heartbeat','health'})
        for p in domains:
            self.assertEqual((p.attrib['budget'],p.attrib['period']),('100','1000'))
            if p.attrib['name']=='health':self.assertEqual({m.attrib['mr'] for m in p.findall('map')},{'trace_sync'})
        self.assertEqual(len([m for m in system.findall('memory_region') if m.attrib.get('phys_addr')=='0xfe770000']),1)
        self.assertFalse(any(m.attrib.get('phys_addr')=='0xfe2a0000' for m in system.findall('memory_region')))
    def test_spec(self):
        spec=diagnostics.SPEC
        self.assertEqual(spec['version'],2)
        patterns=spec['states']
        self.assertEqual([sum(ms for _,ms in patterns[name]) for name in patterns],[4000,4000,6000,6000])
        self.assertEqual(len(set(json.dumps(v) for v in patterns.values())),4)
        self.assertEqual((ROOT/'pds/diagnostics/src/display.rs').read_text(),diagnostics.rust())
        self.assertEqual(spec['lease_ms'],5000)
    def test_rust_state_and_protocol(self):
        with tempfile.TemporaryDirectory() as temp:
            path=Path(temp)/'check.rs'
            path.write_text(f'''#[path="{ROOT}/pds/diagnostics/src/display.rs"] mod display;
#[path="{ROOT}/pds/net_probe/src/lib.rs"] mod probe;
#[test] fn transitions() {{use display::*;
assert_eq!(select(100,100,true,None,false),State::Ready);
assert_eq!(select(5000,5000,true,Some(0),false),State::Ready);
assert_eq!(select(4999,4999,true,Some(0),false),State::IoVerified);
assert_eq!(select(10,10,false,Some(1),false),State::Wait);
assert_eq!(select(102,0,true,Some(1),false),State::Fault);
assert_eq!(select(10,10,true,Some(1),true),State::Fault);
let mut d=Display::new(0);assert!(!d.tick(State::Ready,0));assert!(d.tick(State::Ready,500));
assert!(!d.tick(State::Fault,600));assert!(d.tick(State::Fault,1100));
assert!(!d.tick(State::Wait,1101));
}}
#[test] fn transaction() {{
let mut p=probe::Probe::new([7;16]);let mut b=[0u8;probe::HEADER+1400];
b[..4].copy_from_slice(b"R5SN");b[4]=1;b[5]=1;b[6..8].copy_from_slice(&1400u16.to_be_bytes());b[8..24].fill(8);b[32..48].fill(7);
let mut out=[0u8;probe::HEADER+1400];assert_eq!(p.receive(&b,0,&mut out),Ok(b.len()));
out[5]=3;assert_eq!(p.receive(&out,1,&mut b),Ok(0));assert_eq!(p.proof,Some(1));
assert!(p.receive(&out,2,&mut b).is_err());p.link_down();assert_eq!(p.proof,None);
for n in 0..probe::HEADER {{assert!(probe::parse(&out[..n]).is_err());}}
}}
''')
            subprocess.run(['rustc','--edition=2024','--test',path,'-o',Path(temp)/'test'],check=True,capture_output=True)
            subprocess.run([Path(temp)/'test'],check=True,capture_output=True)
    def test_dma_guards(self):
        a=dict(cpu_virtual=0x50000000,physical=0x22000000,device=0x22000000,size=4096,alignment=64,attributes='Normal-NC',owner='driver',provenance='test capdl allocation',bus_identity_verified=True)
        self.assertEqual(dma_area(a,[]),(a['physical'],a['physical']+4096))
        for key,value in [('attributes','Device'),('bus_identity_verified',False),('physical',0x30000001),('device',0x100000000)]:
            with self.subTest(key=key),self.assertRaises(ValueError):dma_area(dict(a,**{key:value}),[])
        with self.assertRaises(ValueError):dma_area(a,[(0x22000000,0x22001000)])
    def test_profile_fail_closed(self):
        for name in ('gmac-link','gmac-rawio'):
            with self.assertRaises(ValueError):validate_native_manifest(dict(format=4,profile=name))

class Wire(unittest.TestCase):
    def test_roundtrips_fuzz(self):
        rng=random.Random(123)
        for length in (0,1,31,255,1400):
            data=rng.randbytes(length);encoded=wire.encode(1,b'n'*16,9,b'b'*16,data)
            self.assertEqual(wire.decode(encoded)['data'],data)
            for cut in range(min(len(encoded),70)):
                with self.assertRaises(ValueError):wire.decode(encoded[:cut])
        for _ in range(1000):
            value=rng.randbytes(rng.randrange(1600))
            with self.assertRaises(ValueError):wire.decode(value)
    def test_host_filters(self):
        a=wire.mac('02:52:35:53:00:01');b=wire.mac('02:00:00:00:00:02')
        f=wire.frame(b,a,wire.encode(2,b'n'*16,2,b'b'*16,b'data'))
        self.assertEqual(wire.accept(f,0,b,a)['seq'],2)
        for frame,kind,local,remote in [(f,4,b,a),(f,0,a,b),(f[:12]+b'\x81\x00'+f[14:],0,b,a),(f+b'\0',0,b,a)]:
            with self.assertRaises(ValueError):wire.accept(frame,kind,local,remote)
    def test_limits(self):
        for data in (bytes(1401),):
            with self.assertRaises(ValueError):wire.encode(1,b'n'*16,0,b'b'*16,data)
        with self.assertRaises(ValueError):wire.mac('ff:ff:ff:ff:ff:ff')

class Mdio(unittest.TestCase):
    def test_descriptor_rings(self):
        with tempfile.TemporaryDirectory() as temp:
            binary=Path(temp)/'ring-test'
            subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-fsanitize=undefined,address','-I',ROOT/'drivers/gmac0',ROOT/'tests/gmac_ring_test.c',ROOT/'drivers/gmac0/ring.c','-o',binary],check=True)
            subprocess.run([binary],check=True)
    def test_busy_and_identity(self):
        with tempfile.TemporaryDirectory() as temp:
            source=Path(temp)/'test.c'
            source.write_text('''#include <assert.h>
#include "mdio.h"
struct fake {unsigned writes,busy;unsigned command;unsigned long long time;};
static uint32_t readreg(void *p,unsigned reg) {struct fake *f=p;return reg==0x204?0x1234:f->busy;}
static void writereg(void *p,unsigned reg,uint32_t value) {struct fake *f=p;assert(reg==0x200);f->writes++;f->command=value;}
static uint64_t timeus(void *p) {return ((struct fake*)p)->time++;}
int main(void) {struct fake f={0};struct mdio_io io={&f,readreg,writereg,timeus};uint16_t value;
assert(mdio_read(&io,1,2,1,&value)==0);assert(value==0x1234);assert(f.command==((1<<21)|(2<<16)|(1<<8)|13));
f.busy=1;assert(mdio_read(&io,1,2,1,&value)==-1);assert(f.writes==1);
assert(mdio_read(&io,32,2,1,&value)==-2);assert(mdio_read(&io,1,2,0,&value)==-2);
return 0;}
''')
            subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-I',ROOT/'drivers/gmac0',source,ROOT/'drivers/gmac0/mdio.c','-o',Path(temp)/'test'],check=True)
            subprocess.run([Path(temp)/'test'],check=True)
if __name__=='__main__':unittest.main()
