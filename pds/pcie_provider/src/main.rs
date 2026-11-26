#![no_std]
#![no_main]

#[cfg(feature = "lan1-only")]
mod arp;
#[cfg(feature = "lan1-only")]
mod b51;
#[cfg(feature = "lan-port")]
mod queue;
#[cfg(feature = "zc")]
mod zc;
#[cfg(any(feature = "tso", feature = "rx-gro"))]
mod tso;
#[cfg(feature = "tso-test")]
mod tso_testgen;
mod dma;
mod mac;
mod mmio;
mod pcie;
mod phy;
mod platform;
mod provider;
mod ring;

use core::{
    ptr::NonNull,
    sync::atomic::{AtomicU32, Ordering},
};
use dma::InboundWindow;
use mmio::{DeviceIo, Mapping};
use platform::{BringupError, R5sPcie};
use provider::{Lan, ProviderError};
use sel4_microkit::{Handler, Infallible, protection_domain};

include!("mapping.rs");

fn status(offset: usize) -> &'static AtomicU32 {
    unsafe { &*((STATUS + offset * 4) as *const AtomicU32) }
}

fn ticks() -> u64 {
    let value;
    unsafe {
        core::arch::asm!("isb; mrs {}, cntvct_el0", out(reg) value, options(nostack));
    }
    value
}

fn delay(us: u32) {
    let start = ticks();
    while ticks().wrapping_sub(start) < u64::from(us) * 24 {
        core::hint::spin_loop();
    }
}

fn error_code<E>(error: BringupError<E>) -> u32 {
    match error {
        BringupError::Provider(ProviderError::ReferenceClock) => 65,
        BringupError::Provider(ProviderError::GateReadback) => 66,
        BringupError::Provider(ProviderError::PipePowerTimeout) => 67,
        BringupError::Provider(ProviderError::PipeIdleTimeout) => 68,
        BringupError::ResetReadback | BringupError::GpioReadback => 69,
        BringupError::PhyNotReady => 70,
        BringupError::Link(_) => 71,
        #[cfg(feature = "lan1-only")]
        BringupError::Probe(pcie::ProbeError::InboundMode) => 161,
        BringupError::Probe(_) => 72,
        BringupError::Bar(_) => 73,
        BringupError::UnsupportedMethod(_) => 74,
        BringupError::Io(_) | BringupError::Provider(ProviderError::Io(_)) => 75,
    }
}

#[protection_domain]
fn init() -> impl Handler {
    #[cfg(not(feature = "supervised"))]
    run();
    Idle
}

#[cfg(feature = "supervised")]
fn phase(value: u32) {
    status(7).store(value, Ordering::Release);
    #[cfg(feature = "tx-diagnostic")]
    {
        status(256).fetch_add(1, Ordering::SeqCst);
        for i in 0..b51::REPORT_WORDS {
            status(257 + i).store(status(i).load(Ordering::Relaxed), Ordering::Relaxed);
        }
        status(256).fetch_add(1, Ordering::Release);
    }
}

