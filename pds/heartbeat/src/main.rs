#![no_std]
#![no_main]

mod gpio;
use core::ptr::write_volatile;
use sel4_microkit::{protection_domain, Handler, Infallible};

const GPIO: usize = 0x40000000;

fn write(offset: usize, value: u32) {
    // Only the GPIO4 device page is mapped; firmware prepares its clock and mux.
    unsafe {
        write_volatile((GPIO + offset) as *mut u32, value);
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

fn delay(iterations: u32) {
    // shortcut: uncalibrated spin delay; use a verified timer after board bring-up.
    for _ in 0..iterations {
        core::hint::spin_loop();
    }
}

#[protection_domain]
fn init() -> impl Handler {
    write(0x04, gpio::masked(26, false));
    write(0x0c, gpio::masked(26, true));
    if cfg!(feature = "fault-injection") {
        panic!("deliberate failure before success pattern");
    }
    loop {
        for _ in 0..3 {
            write(0x04, gpio::masked(26, true));
            delay(100_000_000);
            write(0x04, gpio::masked(26, false));
            delay(100_000_000);
        }
        delay(1_000_000_000);
    }
    #[allow(unreachable_code)]
    Idle
}

struct Idle;
impl Handler for Idle {
    type Error = Infallible;
}
