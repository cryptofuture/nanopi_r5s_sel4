#![no_std]
#![no_main]
mod display;
mod link_health;
use core::sync::atomic::{AtomicU32, Ordering};
use sel4_microkit::{protection_domain, Handler, Infallible};
fn shared(offset:usize)-> &'static AtomicU32 {unsafe {&*((0x40001000+offset) as *const AtomicU32)}}
fn now()->u64 {let ticks:u64; unsafe {core::arch::asm!("isb", "mrs {}, cntvct_el0",out(reg) ticks,options(nostack));} ticks/24000}
fn led(on:bool) {unsafe {core::ptr::write_volatile(0x40000004 as *mut u32,0x04000000|if on {0x400}else{0});core::arch::asm!("dsb sy",options(nostack));}}
#[protection_domain]
fn init()->impl Handler {
    while shared(0).load(Ordering::Acquire)!=1 {core::hint::spin_loop();}
    unsafe {core::ptr::write_volatile(0x4000000c as *mut u32,0x04000400);}
    let mut display=display::Display::new(now());
    let mut last=now();
    let mut last_count=shared(4).load(Ordering::Acquire);
    let mut seen_health=false;
    let mut driver_health=link_health::Health::new(now());
    let mut last_poll=now();
    let mut frequency:u64;
    unsafe {core::arch::asm!("mrs {}, cntfrq_el0",out(reg) frequency,options(nostack));}
    let mut fault=frequency!=24000000;
    let mut reason=if fault {1} else {0};
    shared(12).store(1,Ordering::Release);
    shared(8).store(1,Ordering::Release);
    loop {
        let time=now();
        if time.saturating_sub(last_poll)>display::POLL_MS {fault=true;if reason==0 {reason=2;}}
        last_poll=time;
        let count=shared(4).load(Ordering::Acquire);
        if count!=last_count {last=time;last_count=count;seen_health=true;}
        if time.saturating_sub(last)>display::HEALTH_MS {fault=true;if reason==0 {reason=3;}}
        let link=match driver_health.check(link_status(),time) {
            Ok(link)=>link,
            Err(code)=>{fault=true;if reason==0 {reason=code;} false}
        };
        let state=if !seen_health && !fault {display::State::Wait} else {display::select(time,last,link,None,fault)};
        shared(16).store(match state {display::State::Wait=>0,display::State::Ready=>1,display::State::IoVerified=>2,display::State::Fault=>3},Ordering::Release);
        shared(20).store(reason,Ordering::Release);
        shared(24).store(time as u32,Ordering::Release);
        led(display.tick(state,time));
        core::hint::spin_loop();
    }
    #[allow(unreachable_code)] Idle
}
struct Idle;
impl Handler for Idle {type Error=Infallible;}

fn link_status()->Option<[u32;11]> {
    let data=unsafe {&*(0x40002000 as *const [AtomicU32;11])};
    for _ in 0..2 {
        let before=data[0].load(Ordering::Acquire);
        if before&1!=0 || before==0 {continue;}
        let mut values=[0;11];
        for i in 0..11 {values[i]=data[i].load(Ordering::Relaxed);}
        core::sync::atomic::fence(Ordering::Acquire);
        if data[0].load(Ordering::Acquire)==before {values[0]=before;return Some(values);}
    }
    None
}
