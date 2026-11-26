use core::sync::atomic::{AtomicU32,Ordering};
use sel4_microkit::{protection_domain,Handler,Infallible};
use crate::{contract::{self,Health,Mode},display,identity::ID,now,status};
fn word(offset:usize)->&'static AtomicU32 {unsafe {&*((0x40001000+offset) as *const AtomicU32)}}
fn led(on:bool) {unsafe {core::ptr::write_volatile(0x40000004 as *mut u32,0x04000000|if on {0x400}else{0});core::arch::asm!("dsb sy",options(nostack));}}
#[protection_domain]
fn init()->impl Handler {
 while word(0).load(Ordering::Acquire)!=1 {core::hint::spin_loop();}
 unsafe {core::ptr::write_volatile(0x4000000c as *mut u32,0x04000400);}
 let start=now();let mut health=Health::new(start);let mut display=display::Display::new(crate::us()/1000);
 let mut fault_display=crate::fault_display::FaultDisplay::new();
 let mut observation=crate::observation::Observation::new();let mut optical_code=None;
 let mut monitor_pending=None;
 let mut last_poll=start;let mut last_beat=start;let mut beat=word(4).load(Ordering::Acquire);let mut seen=false;
 let frequency:u64;unsafe {core::arch::asm!("mrs {},cntfrq_el0",out(reg)frequency,options(nostack));}
 let mut effect=if frequency==24000000 {0}else{109};let mut effect_time=start;
 word(12).store(1,Ordering::Release);word(8).store(1,Ordering::Release);
 loop {
  let (snapshot,time)=contract::observe(status(),now);let mut mode=health.check(snapshot,time,ID);
  let reason=if time.wrapping_sub(last_poll)>10 {111}else if frequency!=24000000 {109}else{0};last_poll=time;
  let count=word(4).load(Ordering::Acquire);if count!=beat {beat=count;last_beat=time;seen=true;}
  let reason=if reason!=0 {reason}else if time.wrapping_sub(last_beat)>100 {110}else{0};
  if effect==0 && reason!=0 {effect=reason;effect_time=time;}
  if effect==0 && health.first!=0 {effect=health.first;effect_time=health.first_time;}
  let mailbox=unsafe {&*(0x40001200 as *const [AtomicU32;crate::report::WORDS])};
  let monitor=crate::report::read(mailbox);
  if monitor.is_none() && mailbox[0].load(Ordering::Acquire)!=0 {
   let begun=*monitor_pending.get_or_insert(time);
   if effect==0 && time.wrapping_sub(begun)>100 {effect=121;effect_time=time;}
  }else{monitor_pending=None;}
  if effect==0 && monitor.is_some() {effect=120;effect_time=time;}
  if effect!=0 {mode=Mode::Fault;}else if !seen {mode=Mode::Wait;}
  if word(128).load(Ordering::Acquire)==0 && effect!=0 {
   word(132).store(effect_time,Ordering::Relaxed);word(136).store(health.first,Ordering::Relaxed);word(140).store(last_beat,Ordering::Relaxed);
   for i in 0..4 {word(144+i*4).store(ID[i],Ordering::Relaxed);}
   word(160).store(contract::PROFILE,Ordering::Relaxed);word(164).store(4,Ordering::Relaxed);
   for (offset,index) in [(168,4),(172,7),(176,6),(180,9),(184,15)] {word(offset).store(snapshot.map_or(0,|s|s[index]),Ordering::Relaxed);}
   word(128).store(effect,Ordering::Release);
  }
  let state=match mode {Mode::Wait=>display::State::Wait,Mode::Ready=>display::State::Ready,Mode::Fault=>display::State::Fault};
  word(16).store(match mode {Mode::Wait=>0,Mode::Ready=>1,Mode::Fault=>3},Ordering::Release);word(20).store(effect,Ordering::Release);word(24).store(time,Ordering::Release);
  let optical_time=crate::us()/1000;
  let on=if mode==Mode::Fault {
   let code=*optical_code.get_or_insert_with(||crate::report::select(snapshot,monitor,effect,ID));
   fault_display.latch(code,optical_time);if code==43 {observation.tick(optical_time).unwrap_or_else(||fault_display.tick(optical_time))}else{fault_display.tick(optical_time)}
  }else{display.tick(state,optical_time)};
  led(on);core::hint::spin_loop();
 }
 #[allow(unreachable_code)] Idle
}
struct Idle;impl Handler for Idle {type Error=Infallible;}
