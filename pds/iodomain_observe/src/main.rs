#![no_std]
#![no_main]
mod contract;mod identity;mod report;mod registry;
#[cfg(feature="supervisor")] mod fault_display;
#[cfg(feature="supervisor")] mod display;
#[cfg(feature="supervisor")] mod supervisor;
#[cfg(not(feature="supervisor"))] mod exercise;
#[cfg(not(feature="supervisor"))] mod driver;
fn now()->u32 {(us()/1000) as u32}
fn us()->u64 {let ticks:u64;unsafe {core::arch::asm!("isb","mrs {},cntvct_el0",out(reg)ticks,options(nostack));}ticks/24}
fn status()->&'static [core::sync::atomic::AtomicU32;contract::N] {unsafe {&*(0x40002000 as *const _)}}

#[cfg(feature="supervisor")] mod observation;
