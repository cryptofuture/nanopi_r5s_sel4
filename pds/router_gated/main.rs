#![no_std]
#![no_main]

mod shared;
mod queue;

use core::sync::atomic::{AtomicU32, Ordering};
use r5s_router_core::{ethernet::EthernetRouter, Drop, Port, Wan};
use sel4_microkit::{protection_domain, Handler, Infallible};

#[protection_domain]
fn init() -> impl Handler {
    let ticks: u64;
    unsafe { core::arch::asm!("isb", "mrs {},cntvct_el0", out(reg) ticks, options(nostack)); }
    let mut router = EthernetRouter::new((ticks ^ (ticks >> 32)) as u32);
    let mut previous = None;
    let mut epoch = None;
    let mut input = [0; 1514];
    let mut output = [0; 1514];
    let mut last = u64::MAX;
    let status = unsafe { &*(0x40005000 as *const [AtomicU32; 7]) };
    let mut drops = 0u32;
    let mut invalid = 0u32;
    let mut no_egress = 0u32;
    let mut fault = false;
    loop {
        let sample = shared::read::<25>(0x40004000);
        let driver = shared::read::<24>(0x40002000);
        let now = shared::millis();
        if now == last { core::hint::spin_loop(); continue; }
        last = now;
        if let (Some(app), Some(d)) = (sample, driver) {
            let age = (now as u32).wrapping_sub(app[2]);
            let usable = app[1] == 1 && app[7] == 0 && age <= 100
                && app[24] & 8 != 0 && app[5] == d[11] && app[8] == d[19]
                && d[1] == 1 && d[2] == 2 && d[3] == 0 && d[14] == 1
                && (now as u32).wrapping_sub(d[4]) <= 100 && app[17] > age;
            let lease = usable.then(|| Wan {
                address: app[10], mask: app[11], gateway: app[12],
                dns: [app[13], app[14], app[15]], dns_count: app[16] as usize,
                expires: now.saturating_add(u64::from(app[17] - age)),
            });
            if epoch != Some(d[11]) {
                router.link_down(Port::Wan, now);
                epoch = Some(d[11]);
                previous = None;
            }
            if previous != lease {
                if router.configure_wan(lease, now).is_err() { fault = true; }
                previous = lease;
            }
            for _ in 0..4 {
                match queue::pop(0x62000000, d[11], &mut input) {
                    Ok(0) => break,
                    Ok(n) => {
                        if !fault && usable {
                            match router.receive(Port::Wan, &input[..n], now, &mut output) {
                                Err(Drop::Policy | Drop::Spoof) => drops = drops.saturating_add(1),
                                Err(_) => invalid = invalid.saturating_add(1),
                                Ok(Some(_)) => no_egress = no_egress.saturating_add(1),
                                Ok(None) => {}
                            }
                        } else { no_egress = no_egress.saturating_add(1); }
                    }
                    Err(()) => { fault = true; break; }
                }
            }
        }
        status[0].fetch_add(1, Ordering::AcqRel);
        status[1].store(if fault { 2 } else { 1 }, Ordering::Relaxed);
        status[2].store(now as u32, Ordering::Relaxed);
        status[3].store(drops, Ordering::Relaxed);
        status[4].store(invalid, Ordering::Relaxed);
        status[5].store(no_egress, Ordering::Relaxed);
        status[6].store(previous.map_or(0,|w|w.address), Ordering::Relaxed);
        status[0].fetch_add(1, Ordering::Release);
    }
    #[allow(unreachable_code)] Idle
}
struct Idle;
impl Handler for Idle { type Error = Infallible; }
