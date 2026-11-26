#![no_std]
#![no_main]
#[cfg(not(feature="plain-leds"))] mod fault_code;
#[cfg(not(feature="plain-leds"))] mod fault_display;
mod display;mod health;mod shared;mod policy;
use core::sync::atomic::{AtomicU32,Ordering};
use sel4_microkit::{protection_domain,Handler,Infallible};
fn word(offset:usize)->&'static AtomicU32 {unsafe {&*((0x40001000+offset) as *const AtomicU32)}}
fn led(on:bool) {unsafe {core::ptr::write_volatile(0x40000004 as *mut u32,0x04000000|if on {0x400}else{0});core::arch::asm!("dsb sy",options(nostack));}}
#[protection_domain]
fn init()->impl Handler {
 while word(0).load(Ordering::Acquire)!=1 {core::hint::spin_loop();}
 unsafe {core::ptr::write_volatile(0x4000000c as *mut u32,0x04000400);}
 let start=shared::millis();let mut display=display::Display::new(start);let mut health=health::Health::new(start as u32);
 #[cfg(not(feature="plain-leds"))]
 let mut optical= fault_display::FaultDisplay::new();
 let mut app_health=health::AppFreshness::new(start as u32);
 let mut last_poll=start;let mut last_beat=start;let mut beat=word(4).load(Ordering::Acquire);let mut seen=false;
 let mut frequency:u64;unsafe {core::arch::asm!("mrs {},cntfrq_el0",out(reg)frequency,options(nostack));}
 let mut reason=if frequency==24000000 {0}else{1};
 word(12).store(1,Ordering::Release);word(8).store(1,Ordering::Release);
 loop {
  let driver=shared::read::<24>(0x40002000);let app=shared::read::<24>(0x40004000);
  let count=word(4).load(Ordering::Acquire);let time=shared::millis();
  let mut link=match health.check(driver,time as u32) {Ok(link)=>link,Err(e)=>{if reason==0 {reason=e;}false}};
  if time.saturating_sub(last_poll)>10 && reason==0 {reason=2;}last_poll=time;
  if count!=beat {beat=count;last_beat=time;seen=true;}
  if time.saturating_sub(last_beat)>100 && reason==0 {reason=3;}
  let mut proof=None;
  if let Err(code)=app_health.check(app.as_ref(),time as u32) {if reason==0 {reason=code;}}
  match app {
   Some(a)=>{
    link &= driver.is_some_and(|d|d[11]==a[5]);
    if policy::WAN {link &= a[9]==1 && driver.is_some_and(|d|d[19]==a[8]);}
    else if link && a[4]==1 {if let Ok(age)=health::age(time as u32,a[3]) {if age<5000 {proof=Some(time.saturating_sub(age as u64));}}}
   },None=>{link=false;}
  }
  let state=if !seen && reason==0 {display::State::Wait}else{display::select(time,last_beat,link,proof,reason!=0)};
  word(16).store(match state {display::State::Wait=>0,display::State::Ready=>1,display::State::IoVerified=>2,display::State::Fault=>3},Ordering::Release);
  word(20).store(reason,Ordering::Release);word(24).store(time as u32,Ordering::Release);
  let on=if state==display::State::Fault {
   #[cfg(feature="plain-leds")]
   {let _=reason;let _=driver;display.tick(display::State::Ready,time)}
   #[cfg(not(feature="plain-leds"))]
   {optical.latch(fault_code::select(reason,driver),time);optical.tick(time)}
  }else{display.tick(state,time)};
  led(on);core::hint::spin_loop();
 }
 #[allow(unreachable_code)] Idle
}
struct Idle;impl Handler for Idle {type Error=Infallible;}
