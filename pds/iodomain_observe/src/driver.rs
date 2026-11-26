use core::sync::atomic::{AtomicU32,Ordering,compiler_fence};
use sel4_microkit::{protection_domain,Handler,Infallible};
use crate::{contract::*,exercise::{self,Backend},now,status,us,identity::ID};
static PRODUCER:Producer=Producer::new();
unsafe extern "C" {fn link_allowed(region:u32,offset:u32,value:u32)->i32;fn link_init()->i32;fn link_poll(speed:*mut u32,duplex:*mut u32)->i32;fn link_inactive()->i32;fn link_stop();static mut link_mac_version:u32;static mut link_phy_id:u32;static mut link_clock_hz:u32;}
fn operation(op:u32,result:u32,start:u64,end:u64,complete:bool) {
 let wire=unsafe {&*(0x40002100 as *const [AtomicU32;crate::report::WORDS])};
 let mut v=[0;crate::report::WORDS];v[1]=1;v[2]=PRODUCER.get(4);v[3]=op;v[4]=result;v[5]=complete as u32;v[6]=now();v[7]=PRODUCER.get(7);
 v[8]=start as u32;v[9]=(start>>32) as u32;v[10]=end as u32;v[11]=(end>>32) as u32;
 for i in 0..4 {v[12+i]=ID[i];}v[16]=PROFILE;
 crate::report::publish(wire,&v);
}
fn record(reason:u32,result:u32,op:u32,start:u64,end:u64,observed:u32,expected:u32) {
 PRODUCER.set(33,unsafe {link_mac_version});PRODUCER.set(34,unsafe {link_phy_id});PRODUCER.set(35,unsafe {link_clock_hz});
 PRODUCER.fail(now(),reason,result,op,start,end,observed,expected);PRODUCER.tick(now(),status(),true);
}
fn fail(reason:u32,result:u32,op:u32,start:u64,end:u64,observed:u32,expected:u32)->! {
 record(reason,result,op,start,end,observed,expected);
 sel4_microkit::debug_println!("preflight first phase={} reason={} result={} op={} start={:#x} end={:#x}",PRODUCER.get(19),reason,result,op,start,end);
 loop {hw_progress();core::hint::spin_loop();}
}
fn inactive_failure(result:i32)->! {
 let flags=(hw_read(0,0)&3)|((hw_read(0,0x1104)&1)<<2)|((hw_read(0,0x1108)&1)<<3);
 let tx=((hw_read(0,0x1110) as u64)<<32)|hw_read(0,0x1114) as u64;
 let rx=((hw_read(0,0x1118) as u64)<<32)|hw_read(0,0x111c) as u64;
 record(INACTIVE,result as u32,0,tx,rx,flags,0);
 unsafe {link_stop();}fail(INACTIVE,result as u32,0,tx,rx,flags,0);
}
#[unsafe(no_mangle)] extern "C" fn hw_us()->u64 {us()}
#[unsafe(no_mangle)] extern "C" fn hw_progress() {
 if PRODUCER.get(14)==0 && deadline(PRODUCER.get(4)).is_some_and(|limit|now().wrapping_sub(PRODUCER.get(5))>limit) {
  if PRODUCER.get(4)>=LINK {unsafe {link_stop();}}
  fail(10,0,0,0,0,0,0);
 }
 let effect=unsafe {&*(0x40001080 as *const AtomicU32)}.load(Ordering::Acquire);
 if effect!=0 && PRODUCER.get(14)==0 {
  if PRODUCER.get(4)>=LINK {unsafe {link_stop();}}
  fail(9,effect,0,0,0,0,0);
 }
 PRODUCER.tick(now(),status(),false);
}
fn address(region:u32,offset:u32)->usize {
 let bases=[0x50000000usize,0x50002000,0x50003000,0x50004000,0x50005000,0x50006000];
 if region>=6 || offset%4!=0 || offset>=if region==0 {8192}else{4096} {fail(WRITE_GUARD,0,region,offset as u64,0,0,0);}
 bases[region as usize]+offset as usize
}
#[unsafe(no_mangle)] extern "C" fn hw_read(region:u32,offset:u32)->u32 {unsafe {core::ptr::read_volatile(address(region,offset) as *const u32)}}
#[unsafe(no_mangle)] extern "C" fn hw_write(region:u32,offset:u32,value:u32) {
 if unsafe {link_allowed(region,offset,value)}!=1 {fail(WRITE_GUARD,0,region,offset as u64,0,value,0);}
 unsafe {core::ptr::write_volatile(address(region,offset) as *mut u32,value);core::arch::asm!("dsb sy",options(nostack));}
}
struct Native;
impl Backend for Native {
 fn call(&mut self,op:u32,start:usize,end:usize)->u32 {
  operation(op,0,start as u64,end as u64,false);
  compiler_fence(Ordering::SeqCst);
  let result=sel4::with_ipc_buffer_mut(|buffer| {
   let raw=buffer.inner_mut();match op {0=>raw.seL4_ARM_VSpace_Clean_Data(3,start as _,end as _),1=>raw.seL4_ARM_VSpace_Invalidate_Data(3,start as _,end as _),_=>raw.seL4_ARM_VSpace_CleanInvalidate_Data(3,start as _,end as _)}
  }) as u32;
  unsafe {core::arch::asm!("dsb sy",options(nostack));}compiler_fence(Ordering::SeqCst);operation(op,result,start as u64,end as u64,true);result
 }
 fn write(&mut self,address:usize,value:u32) {unsafe {core::ptr::write_volatile(address as *mut u32,value);}}
 fn read(&mut self,address:usize)->u32 {unsafe {core::ptr::read_volatile(address as *const u32)}}
 fn progress(&mut self) {hw_progress();}
}
#[protection_domain]
fn init()->impl Handler {
 while unsafe {&*(0x40001008 as *const AtomicU32)}.load(Ordering::Acquire)!=1 {core::hint::spin_loop();}
 PRODUCER.begin(now(),ID);PRODUCER.tick(now(),status(),true);
 let frequency:u64;unsafe {core::arch::asm!("mrs {},cntfrq_el0",out(reg)frequency,options(nostack));}
 if frequency!=24000000 {fail(CLOCK,0,0,frequency,24000000,0,0);}
 if exercise::POOL%4096!=0 || exercise::SIZE!=64*4096 || exercise::LINE!=64 {fail(GEOMETRY,0,0,exercise::POOL as u64,exercise::SIZE as u64,0,0);}
 PRODUCER.phase(CACHE,now());PRODUCER.tick(now(),status(),true);
 for page in 0..64 {
  if let Err(error)=exercise::page(&mut Native,page) {fail(if error.readback {READBACK}else if error.syscall==100 {GEOMETRY}else{CACHE_CALL},if error.syscall==100 {0}else{error.syscall},error.op,error.start as u64,error.end as u64,error.observed,error.expected);}
  PRODUCER.set(7,(page+1) as u32);hw_progress();
 }
 PRODUCER.set(12,1);PRODUCER.phase(LINK,now());PRODUCER.tick(now(),status(),true);
 operation(10,0,0,0,false);let result=unsafe {link_init()};operation(10,result as u32,0,0,true);
 if result==3 {inactive_failure(result);}
 if result!=0 {unsafe {link_stop();}fail(LINK_INIT,result as u32,0,0,0,0,0);}
 if unsafe {link_inactive()}!=1 {inactive_failure(0);}
 PRODUCER.set(13,1);PRODUCER.phase(MONITOR,now());PRODUCER.tick(now(),status(),true);
 let mut next=now();
 loop {
  let time=now();hw_progress();
  if time.wrapping_sub(next)<0x80000000 {
   let mut speed=0;let mut duplex=0;operation(11,0,0,0,false);let result=unsafe {link_poll(&mut speed,&mut duplex)};operation(11,result as u32,speed as u64,duplex as u64,true);
   if result== -3 {inactive_failure(result);}
   if result<0 {unsafe {link_stop();}fail(LINK_POLL,result as u32,0,0,0,0,0);}
   if unsafe {link_inactive()}!=1 {inactive_failure(0);}
   PRODUCER.sample(now(),result==1,speed,duplex);PRODUCER.tick(now(),status(),true);next=now().wrapping_add(200);
  }
  core::hint::spin_loop();
 }
 #[allow(unreachable_code)] Idle
}
struct Idle;impl Handler for Idle {type Error=Infallible;}

#[unsafe(no_mangle)] extern "C" fn io_snapshot(a:u32,b:u32,c:u32) {
 let v=unsafe {&*(0x40002300 as *const [AtomicU32;4])};
 v[1].store(a,Ordering::Relaxed);v[2].store(b,Ordering::Relaxed);v[3].store(c,Ordering::Relaxed);v[0].store(0x494f5331,Ordering::Release);
}
