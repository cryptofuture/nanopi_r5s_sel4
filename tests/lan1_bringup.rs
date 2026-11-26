extern crate rtl8125;
use rtl8125::{dma, mac, pcie, provider, ring};
#[cfg(feature = "tx-diagnostic")]
use rtl8125::mmio;
use std::{cell::Cell, collections::BTreeMap, ptr::NonNull};
#[path = "../pds/pcie_provider/src/arp.rs"]
mod arp;
#[path = "../pds/pcie_provider/src/b51.rs"]
mod b51;
#[path = "../pds/gmac_dma_corrected/src/tso.rs"]
mod tso;
#[path = "../pds/gmac_dma_corrected/src/zc.rs"]
mod zc;
#[path = "../pds/gmac_dma_corrected/src/tso_testgen.rs"]
mod tso_testgen;
#[path = "../pds/wan_dhcp_diagnostic/poll_deadline.rs"]
mod poll_deadline;

#[cfg(feature = "lan-port")]
extern crate self as sel4;
#[cfg(feature = "lan-port")]
pub fn r#yield() {}

#[cfg(feature = "lan-port")]
#[path = "../pds/gmac_dma_corrected/src/queue.rs"]
mod real_queue;

#[cfg(feature = "lan-port")]
mod queue {
    use super::*;
    pub use real_queue::{batch, COUNT};
    pub const DIRECTION: usize = 0x4000;
    thread_local! {
        pub static POPPED: Cell<u8> = const { Cell::new(0) };
        pub static DOWN_SAMPLES: Cell<u8> = const { Cell::new(0) };
    }
    pub fn pop(_: usize, _: u32, frame: &mut [u8]) -> Result<usize, ()> {
        POPPED.with(|n| {
            if n.get() == 112 { return Ok(0); }
            frame[..60].fill(n.get());
            n.set(n.get() + 1);
            Ok(60)
        })
    }
    pub unsafe fn consume_in_place<T>(base:usize, consume:impl FnOnce(&mut [u8],u32)->T) -> Result<Option<T>,()> {
        let mut frame=[0;1514];
        let len=pop(base,0,&mut frame)?;
        Ok((len!=0).then(||consume(&mut frame[..len],0)))
    }
    pub fn push_meta(base:usize,epoch:u32,frame:&[u8],_:u32)->Result<bool,()> {
        push(base,epoch,frame)
    }
    pub fn push(_: usize, _: u32, _: &[u8]) -> Result<bool, ()> { Ok(true) }
}

thread_local! {
    static CLOCK: Cell<u64> = const { Cell::new(0) };
    static EVENTS: Cell<u32> = const { Cell::new(0) };
    static SUPERVISOR: Cell<Option<u64>> = const { Cell::new(None) };
}
fn ticks() -> u64 {
    CLOCK.with(|t| {
        let next = t.get() + if cfg!(feature = "lan-port") { 240_000 } else { poll_deadline::COUNTER_HZ };
        SUPERVISOR.with(|sample| {
            if let Some(mut previous) = sample.get() {
                // Model a runnable supervisor during each second of foreground LAN work.
                for ms in 1..=1000 {
                    let now = t.get() + ms * poll_deadline::TICKS_PER_MS;
                    assert_eq!(poll_deadline::missed(&mut previous, now), None);
                }
                sample.set(Some(previous));
            }
        });
        t.set(next);
        t.get()
    })
}
fn progress(id: u32) {
    EVENTS.with(|e| e.set(e.get() | 1 << (id - 1)));
}

#[cfg(feature = "tx-diagnostic")]
fn status(index: usize) -> &'static std::sync::atomic::AtomicU32 {
    use std::sync::atomic::AtomicU32;
    static WORDS: [AtomicU32; b51::REPORT_WORDS] = [const { AtomicU32::new(0) }; b51::REPORT_WORDS];
    &WORDS[index]
}

const BAR: u64 = b51::BAR;
const CFG: u64 = b51::CONFIG.0;
const BUS: u64 = if cfg!(feature = "lan2") {
    0x81000000
} else {
    0x80000000
};
const SIZE: usize = 0x100000;
const MAC: [u8; 6] = [2, 0x52, 0x35, 0x53, 0x81, 0x25];
const IDENTITY: ring::Identity = ring::Identity {
    pci_id: 0x8125_10ec,
    revision: 0x05,
    tx_config: 0x6410_0000,
};

#[derive(PartialEq, Clone, Copy)]
enum Failure {
    None,
    Firmware,
    Base,
    CPlusReadback,
    Phy,
    Rx,
    Tx,
    Start,
}

