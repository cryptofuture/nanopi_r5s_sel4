#![no_std]
#![no_main]
use core::sync::atomic::{AtomicU32, Ordering};
use sel4_microkit::{protection_domain, Handler, Infallible};
#[protection_domain]
fn init()->impl Handler {
    let heartbeat=unsafe {&*(0x40001004 as *const AtomicU32)};
    let owner=unsafe {&*(0x40001008 as *const AtomicU32)};
    while owner.load(Ordering::Acquire)!=1 {core::hint::spin_loop();}
    loop {heartbeat.fetch_add(1,Ordering::Release);core::hint::spin_loop();}
    #[allow(unreachable_code)] Idle
}
struct Idle;
impl Handler for Idle {type Error=Infallible;}
