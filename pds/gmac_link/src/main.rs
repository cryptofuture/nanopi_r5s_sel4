#![no_std]
#![no_main]
use core::sync::atomic::{AtomicU32,Ordering};
use sel4_microkit::{protection_domain,Handler,Infallible};
static STATE:AtomicU32=AtomicU32::new(0);
static FAULT:AtomicU32=AtomicU32::new(0);
static SPEED:AtomicU32=AtomicU32::new(0);
static DUPLEX:AtomicU32=AtomicU32::new(0);
static READ_MS:AtomicU32=AtomicU32::new(0);
static PUBLISHED_MS:AtomicU32=AtomicU32::new(u32::MAX);
unsafe extern "C" {
 fn link_allowed(region:u32,offset:u32,value:u32)->i32;
 fn link_init()->i32;
 fn link_stop();
 fn link_poll(speed:*mut u32,duplex:*mut u32)->i32;
 static link_mac_version:u32;static link_phy_id:u32;static link_clock_hz:u32;
}
fn word(offset:usize)-> &'static AtomicU32 {unsafe {&*((0x40002000+offset) as *const AtomicU32)}}
#[unsafe(no_mangle)] extern "C" fn hw_us()->u64 {let ticks:u64;unsafe {core::arch::asm!("isb","mrs {}, cntvct_el0",out(reg) ticks,options(nostack));}ticks/24}
#[unsafe(no_mangle)] extern "C" fn hw_progress() {
 let now=(hw_us()/1000) as u32;
 if PUBLISHED_MS.load(Ordering::Relaxed)==now {return;}
 PUBLISHED_MS.store(now,Ordering::Relaxed);
 word(0).fetch_add(1,Ordering::AcqRel);
 for (offset,value) in [(4,1),(8,STATE.load(Ordering::Relaxed)),(12,FAULT.load(Ordering::Relaxed)),(16,now),(20,READ_MS.load(Ordering::Relaxed)),(24,SPEED.load(Ordering::Relaxed)),(28,DUPLEX.load(Ordering::Relaxed)),(32,unsafe {link_mac_version}),(36,unsafe {link_phy_id}),(40,unsafe {link_clock_hz})] {word(offset).store(value,Ordering::Relaxed);}
 word(0).fetch_add(1,Ordering::Release);
}
fn address(region:u32,offset:u32)->usize {
 let base=[0x50000000usize,0x50002000,0x50003000,0x50004000,0x50005000,0x50006000];
 assert!(region<6 && offset%4==0 && offset<if region==0 {8192}else{4096});base[region as usize]+offset as usize
}
#[unsafe(no_mangle)] extern "C" fn hw_read(region:u32,offset:u32)->u32 {unsafe {core::ptr::read_volatile(address(region,offset) as *const u32)}}
#[unsafe(no_mangle)] extern "C" fn hw_write(region:u32,offset:u32,value:u32) {
 if unsafe {link_allowed(region,offset,value)}!=1 {FAULT.store(99,Ordering::Relaxed);STATE.store(3,Ordering::Relaxed);loop {hw_progress();}}
 unsafe {core::ptr::write_volatile(address(region,offset) as *mut u32,value);core::arch::asm!("dsb sy",options(nostack));}
}
#[protection_domain]
fn init()->impl Handler {
 while unsafe {&*(0x40001008 as *const AtomicU32)}.load(Ordering::Acquire)!=1 {core::hint::spin_loop();}
 let frequency:u64;unsafe {core::arch::asm!("mrs {}, cntfrq_el0",out(reg) frequency,options(nostack));}
 let result=if frequency==24000000 {unsafe {link_init()}} else {10};
 sel4_microkit::debug_println!("GMAC init={} version={:#x} phy={:#x} clock={} DMA=off",result,unsafe {link_mac_version},unsafe {link_phy_id},unsafe {link_clock_hz});
 if result!=0 {unsafe {link_stop();}FAULT.store(result as u32,Ordering::Relaxed);STATE.store(3,Ordering::Relaxed);}
 let mut next=0;
 let mut previous=(u32::MAX,0,0);
 loop {
  hw_progress();let now=hw_us()/1000;
  if STATE.load(Ordering::Relaxed)!=3 && now>=next {
   let mut speed=0;let mut duplex=0;let result=unsafe {link_poll(&mut speed,&mut duplex)};
   if result<0 {unsafe {link_stop();}FAULT.store((-result) as u32,Ordering::Relaxed);STATE.store(3,Ordering::Relaxed);}
   else {SPEED.store(speed,Ordering::Relaxed);DUPLEX.store(duplex,Ordering::Relaxed);READ_MS.store((hw_us()/1000) as u32,Ordering::Relaxed);STATE.store(if result==1 {2}else{1},Ordering::Relaxed);}
   let current=(STATE.load(Ordering::Relaxed),speed,duplex);
   if current!=previous {sel4_microkit::debug_println!("GMAC state={} speed={} duplex={} fault={}",current.0,speed,duplex,FAULT.load(Ordering::Relaxed));previous=current;}
   next=hw_us()/1000+200;
  }
  core::hint::spin_loop();
 }
 #[allow(unreachable_code)] Idle
}
struct Idle;impl Handler for Idle {type Error=Infallible;}