struct Model {
    pool: NonNull<u8>,
    regs: BTreeMap<u64, u32>,
    ocp: BTreeMap<(bool, u32), u16>,
    fail: Failure,
    supplied: bool,
    master: bool,
    transmitted: bool,
    rss_setup: bool,
    #[cfg(feature = "tx-diagnostic")]
    root_error: u32,
    #[cfg(feature = "tx-diagnostic")]
    tally_event: u32,
    #[cfg(feature = "tx-diagnostic")]
    tally_requests: usize,
    #[cfg(feature = "lan-dhcp")]
    sent: Vec<Vec<u8>>,
}
impl Model {
    fn new(fail: Failure) -> Self {
        let layout = std::alloc::Layout::from_size_align(SIZE, 4096).unwrap();
        let pool = NonNull::new(unsafe { std::alloc::alloc_zeroed(layout) }).unwrap();
        let mut regs = BTreeMap::from([(BAR + 0xe0, 0x2163), (BAR + 0xd3, 0x32), (BAR + 0xe2, 0x103), (BAR + 0x6c, 2)]);
        for (i, v) in MAC.iter().enumerate() {
            regs.insert(BAR + 0x19e0 + i as u64, u32::from(*v));
        }
        Self {
            pool,
            regs,
            ocp: BTreeMap::new(),
            fail,
            supplied: false,
            master: false,
            transmitted: false,
            rss_setup: false,
            #[cfg(feature = "tx-diagnostic")]
            root_error: 0,
            #[cfg(feature = "tx-diagnostic")]
            tally_event: 28,
            #[cfg(feature = "tx-diagnostic")]
            tally_requests: 0,
            #[cfg(feature = "lan-dhcp")]
            sent: Vec::new(),
        }
    }
    fn ptr(&self, bus: u64, size: usize) -> *mut u8 {
        let offset = bus.checked_sub(BUS).unwrap() as usize;
        assert!(offset + size <= SIZE);
        unsafe { self.pool.as_ptr().add(offset) }
    }
    fn descriptor(&self, offset: u64, slot: usize) -> (*mut u32, u64) {
        let extended = offset == 0x20 && self.ocp.get(&(true, 0xeb58)).copied().unwrap_or(0) & 1 != 0;
        let stride = if extended { 32 } else { 16 };
        let address = u64::from(self.regs[&(BAR + offset)]) + (slot * stride) as u64;
        let ptr = self.ptr(address, stride).cast::<u32>();
        if extended {
            assert_eq!(unsafe { std::slice::from_raw_parts(ptr.add(4), 4) }, &[0; 4]);
        }
        let bus = unsafe { u64::from(*ptr.add(2)) | u64::from(*ptr.add(3)) << 32 };
        (ptr, bus)
    }
    fn inspect_rings(&self) {
        for slot in 0..64 {
            let end = if slot == 63 { 1 << 30 } else { 0 };
            for (offset, flags, index) in [(0x20, end, slot), (0xe4, 0x80000800 | end, 64 + slot)] {
                let (d, bus) = self.descriptor(offset, slot);
                assert_eq!(unsafe { *d }, flags);
                assert_eq!(unsafe { *d.add(1) }, 0);
                assert_eq!(bus, BUS + 4096 + index as u64 * 2048);
            }
        }
    }
}
impl Drop for Model {
    fn drop(&mut self) {
        unsafe {
            std::alloc::dealloc(
                self.pool.as_ptr(),
                std::alloc::Layout::from_size_align(SIZE, 4096).unwrap(),
            )
        }
    }
}
impl pcie::ConfigIo for Model {
    type Error = ();
    fn read32(&mut self, a: u64) -> Result<u32, ()> {
        #[cfg(feature = "lan-port")]
        if a == BAR + 0x3c && status(39).load(std::sync::atomic::Ordering::Relaxed) == 112 {
            return Ok(0x8000);
        }
        if a == BAR + 0x4500 {
            assert!(self.rss_setup);
            if self.fail == Failure::Base { return Ok(0); }
        }
        #[cfg(feature = "tx-diagnostic")]
        if a == b51::DBI + 4 && !self.sent.is_empty() {
            return Ok(self.root_error | self.regs[&a]);
        }
        if a == BAR + 0x40 {
            return Ok(0x64100000 | self.regs.get(&a).copied().unwrap_or(0));
        }
        if a == BAR + 0xe4 && self.fail == Failure::Base {
            return Ok(0);
        }
        if !cfg!(feature = "lan-dhcp") && a == BAR + 0x3c && self.master && !self.supplied && self.fail != Failure::Rx {
            let (d, bus) = self.descriptor(0xe4, 0);
            let payload = unsafe { std::slice::from_raw_parts_mut(self.ptr(bus, 64), 64) };
            payload[..6].fill(255);
            let peer = [2, 0, 0, 0, 0, 2];
            payload[6..12].copy_from_slice(&peer);
            payload[12..22].copy_from_slice(&[8, 6, 0, 1, 8, 0, 6, 4, 0, 1]);
            payload[22..28].copy_from_slice(&peer);
            payload[28..32].copy_from_slice(&[10, 44, 1, 2]);
            payload[38..42].copy_from_slice(&[10, 44, 1, 1]);
            unsafe { *d = 0x30000040 };
            self.supplied = true;
        }
        Ok(self.regs.get(&a).copied().unwrap_or(0))
    }
    fn write32(&mut self, a: u64, mut value: u32) -> Result<(), ()> {
        #[cfg(feature = "tx-diagnostic")]
        if a == BAR + 0x10 && value & 8 != 0 {
            self.tally_requests += 1;
            assert!(self.master);
            let destination = BUS + if self.regs[&(BAR + 0x37)] == 4 { 0xff100 } else { 0xff900 };
            assert_eq!(value, destination as u32 | 8);
            assert_eq!(self.regs[&(BAR + 0x14)], 0);
            let buffer = self.ptr(destination, 192).cast::<u32>();
            assert_eq!(unsafe { *buffer }, 0xa5a5_a5a5);
            if self.tally_event == 28 {
                unsafe { buffer.write(self.tally_requests as u32); buffer.add(1).write(0) };
            }
            if self.tally_event == 31 {
                unsafe { buffer.add(48).write(0) };
            }
            if self.tally_event != 30 {
                value &= !8;
            }
        }
        if a == BAR + 0x4500 {
            assert!(!self.master);
            if !self.rss_setup {
                assert_eq!(value, 0);
                assert!(!self.regs.contains_key(&a));
                assert_eq!(self.regs[&(BAR + 0x50)], 0xc0);
            }
        }
        if a == BAR + 0xb0 || a == BAR + 0xb8 {
            let mac = a == BAR + 0xb0;
            let address = (value & 0x7fffffff) >> 15 & !1;
            if value >> 31 != 0 {
                let mut data = value as u16;
                if !mac && address == 0xa400 {
                    data &= !0x8000;
                }
                self.ocp.insert((mac, address), data);
                value = 0;
            } else {
                let datum = match (mac, address) {
                    (false, 0xb800) if self.fail != Failure::Firmware => 0x40,
                    (false, 0xa402) if self.fail != Failure::Phy => 0x24,
                    key => self.ocp.get(&key).copied().unwrap_or(0),
                };
                value = u32::from(datum) | if mac { 0 } else { 1 << 31 };
            }
        }
        if (a == BAR + 0x44 && value == 0x40000f0a) || (a == BAR + 0x40 && value == 0x03000700) {
            assert_eq!(self.regs[&(BAR + 0x37)] & 0x0c, 0);
        }
        if a == BAR + 0x80 {
            value &= !(1 << 31);
        }
        if a == BAR + 0x3c {
            value = 0;
        }
        if a == (CFG + 4) && value == 6 {
            assert_eq!(self.regs[&(b51::DBI + 4)], 6);
            assert_eq!(self.regs[&(BAR + 0xe0)], 0x2063);
            assert_eq!(self.regs[&(BAR + 0x4500)], 0);
            self.inspect_rings();
            self.master = true;
        }
        self.regs.insert(a, value);
        Ok(())
    }
    fn delay_us(&mut self, _: u32) {}
}

