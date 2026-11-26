use core::sync::atomic::{AtomicU32,Ordering,fence};
pub const N:usize=40;
pub const ABI:u32=2;
pub const PROFILE:u32=0x50524632;
pub const BOOT:u32=1;pub const CACHE:u32=2;pub const LINK:u32=3;pub const MONITOR:u32=4;pub const FAILED:u32=5;
pub const CACHE_MS:u32=3000;pub const LINK_MS:u32=1500;pub const EXEC_MS:u32=100;pub const PHY_MS:u32=500;
pub fn deadline(phase:u32)->Option<u32> {match phase {BOOT=>Some(EXEC_MS),CACHE=>Some(CACHE_MS),LINK=>Some(LINK_MS),_=>None}}
pub const CLOCK:u32=1;pub const GEOMETRY:u32=2;pub const CACHE_CALL:u32=3;pub const READBACK:u32=4;pub const LINK_INIT:u32=5;pub const LINK_POLL:u32=6;pub const INACTIVE:u32=7;pub const WRITE_GUARD:u32=8;
#[derive(Clone,Copy,Debug,PartialEq)]
pub enum Mode {Wait,Ready,Fault}
pub fn read(wire:&[AtomicU32;N])->Option<[u32;N]> {
 for _ in 0..2 {
  let seq=wire[0].load(Ordering::Acquire);if seq==0 || seq&1!=0 {continue;}
  let mut value=[0;N];for i in 1..N {value[i]=wire[i].load(Ordering::Relaxed);}fence(Ordering::Acquire);
  if wire[0].load(Ordering::Acquire)==seq {value[0]=seq;return Some(value);}
 }None
}
// Sample time after publication to avoid preemption-induced future timestamps.
pub fn observe(wire:&[AtomicU32;N],clock:impl FnOnce()->u32)->(Option<[u32;N]>,u32) {
 let snapshot=read(wire);let time=clock();(snapshot,time)
}
pub fn publish(wire:&[AtomicU32;N],value:&[u32;N]) {
 wire[0].fetch_add(1,Ordering::AcqRel);for i in 1..N {wire[i].store(value[i],Ordering::Relaxed);}wire[0].fetch_add(1,Ordering::Release);
}
pub struct Producer {data:[AtomicU32;N],last:AtomicU32}
impl Producer {
 pub const fn new()->Self {Self {data:[const {AtomicU32::new(0)};N],last:AtomicU32::new(u32::MAX)}}
 pub fn set(&self,index:usize,value:u32) {self.data[index].store(value,Ordering::Relaxed);}
 pub fn get(&self,index:usize)->u32 {self.data[index].load(Ordering::Relaxed)}
 pub fn begin(&self,now:u32,id:[u32;4]) {
  self.set(1,ABI);self.set(2,PROFILE);self.set(3,now|1);for i in 0..4 {self.set(23+i,id[i]);}self.phase(BOOT,now);
 }
 pub fn phase(&self,phase:u32,now:u32) {self.set(4,phase);self.set(5,now);}
 pub fn sample(&self,now:u32,up:bool,speed:u32,duplex:u32) {self.set(8,now);self.set(9,if up {2}else{1});self.set(10,speed);self.set(11,duplex);}
 pub fn fail(&self,now:u32,reason:u32,syscall:u32,op:u32,start:u64,end:u64,observed:u32,expected:u32) {
  if self.get(14)!=0 {return;}
  self.set(15,syscall);self.set(16,op);self.set(17,start as u32);self.set(18,end as u32);self.set(19,self.get(4));self.set(20,now);self.set(21,self.get(7));self.set(22,self.get(9));self.set(27,(start>>32) as u32);self.set(28,(end>>32) as u32);self.set(29,if reason==9 {4}else if self.get(4)==CACHE {2}else if self.get(4)>=LINK {3}else{1});self.set(30,observed);self.set(31,expected);self.set(32,self.get(5));self.set(36,self.get(33));self.set(37,self.get(34));self.set(38,self.get(35));self.set(14,reason);self.phase(FAILED,now);
 }
 pub fn tick(&self,now:u32,wire:&[AtomicU32;N],force:bool) {
  if !force && self.last.load(Ordering::Relaxed)==now {return;}self.last.store(now,Ordering::Relaxed);self.set(6,now);
  let mut value=[0;N];for i in 1..N {value[i]=self.get(i);}publish(wire,&value);
 }
 pub fn snapshot(&self)->[u32;N] {let mut v=[0;N];for i in 1..N {v[i]=self.get(i);}v}
}
pub struct Health {start:u32,last_seq:u32,last_change:u32,epoch:u32,phase:u32,phase_start:u32,cached:Option<[u32;N]>,pub first:u32,pub first_time:u32}
impl Health {
 pub fn new(now:u32)->Self {Self {start:now,last_seq:0,last_change:now,epoch:0,phase:0,phase_start:now,cached:None,first:0,first_time:0}}
 fn fault(&mut self,now:u32,reason:u32)->Mode {if self.first==0 {self.first=reason;self.first_time=now;}Mode::Fault}
 pub fn check(&mut self,sample:Option<[u32;N]>,now:u32,id:[u32;4])->Mode {
  if self.first!=0 {return Mode::Fault;}
  if let Some(v)=sample {
   if v[0]==0 || v[0]&1!=0 || v[1]!=ABI || v[2]!=PROFILE || v[23..27]!=id || v[3]==0 {return self.fault(now,101);}
   if self.epoch!=0 && self.epoch!=v[3] {return self.fault(now,107);}self.epoch=v[3];
   if self.last_seq!=v[0] {
    if self.last_seq!=0 && (v[0].wrapping_sub(self.last_seq) as i32)<=0 {return self.fault(now,101);}
    self.last_seq=v[0];self.last_change=now;
   }
   if v[4]<BOOT || v[4]>FAILED || v[4]<self.phase {return self.fault(now,104);}
   if v[4]==self.phase && v[5]!=self.phase_start {return self.fault(now,108);}
   if v[4]!=self.phase {if now.wrapping_sub(v[5])>=0x80000000 {return self.fault(now,104);}self.phase=v[4];self.phase_start=v[5];}
   self.cached=Some(v);
  }
  let Some(v)=self.cached else {return if now.wrapping_sub(self.start)>EXEC_MS {self.fault(now,102)}else{Mode::Wait}};
  if v[14]!=0 || v[4]==FAILED {return self.fault(now,1000+v[14]);}
  if now.wrapping_sub(v[6])>EXEC_MS || now.wrapping_sub(self.last_change)>EXEC_MS {return self.fault(now,103);}
  let elapsed=now.wrapping_sub(v[5]);
  match v[4] {
   BOOT if elapsed>EXEC_MS=>return self.fault(now,104),
   CACHE if elapsed>CACHE_MS=>return self.fault(now,104),
   LINK if elapsed>LINK_MS=>return self.fault(now,104),
   MONITOR=>{
    if v[12]!=1 || v[13]!=1 || v[7]!=64 {return self.fault(now,104);}
    if v[9]==0 {return if elapsed>PHY_MS {self.fault(now,105)}else{Mode::Wait};}
    if now.wrapping_sub(v[8])>PHY_MS {return self.fault(now,105);}
    if v[9]==1 {if v[10]!=0 || v[11]!=0 {return self.fault(now,106);}return if self.first==0 {Mode::Wait}else{Mode::Fault};}
    if v[9]!=2 || ![10,100,1000].contains(&v[10]) || v[11]>1 || v[10]==1000 && v[11]!=1 {return self.fault(now,106);}
    return if self.first==0 {Mode::Ready}else{Mode::Fault};
   },_=>{}
  }
  if self.first!=0 {Mode::Fault}else{Mode::Wait}
 }
}