fn run() {
    #[cfg(feature = "supervised")]
    phase(1);
    let frequency: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frequency, options(nostack));
    }
    if frequency != 24_000_000 {
        status(0).store(65, Ordering::Release);
        return;
    }
    let maps = PHYSICAL.map(|(physical, address, size)| Mapping {
        physical,
        virtual_base: NonNull::new(address as *mut u8).unwrap(),
        size,
    });
    let Ok(io) = (unsafe { DeviceIo::new(&maps, delay) }) else {
        status(0).store(75, Ordering::Release);
        return;
    };
    #[cfg(all(feature = "lan2", not(feature = "lan2-standalone")))]
    let host: Result<_, BringupError<mmio::AccessError>> = {
        let shared = unsafe { &*(0x41100000 as *const AtomicU32) };
        let start = ticks();
        loop {
            match shared.load(Ordering::Acquire) {
                1 => break Ok(R5sPcie::after_shared_prepare(io)),
                0 if ticks().wrapping_sub(start) < 24_000_000 * 10 => core::hint::spin_loop(),
                _ => break Err(BringupError::PhyNotReady),
            }
        }
    };
    #[cfg(any(not(feature = "lan2"), feature = "lan2-standalone"))]
    let host = R5sPcie::prepare(io);
    match host {
        Err(error) => status(0).store(error_code(error), Ordering::Release),
        Ok(mut pcie) => {
            status(0).store(1, Ordering::Release);
            #[cfg(feature = "lan1-only")]
            {
                let result = prepare_lan(pcie);
                #[cfg(feature = "lan-port")]
 status(32).store(0, Ordering::Release);
 status(1).store(result.err().unwrap_or(167), Ordering::Release);
                return;
            }
            #[cfg(not(feature = "lan1-only"))]
            {
                let window = InboundWindow {
                    physical: 0x24200000,
                    bus: 0x80000000,
                    size: 0x300000,
                };
                let ports = [Lan::One, Lan::Two];
                for (lane, port) in ports.into_iter().enumerate() {
                    let result = pcie.detect(port).and_then(|()| pcie.identify_method(port));
                    let result = result.and_then(|(identity, method)| {
                        let control = pcie.inbound_region0_control(port)?;
                        status(lane + 3).store(control, Ordering::Relaxed);
                        let window = InboundWindow {
                            physical: if lane == 0 { 0x24200000 } else { 0x24300000 },
                            bus: if lane == 0 { 0x80000000 } else { 0x81000000 },
                            size: if lane == 0 { 0x300000 } else { 0x200000 },
                        };
                        pcie.map_inbound(port, &window)?;
                        let _ = identity;
                        Ok(method)
                    });
                    let value = match result {
                        Ok(ring::Method::A48) => 148,
                        Ok(ring::Method::A49) => 149,
                        Ok(ring::Method::B50) => 150,
                        Ok(ring::Method::B51) => 151,
                        Err(error) => error_code(error),
                    };
                    status(lane + 1).store(value, Ordering::Release);
                }
            }
        }
    }
}

#[cfg(feature = "lan1-only")]
fn progress(id: u32) {
    status(2).fetch_or(1 << (id - 1), Ordering::Release);
}