#[test]
fn b51_rss_registers_select_queues_and_require_readback() {
    let mut io = Model::new(Failure::None);
    io.rss_setup = true;
    assert_eq!(b51::configure_rss(&mut io, 3), Err(166));
    assert!(!io.regs.contains_key(&(BAR + 0x4500)));
    for (queues, mask, table) in [(2, 4, 0x01000100), (4, 8, 0x03020100)] {
        b51::configure_rss(&mut io, queues).unwrap();
        assert_eq!(io.regs[&(BAR + 0x4800)] & 0x1c, mask);
        assert_eq!(io.regs[&(BAR + 0x4500)], queues.ilog2() << 16 | 0x703);
        assert!(io.regs.contains_key(&(BAR + 0x4624)));
        for offset in (0x4700..0x4780).step_by(4) {
            assert_eq!(io.regs[&(BAR + offset)], table);
        }
    }
    io.fail = Failure::Base;
    assert_eq!(b51::configure_rss(&mut io, 4), Err(166));
    assert!(!io.master);
}
impl mac::Registers for Model {
    fn read8(&mut self, a: u64) -> Result<u8, ()> {
        Ok(self.regs.get(&a).copied().unwrap_or(0) as u8)
    }
    fn read16(&mut self, a: u64) -> Result<u16, ()> {
        #[cfg(feature = "lan-port")]
        if !self.sent.is_empty() {
            if a == BAR + 0x6c && self.sent.len() == 8 {
                let down = queue::DOWN_SAMPLES.with(|n| {
                    n.set(n.get().saturating_add(1));
                    n.get() <= 2
                });
                if down || self.fail == Failure::Tx { return Ok(0); }
            }
            if a == BAR + dma::TX_CLOSE {
                let previous = self.regs.get(&a).copied().unwrap_or(0) as usize;
                for n in previous..self.sent.len() {
                    let (_, bus) = self.descriptor(0x20, n % 64);
                    assert_eq!(unsafe { std::slice::from_raw_parts(self.ptr(bus, 60), 60) }, &[n as u8; 60]);
                }
                let close = match self.sent.len() {
                    112 => 112,
                    n if n >= 63 => n - 5,
                    _ => 0,
                };
                self.regs.insert(a, close as u32);
            }
        }
        Ok(self.regs.get(&a).copied().unwrap_or(0) as u16)
    }
    fn write8(&mut self, a: u64, mut v: u8) -> Result<(), ()> {
        assert_ne!(a, BAR + 0x4500);
        if a == BAR + 0x37 {
            if v & 4 != 0 {
                assert!(self.master);
                #[cfg(feature = "tx-no-close")]
                assert_eq!(self.regs[&(BAR + 0x40)] & 0x40, 0x40);
                if self.fail == Failure::Start {
                    return Err(());
                }
            }
            if v == 0x10 {
                v = 0;
            }
        }
        self.regs.insert(a, u32::from(v));
        Ok(())
    }
    fn write16(&mut self, a: u64, mut v: u16) -> Result<(), ()> {
        if a == BAR + 0xe0 && self.fail == Failure::CPlusReadback {
            v &= !0x20;
        }
        #[cfg(feature = "lan-dhcp")]
        if a == BAR + 0x90 || a == BAR + dma::TX_TAIL {
            let no_close = self.regs.get(&(BAR + 0x40)).is_some_and(|config| config & 0x40 != 0);
            assert_eq!(a, BAR + if no_close { dma::TX_TAIL } else { 0x90 });
            assert_eq!(v, if no_close { (self.sent.len() + 1) as u16 } else { 1 });
            let (d, bus) = self.descriptor(0x20, self.sent.len() % 64);
            let flags = unsafe { *d };
            assert_eq!(flags & 0xb0000000, 0xb0000000);
            assert_eq!(unsafe { *d.add(1) }, 0);
            let packet = unsafe { std::slice::from_raw_parts(self.ptr(bus, (flags & 0xffff) as usize), (flags & 0xffff) as usize) };
            self.sent.push(packet.to_vec());
            if self.fail != Failure::Tx && !cfg!(feature = "lan-port") {
                if no_close {
                    self.regs.insert(BAR + dma::TX_CLOSE, u32::from(v));
                } else {
                    unsafe { *d &= !(1 << 31) }
                }
            }
        }
        #[cfg(not(feature = "lan-dhcp"))]
        if a == BAR + 0x90 {
            let (d, bus) = self.descriptor(0x20, 0);
            assert_eq!(unsafe { *d }, 0xb000003c);
            let packet = unsafe { std::slice::from_raw_parts(self.ptr(bus, 60), 60) };
            assert_eq!(packet[6..12], MAC);
            assert_eq!(packet[21], 2);
            assert_eq!(packet[28..32], [10, 44, 1, 1]);
            assert_eq!(packet[38..42], [10, 44, 1, 2]);
            assert_eq!(packet[42..], [0; 18]);
            self.transmitted = true;
            if self.fail != Failure::Tx {
                unsafe { *d &= !(1 << 31) }
            }
        }
        self.regs.insert(a, u32::from(v));
        Ok(())
    }
}

