#![no_std]
#![no_main]

mod fault_display;

use core::sync::atomic::{AtomicU32, Ordering};
use sel4_microkit::{Handler, Infallible, protection_domain};

fn read(address: usize) -> u32 {
    unsafe { (&*(address as *const AtomicU32)).load(Ordering::Acquire) }
}

fn led(on: bool) {
    unsafe {
        core::ptr::write_volatile(0x40000004 as *mut u32, 0x04000000 | (u32::from(on) << 10));
        core::arch::asm!("dsb sy", options(nostack));
    }
}

fn millis() -> u64 {
    let count: u64;
    unsafe { core::arch::asm!("isb; mrs {}, cntvct_el0", out(reg) count, options(nostack)); }
    count / 24000
}

#[protection_domain]
fn init() -> impl Handler {
    while read(0x40001000) != 1 {
        core::hint::spin_loop();
    }
    unsafe { core::ptr::write_volatile(0x4000000c as *mut u32, 0x04000400); }
    let start = millis();
    let reports = loop {
        let time = millis() - start;
        let shared = read(0x41000000);
        let lan = [read(0x41000004), read(0x41000008)];
        if (65..=75).contains(&shared) {
            break [shared as u8; 2];
        }
        if (shared == 1 && lan.iter().all(|&code| code != 0)) || time >= 8000 {
            break core::array::from_fn(|port| match lan[port] {
                code @ 65..=75 => (code + 11 * (1 + port as u32)) as u8,
                code @ 148..=151 => {
                    let enabled = read(0x4100000c + port * 4) >> 31;
                    (100 + 8 * port as u32 + 2 * (code - 148) + enabled) as u8
                }
                _ => 98 + port as u8,
            });
        }
        led(time % 4000 < 2000);
    };
    let start = millis();
    let mut displays = [fault_display::FaultDisplay::new(), fault_display::FaultDisplay::new()];
    for (display, id) in displays.iter_mut().zip(reports) {
        display.latch(id, 0);
    }
    loop {
        let time = (millis() - start) % 170000;
        // Show each port's record twice before moving to the next.
        led(displays[(time / 85000) as usize].tick(time % 85000));
    }
    #[allow(unreachable_code)]
    Idle
}

struct Idle;
impl Handler for Idle {
    type Error = Infallible;
}