#[cfg(feature = "lan1-only")]
fn prepare_lan<F: FnMut(u32)>(mut host: R5sPcie<DeviceIo<'_, F>>) -> Result<(), u32> {
    use pcie::ConfigIo;
    #[cfg(feature = "supervised")]
    phase(2);
    host.detect(b51::PORT).map_err(error_code)?;
    progress(1);
    let revision = host.revision(b51::PORT).map_err(error_code)?;
    status(4).store(revision.into(), Ordering::Release);
    let (identity, method) = match host.identify_method(b51::PORT) {
        Ok(result) => result,
        Err(BringupError::UnsupportedMethod(identity)) => {
            status(4).store(identity.revision.into(), Ordering::Release);
            status(5).store(identity.tx_config, Ordering::Release);
            status(8).store(identity.pci_id, Ordering::Release);
            progress(20);
            return Err(74);
        }
        Err(error) => return Err(error_code(error)),
    };
    status(4).store(identity.revision.into(), Ordering::Release);
    status(5).store(identity.tx_config, Ordering::Release);
    status(8).store(identity.pci_id, Ordering::Release);
    let start = match method {
        ring::Method::B51 => {
            progress(16);
            b51::prepare
        }
        _ => return Err(74),
    };
    #[cfg(feature = "supervised")]
    phase(12);
    progress(2);
    let (va, window) = b51::window()?;
    progress(3);
    #[cfg(feature = "tx-diagnostic")]
    {
        for (index, value) in [b51::DBI as u32, (b51::DBI >> 32) as u32, window.bus as u32,
            (window.bus + window.size - 1) as u32, window.physical as u32, 0].into_iter().enumerate() {
            status(38 + index).store(value, Ordering::Relaxed);
        }
        status(247).store(1, Ordering::Relaxed);
        phase(13);
    }
    let mut io = host.into_io();
    #[cfg(feature = "tx-diagnostic")]
    {
        for (i, offset) in [4, 0x78, 0x80, 0x104, 0x110].into_iter().enumerate() {
            status(48 + i).store(io.read32(b51::DBI + offset).unwrap_or(u32::MAX), Ordering::Relaxed);
        }
        use pcie::EndpointConfig;
        let mut endpoint = pcie::DirectEndpoint { io: &mut io, dbi: b51::DBI, config_aperture: b51::CONFIG, buses: b51::BUSES };
        for (i, offset) in [4, 0x18, 0x1c, 0x20, 0x24, 0x44, 0x78, 0x80, 0x104, 0x110].into_iter().enumerate() {
            status(53 + i).store(endpoint.read_dword(offset).unwrap_or(u32::MAX), Ordering::Relaxed);
        }
    }
    let mapped = pcie::map_inbound(&mut io, b51::DBI, &window).map_err(|error| match error {
        pcie::ProbeError::WindowTimeout => 162u32,
        _ => 161u32,
    });
    #[cfg(any(feature = "tx-diagnostic", feature = "lan-port"))]
    {
        let aperture_ok = b51::inspect_aperture(&mut io, &window, mapped.as_ref().ok().copied());
        if mapped.is_ok() && !aperture_ok {
            status(247).store(2, Ordering::Relaxed);
            let disabled = pcie::set_enabled(&mut io, b51::DBI + 0x300104, false).is_ok();
            status(63).store(u32::from(disabled), Ordering::Relaxed);
            return Err(161);
        }
        if mapped.is_ok() {
            for index in 28..64 {
                status(index).store(if cfg!(feature = "lan-port") { 0 } else { u32::MAX }, Ordering::Relaxed);
            }
            status(247).store(3, Ordering::Relaxed);
        }
    }
    mapped?;
    progress(4);
    let dbi = b51::DBI;
    io.write32(dbi + 4, 2).map_err(|_| 75u32)?;
    let endpoint = |io: &mut DeviceIo<'_, F>, command| {
        pcie::set_endpoint_command(io, dbi, b51::CONFIG, b51::BUSES, command).map_err(|_| 72u32)
    };
    if io.read32(dbi + 4).map_err(|_| 75u32)? & 7 != 2 {
        return Err(75);
    }
    if let Err(error) = endpoint(&mut io, 2) {
        io.write32(dbi + 4, 0).map_err(|_| 170u32)?;
        return Err(error);
    }
    let mut ep = pcie::DirectEndpoint {
        io: &mut io,
        dbi,
        config_aperture: b51::CONFIG,
        buses: b51::BUSES,
    };
    use pcie::EndpointConfig;
    let result = (|| {
        let old = ep.read_dword(0x70c).map_err(|_| 72u32)?;
        let value = (old & 0x00ffffff) | 0x27000000;
        ep.write_dword(0x70c, value).map_err(|_| 72u32)?;
        if ep.read_dword(0x70c).map_err(|_| 72u32)? != value {
            return Err(72);
        }
        #[cfg(feature = "supervised")]
        phase(3);
        unsafe { start(ep.io, identity, va, &window) }
    })();
    #[cfg(feature = "tx-diagnostic")]
    {
        status(135).store(result.as_ref().err().copied().unwrap_or(0), Ordering::Relaxed);
        status(143).store(status(7).load(Ordering::Relaxed), Ordering::Relaxed);
    }
    if !b51::shutdown(&mut io, identity) {
        let _ = R5sPcie::hold_reset(&mut io, b51::PORT);
        status(3).store(result.err().unwrap_or(0), Ordering::Release);
        return Err(170);
    }
    result
}

struct Idle;
impl Handler for Idle {
    type Error = Infallible;

    #[cfg(feature = "supervised")]
    fn notified(&mut self, channels: sel4_microkit::ChannelSet) -> Result<(), Infallible> {
        if channels.contains(sel4_microkit::Channel::new(0)) && status(7).load(Ordering::Acquire) == 0 {
            #[cfg(feature = "tx-diagnostic")]
            b51::init_report();
            run();
            #[cfg(feature = "tx-diagnostic")]
            if status(135).load(Ordering::Relaxed) == u32::MAX {
                let provider = status(1).load(Ordering::Relaxed);
                status(135).store(match provider { 167 => 0, 0 => status(0).load(Ordering::Relaxed), code => code }, Ordering::Relaxed);
                status(143).store(status(7).load(Ordering::Relaxed), Ordering::Relaxed);
            }
            phase(0xff);
            #[cfg(feature = "tx-diagnostic")]
            for i in (0..b51::REPORT_WORDS).step_by(4) {
                let word = |n| status(i + n).load(Ordering::Acquire);
                sel4_microkit::debug_println!("B51v1 {:03} {:08x} {:08x} {:08x} {:08x}", i,
                    word(0usize), word(1usize), word(2usize), word(3usize));
            }
        }
        Ok(())
    }
}