#[cfg(not(feature = "lan-dhcp"))]
#[test]
fn b51_activation_and_bounded_failures() {
    for (failure, expected, master) in [
        (Failure::None, Ok(()), true),
        (Failure::Firmware, Err(164), false),
        (Failure::Base, Err(166), false),
        (Failure::CPlusReadback, Err(166), false),
        (Failure::Phy, Err(165), false),
        (Failure::Rx, Err(168), true),
        (Failure::Start, Err(171), true),
        (Failure::Tx, Err(169), true),
    ] {
        EVENTS.with(|e| e.set(0));
        let mut m = Model::new(failure);
        let va = m.pool;
        let window = dma::InboundWindow {
            physical: if cfg!(feature = "lan2") {
                0x24300000
            } else {
                0x24200000
            },
            bus: BUS,
            size: SIZE as u64,
        };
        SUPERVISOR.with(|sample| sample.set(Some(CLOCK.with(Cell::get))));
        assert_eq!(unsafe { b51::prepare(&mut m, IDENTITY, va, &window) }, expected);
        SUPERVISOR.with(|sample| {
            let mut previous = sample.replace(None).unwrap();
            let now = CLOCK.with(Cell::get) + poll_deadline::DEADLINE_TICKS + 1;
            assert_eq!(
                poll_deadline::missed(&mut previous, now),
                Some(poll_deadline::DEADLINE_TICKS + 1)
            );
        });
        assert_eq!(m.master, master);
        if failure == Failure::None {
            assert!(m.transmitted);
            assert_eq!(EVENTS.with(Cell::get), 0x1ff0);
            assert_eq!(m.ocp.get(&(true, 0xfc48)), Some(&0x3f));
            assert_eq!(m.ocp.get(&(false, 0xa438)), Some(&0x2417));
        }
    }
}

