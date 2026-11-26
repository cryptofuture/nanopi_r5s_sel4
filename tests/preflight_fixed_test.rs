#[path="../pds/preflight_fixed/src/contract.rs"] mod contract;
#[path="../pds/preflight_fixed/src/exercise.rs"] mod exercise;
#[path="../pds/preflight_fixed/src/display.rs"] mod display;
#[path="../pds/gmac_supervisor/src/link_health.rs"] mod old_health;
use contract::*;
use exercise::*;
use std::sync::{Mutex,atomic::{AtomicU32,Ordering}};
const ID:[u32;4]=[1,2,3,4];
unsafe extern "C" {fn fixture_reset(scenario:u32,speed:u32,duplex:u32);fn fixture_advance(units:u32);fn fixture_wall()->u64;fn fixture_settle();fn fixture_unresolved();fn fixture_torn(v:u32);fn fixture_latched();fn fixture_busy(v:u32);fn fixture_active();fn fixture_wrong_phy();fn link_init()->i32;fn link_poll(speed:*mut u32,duplex:*mut u32)->i32;}
struct Sim {p:Producer,wire:[AtomicU32;N],health:Health,display:display::Display,last:u64,ready:u32,wait:u32,fault:u32,allow_fault:bool}
static SIM:Mutex<Option<Sim>>=Mutex::new(None);
#[unsafe(no_mangle)] extern "C" fn host_schedule(time:u64) {
 let mut slot=SIM.lock().unwrap();let Some(s)=slot.as_mut() else{return};if time==s.last {return;}s.last=time;
 let mode=s.health.check(read(&s.wire),time as u32,ID);
 match mode {Mode::Ready=>s.ready+=1,Mode::Wait=>s.wait+=1,Mode::Fault=>s.fault+=1};
 let state=match mode {Mode::Ready=>display::State::Ready,Mode::Wait=>display::State::Wait,Mode::Fault=>display::State::Fault};s.display.tick(state,time);
 if !s.allow_fault {assert_ne!(mode,Mode::Fault,"time={} reason={}",time,s.health.first);}
}
#[unsafe(no_mangle)] extern "C" fn host_progress(time:u64) {if let Some(s)=SIM.lock().unwrap().as_mut(){s.p.tick(time as u32,&s.wire,false);}}
fn change(f:impl FnOnce(&Producer,u32)) {let now=unsafe {fixture_wall()} as u32;let slot=SIM.lock().unwrap();let s=slot.as_ref().unwrap();f(&s.p,now);s.p.tick(now,&s.wire,true);}
struct Memory {cpu:Vec<u32>,device:Vec<u32>,error:u32,corrupt:bool,calls:Vec<(u32,usize,usize)>}
impl Memory {fn new()->Self {Self {cpu:vec![0;SIZE/4],device:vec![0;SIZE/4],error:0,corrupt:false,calls:vec![]}}}
impl Backend for Memory {
 fn call(&mut self,op:u32,start:usize,end:usize)->u32 {
  assert_eq!(start/4096,(end-1)/4096);self.calls.push((op,start,end));if self.error!=0 {return self.error;}
  let a=(start-POOL)/4;let b=(end-POOL)/4;
  if op==0 || op==2 {self.device[a..b].copy_from_slice(&self.cpu[a..b]);}if op==1 || op==2 {self.cpu[a..b].copy_from_slice(&self.device[a..b]);}
  if self.corrupt {self.cpu[a]^=1;}
  unsafe {fixture_advance(50);}0
 }
 fn write(&mut self,address:usize,value:u32) {self.cpu[(address-POOL)/4]=value;unsafe{fixture_advance(1);}}
 fn read(&mut self,address:usize)->u32 {unsafe{fixture_advance(1);}self.cpu[(address-POOL)/4]}
 fn progress(&mut self) {host_progress(unsafe{fixture_wall()});}
}
fn snapshot(phase:u32,time:u32)->[u32;N] {let p=Producer::new();p.begin(time,ID);p.phase(phase,time);p.set(7,64);p.set(12,1);p.set(13,1);p.sample(time,true,1000,1);let wire=[const{AtomicU32::new(0)};N];p.tick(time,&wire,true);read(&wire).unwrap()}
#[test] fn integrated_cache_phy_budget_and_visual_cycles() {
 for scenario in 0..=12 {for (rate,duplex) in [(10,0),(10,1),(100,0),(100,1),(1000,1)] {
  unsafe{fixture_reset(scenario,rate,duplex);}
  *SIM.lock().unwrap()=Some(Sim{p:Producer::new(),wire:[const{AtomicU32::new(0)};N],health:Health::new(0),display:display::Display::new(0),last:u64::MAX,ready:0,wait:0,fault:0,allow_fault:false});
  unsafe{fixture_advance(match scenario%3 {0=>6000,1=>0,_=>10000});} // Model unsynchronized startup with 0, 30 or 50ms producer delay.
  change(|p,t|{p.begin(t,ID);p.phase(CACHE,t);});let mut memory=Memory::new();
  for i in 0..64 {page(&mut memory,i).unwrap();change(|p,_|p.set(7,(i+1) as u32));}
  assert!(unsafe{fixture_wall()}>1000);change(|p,t|{p.set(12,1);p.phase(LINK,t);});assert_eq!(unsafe{link_init()},0);
  change(|p,t|{p.set(13,1);p.phase(MONITOR,t);});let mut next=0;
  while unsafe{fixture_wall()}<30000 {
   let t=unsafe{fixture_wall()};host_progress(t);
   if t>=next {let mut speed=0;let mut full=0;let result=unsafe{link_poll(&mut speed,&mut full)};assert!(result>=0);change(|p,t|p.sample(t,result==1,speed,full));next=unsafe{fixture_wall()}+200;}
   unsafe{fixture_advance(100);}
  }
  let slot=SIM.lock().unwrap();let s=slot.as_ref().unwrap();assert_eq!(s.fault,0);assert!(s.wait>0);if scenario==0 {assert_eq!(s.ready,0);}else {assert!(s.ready>10000);}
 }}
 *SIM.lock().unwrap()=None;
}
#[test] fn bounded_startup_stall_and_old_unbounded_phase() {
 let mut old=old_health::Health::new(0);let mut fixed=Health::new(0);
 for t in 0..=3100u32 {let old_v=[t*2+2,1,0,0,t,0,0,0,0,0,0];assert_eq!(old.check(Some(old_v),t as u64),Ok(false));let mut v=snapshot(CACHE,0);v[0]=t*2+2;v[6]=t;let mode=fixed.check(Some(v),t,ID);assert_eq!(mode,if t<=CACHE_MS{Mode::Wait}else{Mode::Fault});}assert_eq!(fixed.first,104);
 let mut h=Health::new(0);assert_eq!(h.check(None,100,ID),Mode::Wait);assert_eq!(h.check(None,101,ID),Mode::Fault);assert_eq!(h.check(None,102,ID),Mode::Fault);
 for phase in [BOOT,CACHE,LINK,MONITOR] {let mut h=Health::new(0);h.check(Some(snapshot(phase,0)),0,ID);assert_eq!(h.check(None,101,ID),Mode::Fault);assert_eq!(h.first,103);}
 let mut h=Health::new(0);let mut v=snapshot(LINK,0);for t in 0..=1501 {v[0]=t*2+2;v[6]=t;let mode=h.check(Some(v),t,ID);if t==1501 {assert_eq!(mode,Mode::Fault);}}
}
#[test] fn status_torn_abi_epoch_timestamp_and_phase_reset() {
 let wire=[const{AtomicU32::new(0)};N];assert!(read(&wire).is_none());wire[0].store(1,Ordering::Release);assert!(read(&wire).is_none());
 for index in [0,1,2,3,4,23] {let mut v=snapshot(MONITOR,0);v[index]=0;assert_eq!(Health::new(0).check(Some(v),0,ID),Mode::Fault);}
 let concurrent=std::sync::Arc::new([const{AtomicU32::new(0)};N]);let writer=concurrent.clone();
 let thread=std::thread::spawn(move||{for value in 1..20000 {let sample=[value;N];publish(&writer,&sample);}});
 for _ in 0..100000 {if let Some(sample)=read(&concurrent) {assert!(sample[1..].iter().all(|v|*v==sample[1]));}}thread.join().unwrap();
 let mut h=Health::new(0);let v=snapshot(MONITOR,0);assert_eq!(h.check(Some(v),0,ID),Mode::Ready);let mut stale=v;stale[0]+=2;stale[3]+=2;assert_eq!(h.check(Some(stale),1,ID),Mode::Fault);assert_eq!(h.first,107);
 let mut h=Health::new(0);let mut v=snapshot(CACHE,0);h.check(Some(v),0,ID);v[0]+=2;v[5]=1;v[6]=1;assert_eq!(h.check(Some(v),1,ID),Mode::Fault);assert_eq!(h.first,108);
 let mut future=snapshot(MONITOR,1);future[6]=0;future[8]=0;assert_eq!(Health::new(0).check(Some(future),0,ID),Mode::Fault);
 let mut long=Health::new(0);let mut stable=snapshot(MONITOR,0);assert_eq!(long.check(Some(stable),0,ID),Mode::Ready);stable[0]+=2;stable[6]=3000000000;stable[8]=3000000000;assert_eq!(long.check(Some(stable),3000000000,ID),Mode::Ready);
 let mut h=Health::new(u32::MAX-50);let mut v=snapshot(MONITOR,u32::MAX-50);assert_eq!(h.check(Some(v),u32::MAX-50,ID),Mode::Ready);v[0]+=2;v[6]=20;v[8]=20;assert_eq!(h.check(Some(v),20,ID),Mode::Ready);
 let mut h=Health::new(0);let mut v=snapshot(MONITOR,0);h.check(Some(v),0,ID);v[0]+=2;v[6]=101;assert_eq!(h.check(Some(v),101,ID),Mode::Ready);v[0]+=2;v[6]=501;assert_eq!(h.check(Some(v),501,ID),Mode::Fault);assert_eq!(h.first,105);
}
#[test] fn actual_cache_algorithm_errors_bounds_and_first_record() {
 let mut memory=Memory::new();for op in 0..3 {for error in 1..=10 {memory.error=error;let result=maintain(&mut memory,op,POOL,4096).unwrap_err();assert_eq!(result.syscall,error);assert_eq!(result.op,op);assert_eq!((result.start,result.end),(POOL,POOL+4096));}}
 memory.error=2;let before=memory.calls.len();let error=page(&mut memory,0).unwrap_err();assert_eq!(error.syscall,2);assert_eq!(memory.calls.len(),before+1);
 memory.error=0;maintain(&mut memory,0,POOL+64,8192).unwrap();assert_eq!(memory.calls[memory.calls.len()-3..],[(0,POOL+64,POOL+4096),(0,POOL+4096,POOL+8192),(0,POOL+8192,POOL+8256)]);
 for (address,length) in [(POOL-64,64),(POOL,0),(POOL+1,64),(POOL,65),(POOL+SIZE,64),(POOL,usize::MAX)] {assert!(maintain(&mut memory,0,address,length).is_err());}
 memory.corrupt=true;let failure=page(&mut memory,0).unwrap_err();assert!(failure.readback);assert_eq!(failure.start,POOL);
 let p=Producer::new();p.begin(0,ID);p.phase(CACHE,1);p.set(33,0x3051);p.set(34,0x001cc916);p.set(35,125000000);p.fail(2,CACHE_CALL,2,1,POOL as u64,(POOL+4096) as u64,0,0);p.fail(200,LINK_INIT,4,0,0,0,0,0);let record=p.snapshot();assert_eq!(record[14],CACHE_CALL);assert_eq!(record[15],2);assert_eq!(record[19],CACHE);assert_eq!(record[20],2);assert_eq!(record[32],1);assert_eq!(&record[36..39],&[0x3051,0x001cc916,125000000]);
}
#[test] fn visual_policy_fault_latch_and_many_cycles() {
 for raw in 0..256 {let mut v=snapshot(MONITOR,0);v[9]=raw;let mut h=Health::new(0);let mode=h.check(Some(v),0,ID);assert!([Mode::Wait,Mode::Ready,Mode::Fault].contains(&mode));if raw>2 {assert_eq!(mode,Mode::Fault);}}
 let mut display=display::Display::new(0);for t in 0..20000 {assert_eq!(display.tick(display::State::Ready,t),t>=500 && (t-500)%4000<200);}
 for state in [display::State::Wait,display::State::Fault] {
  let mut d=display::Display::new(0);
  for t in 0..30000 {let expected=t>=500 && match state {display::State::Wait=>(t-500)%4000<2000,_=>{let x=(t-500)%6000;x<2000 && x%250<125}};assert_eq!(d.tick(state,t),expected);}
 }
 let mut h=Health::new(0);let mut invalid=snapshot(MONITOR,0);invalid[1]=0;assert_eq!(h.check(Some(invalid),0,ID),Mode::Fault);assert_eq!(h.check(Some(snapshot(MONITOR,1)),1,ID),Mode::Fault);
}