#[test]
fn unsupported_method_never_arms_dma() {
    let mut model = Model::new(Failure::None);
    let identity = ring::Identity { tx_config: 0x6090_0000, ..IDENTITY };
    let window = dma::InboundWindow {
        physical: if cfg!(feature = "lan2") { 0x24300000 } else { 0x24200000 },
        bus: BUS,
        size: SIZE as u64,
    };
    let pool = model.pool;
    assert_eq!(unsafe { b51::prepare(&mut model, identity, pool, &window) }, Err(163));
    assert!(!model.master);
    assert!(!model.regs.contains_key(&(BAR + 0x20)));
}

#[cfg(feature = "tx-diagnostic")]
#[test]
fn one_tx_without_rx_reports_completion_timeout_and_pci_errors() {
    use std::sync::atomic::Ordering::Acquire;
    let window = dma::InboundWindow { physical: 0x24200000, bus: BUS, size: SIZE as u64 };
    let region = b51::DBI + 0x300100;
    let mut bridge = Model::new(Failure::None);
    bridge.regs.extend([(region + 4, 1 << 31), (region + 8, BUS as u32),
        (region + 16, (BUS + SIZE as u64 - 1) as u32), (region + 20, 0x24200000)]);
    assert!(b51::inspect_aperture(&mut bridge, &window, Some(BUS as u32 + SIZE as u32 - 1)));
    bridge.regs.extend([(region + 0xe04, 1 << 31), (region + 0xe08, BUS as u32),
        (region + 0xe10, BUS as u32 + 0xffff)]);
    assert!(!b51::inspect_aperture(&mut bridge, &window, Some(BUS as u32 + SIZE as u32 - 1)));
    bridge.regs.insert(region + 0xe0c, 1);
    bridge.regs.insert(region + 0xe20, 1);
    assert!(b51::inspect_aperture(&mut bridge, &window, Some(BUS as u32 + SIZE as u32 - 1)));
    bridge.regs.insert(region + 0xe04, 0xc0000000);
    assert!(!b51::inspect_aperture(&mut bridge, &window, Some(BUS as u32 + SIZE as u32 - 1)));
    for (failure, root_error, expected, milestone, tally_event) in [
        (Failure::None, 0, Ok(()), 22, 28),
        (Failure::Tx, 0, Err(169), 23, 28),
        (Failure::Tx, 0, Err(169), 23, 29),
        (Failure::Tx, 0, Err(169), 23, 30),
        (Failure::Tx, 0, Err(169), 23, 31),
        (Failure::None, 1 << 29, Err(171), 25, 28),
    ] {
        EVENTS.with(|events| events.set(0));
        let mut model = Model::new(failure);
        let window = dma::InboundWindow { physical: 0x24200000, bus: BUS, size: SIZE as u64 };
        let pool = model.pool;
        unsafe { pool.as_ptr().write_bytes(0xa5, 4096) };
        model.root_error = root_error;
        model.tally_event = tally_event;
        b51::init_report();
        assert_eq!(unsafe { b51::prepare(&mut model, IDENTITY, pool, &window) }, expected);
        assert_eq!(model.sent.len(), 1);
        assert_eq!(model.tally_requests, if cfg!(feature = "tally-rx-first") || matches!(tally_event, 30 | 31) { 1 } else { 2 });
        assert_eq!(model.sent[0][..6], [255; 6]);
        assert_eq!(model.sent[0][28..32], [10, 44, 1, 1]);
        assert_eq!(model.sent[0][42..], [0; 18]);
        assert_eq!(unsafe { *model.descriptor(0xe4, 0).0 }, 0x80000800);
        assert_eq!(model.regs[&(BAR + 0x37)], if tally_event == 31 && !cfg!(feature = "tally-rx-first") { 4 } else { 12 });
        assert_eq!(status(11).load(Acquire), root_error | 6);
        assert_eq!(status(9).load(Acquire) >> 31, u32::from(failure == Failure::Tx));
        assert_eq!(model.ocp[&(true, 0xe63e)] & 0xc30, 0x20);
        assert_eq!(model.ocp[&(true, 0xe0c0)], 0x4000);
        assert_eq!(model.ocp[&(true, 0xd430)] & 0xfff, 0x45f);
        assert_eq!(model.regs[&(BAR + 0x57)], 0x10);
        #[cfg(feature = "tx-no-close")]
        {
            assert_eq!(model.ocp[&(true, 0xeb58)] & 1, 1);
            assert_eq!(model.regs[&(BAR + 0x57)], 0x10);
            assert_eq!(model.regs[&(BAR + 0xd0)] & 0xc0, 0xc0);
            assert_eq!(model.regs[&(BAR + 0xe4)], (BUS + 0x900) as u32);
            for (offset, size) in [(0x800, 32), (0xd00, 16)] {
                let guard = unsafe { std::slice::from_raw_parts(model.ptr(BUS + offset, size), size) };
                assert!(guard.iter().all(|&v| v == 0));
            }
            assert_eq!(status(13).load(Acquire), 0);
            assert_eq!(status(14).load(Acquire), 0);
            assert_eq!(status(15).load(Acquire), 1);
            assert_eq!(status(16).load(Acquire), u32::from(failure != Failure::Tx));
            assert_ne!(unsafe { *model.descriptor(0x20, 0).0 } & (1 << 31), 0);
        }
        assert_eq!(status(32).load(Acquire), 0xb000003c);
        assert_eq!(status(34).load(Acquire), BUS as u32 + 4096);
        if cfg!(feature = "tx-check") { assert_eq!(status(246).load(Acquire), 1); }
        if cfg!(feature = "tally-rx-first") {
            assert_eq!(status(21).load(Acquire), 0);
            assert_eq!(status(128).load(Acquire), 0);
            assert_eq!(status(22).load(Acquire), tally_event);
        } else {
            assert_eq!(status(21).load(Acquire), tally_event);
            if tally_event == 28 {
                assert_eq!(status(144).load(Acquire), 1);
                assert_eq!(status(192).load(Acquire), 2);
                assert_eq!(status(133).load(Acquire), 0);
                assert_eq!(status(141).load(Acquire), 0);
            }
            assert_eq!(status(22).load(Acquire), if tally_event == 31 { 0 } else { tally_event });
            if tally_event == 30 {
                assert_eq!(status(128).load(Acquire), 3);
                assert_eq!(status(136).load(Acquire), 1);
                assert_eq!(status(192).load(Acquire), u32::MAX);
            }
        }
        let events = EVENTS.with(Cell::get);
        assert_ne!(events & (1 << 20), 0);
        assert_ne!(events & (1 << (milestone - 1)), 0);
        assert_ne!(events & (1 << (tally_event - 1)), 0);
    }
}

#[test]
fn shutdown_checks_pending_transactions_and_never_recycles_slots() {
    for (pending, ok) in [(1 << 21, false), (0, true)] {
        let mut model = Model::new(Failure::None);
        model.regs.extend([
            (CFG + 0x34, 0x70),
            (CFG + 0x70, 0x10),
            (CFG + 0x78, pending),
        ]);
        unsafe {
            model.pool.as_ptr().write(0xa5);
        }
        assert_eq!(b51::shutdown(&mut model, IDENTITY), ok);
        assert_eq!(model.regs[&(CFG + 4)], 0);
        assert_eq!(model.regs[&(b51::DBI + 4)], 0);
        assert_eq!(unsafe { model.pool.as_ptr().read() }, 0xa5);
    }
}

#[test]
fn absent_microkit_pool_fails_closed() {
    assert!(matches!(b51::window(), Err(160)));
}

#[cfg(all(feature = "lan-dhcp", not(any(feature = "tx-diagnostic", feature = "lan-port"))))]
fn status(_: usize) -> &'static std::sync::atomic::AtomicU32 {
    static STATUS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    &STATUS
}

#[cfg(feature = "lan-dhcp")]
mod service {
    use super::*;
    use r5s_router_core::{arp, checksum, dhcp, ipat, set16, setip, transport_checksum, u16at};

    const PEER: [u8; 6] = [0xd4, 0x93, 0x90, 0x1e, 0x74, 0x9d];
    const SERVER: [u8; 6] = [2, 0x52, 0x35, 0x53, 0, 2];
    const IP: u32 = 0x0a2c0101;
    const XID: u32 = 0x12345678;

    fn request(kind: u8, wanted: Option<u32>, broadcast: bool) -> Vec<u8> {
        let mut data = vec![0; 240];
        data[..4].copy_from_slice(&[1, 1, 6, 0]);
        setip(&mut data, 4, XID);
        set16(&mut data, 10, if broadcast { 0x8000 } else { 0 });
        data[28..34].copy_from_slice(&PEER);
        data[236..240].copy_from_slice(&[99, 130, 83, 99]);
        data.extend([53, 1, kind]);
        if let Some(ip) = wanted {
            data.extend([50, 4]);
            data.extend(ip.to_be_bytes());
            data.extend([54, 4]);
            data.extend(IP.to_be_bytes());
        }
        data.push(255);
        let length = 28 + data.len();
        let mut frame = vec![0; 14 + length];
        frame[..6].copy_from_slice(&[255; 6]);
        frame[6..12].copy_from_slice(&PEER);
        set16(&mut frame, 12, 0x0800);
        let packet = &mut frame[14..];
        packet[0] = 0x45;
        packet[8] = 64;
        packet[9] = 17;
        set16(packet, 2, length as u16);
        setip(packet, 16, u32::MAX);
        set16(packet, 20, 68);
        set16(packet, 22, 67);
        set16(packet, 24, (length - 20) as u16);
        packet[28..].copy_from_slice(&data);
        set16(packet, 10, checksum(&packet[..20]));
        let udp = transport_checksum(packet, length);
        set16(packet, 26, if udp == 0 { u16::MAX } else { udp });
        frame
    }

    type TestDma = dma::PackedDma<Model, 64, { cfg!(feature = "tx-no-close") }>;

    fn backend(fail: Failure) -> (TestDma, ring::Ring<64>) {
        let mut model = Model::new(fail);
        #[cfg(feature = "tx-no-close")]
        {
            model.ocp.insert((true, 0xeb58), 1);
            model.regs.insert(BAR + 0x40, 1 << 6);
        }
        let pool = model.pool;
        let memory = |offset, size| dma::DmaMemory {
            cpu: NonNull::new(unsafe { pool.as_ptr().add(offset) }).unwrap(),
            physical: 0x24200000 + offset as u64,
            size,
        };
        let window = dma::InboundWindow { physical: 0x24200000, bus: BUS, size: SIZE as u64 };
        let mut dma = unsafe { TestDma::from_verified_memory(
            model, BAR, memory(0, TestDma::DESCRIPTOR_BYTES), memory(4096, 64 * 4096), &window
        ) }.unwrap();
        let (tx, rx, _) = dma.bus_addresses();
        dma.registers().regs.extend([(BAR + 0x20, tx as u32), (BAR + 0xe4, rx as u32)]);
        let mut ring = ring::Ring::new(BUS + 4096, 64 * 4096).unwrap();
        ring.initialize(&mut dma).unwrap();
        #[cfg(feature = "tx-no-close")]
        dma.enable_no_close().unwrap();
        dma.registers().inspect_rings();
        (dma, ring)
    }

    fn inject(model: &mut Model, slot: usize, frame: &[u8], errors: u32) {
        let (descriptor, bus) = model.descriptor(0xe4, slot);
        let flags = unsafe { *descriptor };
        assert_eq!(flags & 0x80000000, 0x80000000);
        unsafe {
            std::ptr::copy_nonoverlapping(frame.as_ptr(), model.ptr(bus, frame.len()), frame.len());
            *descriptor = (flags & 0x40000000) | 0x30000000 | errors | (frame.len() as u32 + 4);
        }
    }

    fn check_dhcp(frame: &[u8], kind: u8, destination: [u8; 6]) -> u32 {
        assert_eq!(frame[..6], destination);
        assert_eq!(frame[6..12], SERVER);
        let options = dhcp::reply_options(&frame[42..]).unwrap();
        assert_eq!(options.kind, kind);
        assert_eq!(options.server, Some(IP));
        assert_eq!(ipat(frame, 46), XID);
        assert_eq!(frame[70..76], PEER);
        let ip = &frame[14..];
        assert_eq!(checksum(&ip[..20]), 0);
        assert_ne!(u16at(ip, 26), 0);
        assert_eq!(transport_checksum(ip, ip.len()), 0);
        let address = ipat(frame, 58);
        assert!((IP + 99..=IP + 198).contains(&address));
        address
    }

    #[test]
    fn packed_dma_delivers_arp_offer_and_ack_after_idle() {
        let (mut dma, mut ring) = backend(Failure::None);
        let mut service = b51::LanService::new(1);
        assert_eq!(service.poll(&mut dma, &mut ring, 600_000), Ok(()));
        let mut arp_frame = [0; 60];
        arp::request(PEER, IP + 1, IP, &mut arp_frame).unwrap();
        inject(dma.registers(), 0, &arp_frame, 1 << 19);
        service.poll(&mut dma, &mut ring, 600_001).unwrap();
        assert!(dma.registers().sent.is_empty());
        assert_eq!(ring::Dma::status(&mut dma, true, 0), Ok(0x80000800));

        arp_frame[18] = 5;
        inject(dma.registers(), 1, &arp_frame, 0);
        service.poll(&mut dma, &mut ring, 600_002).unwrap();
        assert!(dma.registers().sent.is_empty());
        arp_frame[18] = 6;
        inject(dma.registers(), 2, &arp_frame, 0);
        service.poll(&mut dma, &mut ring, 600_003).unwrap();
        let reply = &dma.registers().sent[0];
        assert_eq!(reply.len(), 60);
        assert_eq!(reply[..6], PEER);
        assert_eq!(reply[32..38], PEER);
        assert_eq!(arp::parse(reply), Ok((2, SERVER, IP, IP + 1)));

        let mut bad = request(1, None, true);
        bad[40] ^= 1;
        inject(dma.registers(), 3, &bad, 0);
        service.poll(&mut dma, &mut ring, 600_004).unwrap();
        assert_eq!(dma.registers().sent.len(), 1);

        let discover = request(1, None, true);
        assert_eq!(transport_checksum(&discover[14..], discover.len() - 14), 0);
        assert_eq!(dhcp::options(&discover[42..]).unwrap().kind, 1);
        inject(dma.registers(), 4, &discover, 0);
        service.poll(&mut dma, &mut ring, 600_005).unwrap();
        assert_eq!(arp::parse(&dma.registers().sent[1]).unwrap().2, 0);
        service.poll(&mut dma, &mut ring, 601_005).unwrap();
        let address = check_dhcp(&dma.registers().sent[2], 2, [255; 6]);
        let mut unicast = request(3, Some(address), false);
        unicast[..6].copy_from_slice(&SERVER);
        inject(dma.registers(), 5, &unicast, 0);
        service.poll(&mut dma, &mut ring, 601_006).unwrap();
        assert_eq!(check_dhcp(&dma.registers().sent[3], 5, PEER), address);
        assert_eq!(ring::Dma::status(&mut dma, false, 3).unwrap() >> 31, 0);
        for slot in 0..6 {
            assert_eq!(ring::Dma::status(&mut dma, true, slot), Ok(0x80000800));
        }
    }

    #[cfg(feature = "tx-self-test")]
    #[test]
    fn announces_ten_times_without_rx() {
        let (mut dma, mut ring) = backend(Failure::None);
        let mut service = b51::LanService::new(1);
        for now in (0..=25000).step_by(500) {
            service.poll(&mut dma, &mut ring, now).unwrap();
            let expected = if now == 0 { 0 } else { ((now - 500) / 2000 + 1).min(10) };
            assert_eq!(dma.registers().sent.len() as u64, expected);
        }
        for frame in &dma.registers().sent {
            assert_eq!(frame[..6], [255; 6]);
            assert_eq!(arp::parse(frame), Ok((1, SERVER, IP, IP)));
        }
        assert_eq!(ring::Dma::status(&mut dma, true, 0), Ok(0x80000800));
    }

    #[test]
    fn tx_timeout_leaves_the_nic_owned_descriptor_intact() {
        let (mut dma, mut ring) = backend(Failure::Tx);
        let mut service = b51::LanService::new(1);
        let mut frame = [0; 60];
        arp::request(PEER, IP + 1, IP, &mut frame).unwrap();
        inject(dma.registers(), 0, &frame, 0);
        assert_eq!(service.poll(&mut dma, &mut ring, 100), Ok(()));
        assert_eq!(service.poll(&mut dma, &mut ring, 1100), Err(169));
        assert_eq!(ring::Dma::status(&mut dma, false, 0), Ok(0xb000003c));
    }
}


#[cfg(feature = "lan-port")]
fn status(i: usize) -> &'static std::sync::atomic::AtomicU32 {
    use std::sync::atomic::AtomicU32;
    static PAGE: [AtomicU32; 1024] = [const { AtomicU32::new(0) }; 1024];
    &PAGE[i]
}

#[cfg(feature = "lan-port")]
#[test]
fn pipelined_port_survives_link_flap_and_preserves_stalled_buffers() {
    use std::sync::atomic::Ordering::Relaxed;
    for fail in [Failure::None, Failure::Tx] {
        for i in 0..1024 { status(i).store(0, Relaxed); }
        queue::POPPED.with(|n| n.set(0));
        queue::DOWN_SAMPLES.with(|n| n.set(0));
        let mut model = Model::new(fail);
        let window = dma::InboundWindow { physical: 0x24200000, bus: BUS, size: SIZE as u64 };
        let pool = model.pool;
        let exit = unsafe { b51::prepare(&mut model, IDENTITY, pool, &window) };
        let count = |i| status(i).load(Relaxed);
        if fail == Failure::None {
            assert_eq!(exit, Err(171)); // Injected MAC fault after final completion.
            assert_eq!((count(38), count(39), count(256), count(257)), (112, 112, 0, 63));
            assert!(count(258) > 0 && count(260) > 1 && count(261) > 1);
            assert_eq!(queue::POPPED.with(Cell::get), 112);
        } else {
            assert_eq!(exit, Err(169));
            assert_eq!((count(38), count(39), count(256), count(40), count(259)), (8, 0, 8, 1, 1));
            assert_eq!(queue::POPPED.with(Cell::get), 8);
            assert_eq!(unsafe { *model.descriptor(0x20, 0).0 } & (1 << 31), 1 << 31);
        }
        assert!(queue::DOWN_SAMPLES.with(Cell::get) >= 2);
    }
}
