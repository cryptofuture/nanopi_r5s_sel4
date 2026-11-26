use crate::mac::Registers;
use crate::{
    dma::{DmaMemory, InboundWindow, PackedDma},
    mac,
    pcie::ConfigIo,
    ring::{Identity, Method, Ring, Rx},
};
use crate::{
    pcie::{self, DirectEndpoint, EndpointConfig},
    progress,
    ring::Dma,
    ticks,
};
use core::ptr::NonNull;

#[cfg(not(feature = "lan2"))]
pub const PORT: crate::provider::Lan = crate::provider::Lan::One;
#[cfg(feature = "lan2")]
pub const PORT: crate::provider::Lan = crate::provider::Lan::Two;
pub const BAR: u64 = if cfg!(feature = "lan2") {
    0xf2200000
} else {
    0xf4200000
};
pub const DBI: u64 = if cfg!(feature = "lan2") {
    0x3c0400000
} else {
    0x3c0000000
};
pub const CONFIG: (u64, u64) = (
    if cfg!(feature = "lan2") {
        0xf2000000
    } else {
        0xf4000000
    },
    0x100000,
);
pub const BUSES: (u8, u8) = if cfg!(feature = "lan2") {
    (16, 31)
} else {
    (0, 15)
};
// Host tests compile this module straight from the source tree; the board
// build copies the crate and resolves the firmware through R5S_PROJECT_ROOT.
#[cfg(test)]
const FW: &[u8; 3328] = include_bytes!("../../../firmware/rtl_nic/rtl8125b-2.fw");
#[cfg(not(test))]
const FW: &[u8; 3328] = include_bytes!(concat!(
    env!("R5S_PROJECT_ROOT"),
    "/firmware/rtl_nic/rtl8125b-2.fw"
));
const SLOTS: usize = match option_env!("R5S_B51_RING") {
    Some(value) if cfg!(feature = "lan-port") => match value.as_bytes() {
        b"128" => 128,
        b"256" => 256,
        b"512" => 512,
        _ => 64,
    },
    _ => 64,
};
const WIDE_RING: bool = SLOTS >= 256;
const RX_QUEUES: usize = match option_env!("R5S_B51_RX_QUEUES") {
    Some(value) if cfg!(feature = "lan-port") => match value.as_bytes() {
        b"2" => 2,
        b"4" => 4,
        _ => 1,
    },
    _ => 1,
};
const TX_QUEUES: usize = match option_env!("R5S_B51_TX_QUEUES") {
    Some(value) if cfg!(feature = "lan-port") && matches!(value.as_bytes(), b"2") => 2,
    _ => 1,
};
const DESC_BYTES: usize = crate::dma::tx_rx_descriptor_bytes(SLOTS, EXTENDED_TX, RX_QUEUES, TX_QUEUES);
const BUFFER_OFFSET: usize = DESC_BYTES.next_multiple_of(4096);
const _: () = assert!(BUFFER_OFFSET + BUFFER_BYTES <= if WIDE_RING { 0x300000 } else { 0x100000 });
const EXTENDED_TX: bool = cfg!(feature = "tx-no-close");
type B51Dma<I> = PackedDma<I, SLOTS, EXTENDED_TX, RX_QUEUES, TX_QUEUES>;
const BUFFER_BYTES: usize = SLOTS * (RX_QUEUES + TX_QUEUES) * crate::ring::BUFFER;
const OWN: u32 = 1 << 31;
const RSS_CTRL: u64 = 0x4500;
const RX_DESC_V3: u32 = 1 << 24;
#[cfg(feature = "lan-port")]
fn rx_checksum_requested() -> bool {
    let setting=if cfg!(feature="lan2") {option_env!("R5S_LAN2_RX_CHECKSUM")}
        else {option_env!("R5S_LAN1_RX_CHECKSUM")};
    setting==Some("1")
}
const CPCMD_MASK: u16 = 0x2063;

#[unsafe(no_mangle)]
static mut LAN1_VA: usize = 0;
#[unsafe(no_mangle)]
static mut LAN1_PA: u64 = 0;

pub fn window() -> Result<(NonNull<u8>, InboundWindow), u32> {
    let va = unsafe { (&raw const LAN1_VA).read_volatile() };
    let private = unsafe { (&raw const LAN1_PA).read_volatile() };
    #[cfg(feature = "zc")]
    let wide_placed = crate::zc::WIDE_PLACED;
    #[cfg(not(feature = "zc"))]
    let wide_placed = false;
    let (expected, physical, bus, size) = match (cfg!(feature = "lan2"), WIDE_RING) {
        (false, true) if wide_placed => (0x24200000, 0x24200000, 0x80000000, 0x500000),
        (true, true) if wide_placed => (0x24800000, 0x24600000, 0x81000000, 0x500000),
        (false, false) => (0x24200000, 0x24200000, 0x80000000, 0x400000),
        (true, false) => (0x24300000, 0x24300000, 0x81000000, 0x300000),
        (false, true) => (0x24200000, 0x24200000, 0x80000000, 0x400000),
        (true, true) => (0x24600000, 0x24500000, 0x81000000, 0x400000),
    };
    #[cfg(feature = "zc")]
    let size = if wide_placed && !cfg!(feature = "lan2") {
        size.max(crate::zc::POOL_PA + crate::zc::POOL_BYTES as u64 - physical)
    } else {size};
    #[cfg(feature = "zc")]
    if (InboundWindow {physical,bus,size}).translate(crate::zc::POOL_PA,crate::zc::POOL_BYTES as u64).is_none() {
        return Err(160);
    }
    if va != 0x62000000 || private != expected {
        return Err(160);
    }
    Ok((
        NonNull::new(va as *mut u8).ok_or(160u32)?,
        InboundWindow { physical, bus, size },
    ))
}

struct Controller<'a, I> {
    io: &'a mut I,
}

pub(crate) fn configure_rss<I: Registers>(io: &mut I, queues: usize) -> Result<(), u32> {
    if !matches!(queues, 2 | 4) { return Err(166); }
    const KEY: [u32; 10] = [
        0xda565a6d, 0xc20e5b25, 0x3d256741, 0xb08fa343, 0xcb2bcad0,
        0xb4307bae, 0xa32dcb77, 0x0cf23080, 0x3bb7426a, 0xfa01acbe,
    ];
    let mut write = |offset, value| -> Result<(), u32> {
        io.write32(BAR + offset, value).map_err(|_| 166u32)?;
        if io.read32(BAR + offset).map_err(|_| 166u32)? != value { return Err(166); }
        Ok(())
    };
    for (index, key) in KEY.into_iter().enumerate() {
        write(0x4600 + index as u64 * 4, key)?;
    }
    for index in 0..32 {
        let table = core::array::from_fn(|slot| ((index * 4 + slot) % queues) as u8);
        write(0x4700 + index as u64 * 4, u32::from_le_bytes(table))?;
    }
    let mode = queues.ilog2();
    let old = io.read16(BAR + 0x4800).map_err(|_| 166u32)?;
    let queue_control = (old & !0x1c) | (mode as u16) << 2;
    io.write16(BAR + 0x4800, queue_control).map_err(|_| 166u32)?;
    if io.read16(BAR + 0x4800).map_err(|_| 166u32)? & 0x1c != queue_control & 0x1c {
        return Err(166);
    }
    let control = mode << 16 | 7 << 8 | 3;
    io.write32(BAR + RSS_CTRL, control).map_err(|_| 166u32)?;
    if io.read32(BAR + RSS_CTRL).map_err(|_| 166u32)? != control { return Err(166); }
    Ok(())
}

impl<I: Registers> Controller<'_, I> {
    fn read(&mut self, offset: u64) -> Result<u32, u32> {
        self.io.read32(BAR + offset).map_err(|_| 163u32)
    }

    fn write(&mut self, offset: u64, value: u32) -> Result<(), u32> {
        self.io.write32(BAR + offset, value).map_err(|_| 163u32)
    }

    fn ocp(&mut self, mac: bool, address: u32, value: Option<u16>) -> Result<u16, u32> {
        if address > 0xfffe || address & 1 != 0 {
            return Err(164);
        }
        let port = if mac { 0xb0 } else { 0xb8 };
        self.write(
            port,
            address << 15 | value.map_or(0, |v| OWN | u32::from(v)),
        )?;
        if mac {
            return Ok(self.read(port)? as u16);
        }
        for _ in 0..25 {
            self.io.delay_us(10);
            let result = self.read(port)?;
            if (result & OWN != 0) == value.is_none() {
                return Ok(result as u16);
            }
        }
        Err(164)
    }

    fn firmware(&mut self) -> Result<(), u32> {
        if FW[..4] != [0; 4]
            || FW.iter().fold(0u8, |sum, x| sum.wrapping_add(*x)) != 0
            || FW[36..44] != [112, 0, 0, 0, 32, 3, 0, 0]
        {
            return Err(164);
        }
        let mut pc = 0usize;
        let mut mac = false;
        let mut page = 0xa400u32;
        let mut previous = 0u16;
        let mut reads = 0;
        for _ in 0..4096 {
            if pc == 800 {
                return Ok(());
            }
            if pc > 800 {
                return Err(164);
            }
            let pos = 112 + 4 * pc;
            let action = u32::from_le_bytes(FW[pos..pos + 4].try_into().unwrap());
            let op = action >> 28;
            let reg = (action >> 16) & 0xfff;
            let data = action as u16;
            pc += 1;
            match op {
                0 | 8 | 12 => {
                    let write = match op {
                        8 => Some(data),
                        12 => Some(previous),
                        _ => None,
                    };
                    if reg == 31 {
                        if let Some(v) = write {
                            page = if !mac && v == 0 {
                                0xa400
                            } else {
                                u32::from(v) << 4
                            };
                        } else {
                            return Err(164);
                        }
                    } else {
                        let offset = if mac {
                            reg
                        } else if page == 0xa400 {
                            reg * 2
                        } else {
                            reg.checked_sub(16).ok_or(164u32)? * 2
                        };
                        let result = self.ocp(mac, page + offset, write)?;
                        if write.is_none() {
                            previous = result;
                            reads += 1;
                        }
                    }
                }
                1 => previous |= data,
                2 => previous &= data,
                3 => pc = pc.checked_sub(reg as usize + 1).ok_or(164u32)?,
                4 if data <= 1 => mac = data != 0,
                7 => reads = 0,
                9 if reads == data => return Err(164), // The pinned program exhausted its PHY patch handshake retries.
                9 => (),
                10 if previous == data => pc += reg as usize,
                10 => (),
                14 => self.io.delay_us(u32::from(data) * 1000),
                _ => return Err(164),
            }
        }
        Err(164)
    }

    fn modify(&mut self, mac: bool, reg: u32, clear: u16, set: u16) -> Result<(), u32> {
        let old = self.ocp(mac, reg, None)?;
        self.ocp(mac, reg, Some((old & !clear) | set))?;
        Ok(())
    }

    fn phy(&mut self) -> Result<(), u32> {
        for (reg, clear, set) in [
            (0xa442, 0, 0x0800),
            (0xac46, 0x00f0, 0x0090),
            (0xad30, 3, 1),
        ] {
            self.modify(false, reg, clear, set)?;
        }
        for (index, value) in [(0x80f5, 0x760e), (0x8107, 0x360e)] {
            self.ocp(false, 0xb87c, Some(index))?;
            self.ocp(false, 0xb87e, Some(value))?;
        }
        self.ocp(false, 0xb87c, Some(0x8551))?;
        self.modify(false, 0xb87e, 0xff00, 0x0800)?;
        self.modify(false, 0xbf00, 0xe000, 0xa000)?;
        self.modify(false, 0xbf46, 0x0f00, 0x0300)?;
        for index in (0x8044..=0x807a).step_by(6) {
            self.ocp(false, 0xa436, Some(index))?;
            self.ocp(false, 0xa438, Some(0x2417))?;
        }
        for (reg, clear, set) in [
            (0xa4ca, 0, 0x40),
            (0xbf84, 0xe000, 0xa000),
            (0xa5b4, 0x8000, 0),
            (0xa430, 4, 0),
            (0xa6d4, 1, 0),
            (0xa6d8, 0x10, 0),
            (0xa424, 0x80, 0),
            (0xa4a2, 0x200, 0),
        ] {
            self.modify(false, reg, clear, set)?;
        }
        self.modify(false, 0xa400, 0x0c00, 0x8000)?;
        for _ in 0..60 {
            self.io.delay_us(10000);
            if self.ocp(false, 0xa400, None)? & 0x8000 == 0 {
                return self.autonegotiate();
            }
        }
        Err(163)
    }

    fn autonegotiate(&mut self) -> Result<(), u32> {
        self.modify(false, 0xa5d4, 0x0180, 0x0080)?;
        self.modify(false, 0xa408, 0x1e0, 0x141)?;
        self.modify(false, 0xa412, 0x300, 0x200)?;
        self.modify(false, 0xa400, 0x0c00, 0x1200)?;
        Ok(())
    }

    fn mac_setup(&mut self) -> Result<(), u32> {
        self.io.write8(BAR + 0x50, 0xc0).map_err(|_| 163u32)?;
        self.modify(true, 0xe092, 0xff, 0)?;
        for (reg, clear) in [(0x53, 0x80), (0x56, 1), (0x54, 2), (0x52, 0x10)] {
            let val = self.io.read8(BAR + reg).map_err(|_| 163u32)?;
            self.io
                .write8(BAR + reg, val & !clear)
                .map_err(|_| 163u32)?;
        }
        self.io.write8(BAR + 0x34, 0).map_err(|_| 163u32)?;
        for offset in (0xa00..0xa80).step_by(4) {
            self.write(offset, 0)?;
        }
        for (offset, value) in [(0x6048, 1546), (0x7a, 0), (0x382, 0x221b), (0x4800, 0)] {
            self.io.write16(BAR + offset, value).map_err(|_| 163u32)?;
        }
        for (index, value) in [
            (0x0b, 0xa908),
            (0x1e, 0x20eb),
            (0x22, 0x0023),
            (2, 0x60c2),
            (0x29, 0xff00),
            (0x4b, 0xa908),
            (0x5e, 0x28eb),
            (0x62, 0x0023),
            (0x42, 0x60c2),
            (0x69, 0xff00),
        ] {
            self.write(0x80, OWN | (index << 16) | value)?;
            let mut done = false;
            for _ in 0..100 {
                self.io.delay_us(10);
                if self.read(0x80)? & OWN == 0 {
                    done = true;
                    break;
                }
            }
            if !done {
                return Err(163);
            }
            self.io.delay_us(10);
        }
        self.modify(true, 0xc0b6, 0xfffe, 0)?;
        self.modify(true, 0xea84, 0, 3)?;
        for (reg, clear, set) in [
            (0xd40a, 0x10, 0),
            (0xc140, 0xffff, 0xffff),
            (0xc142, 0xffff, 0xffff),
            (0xd3e2, 0xfff, 0x3a9),
            (0xd3e4, 0xff, 0),
            (0xe860, 0, 0x80),
            (0xeb58, 1, u16::from(EXTENDED_TX)),
            (0xe614, 0x700, 0x200),
            (0xe63e, 0xc30, 0x20),
            (0xc0b4, 1, 0),
            (0xc0b4, 0, 1),
            (0xc0b4, 0, 0xc),
            (0xeb6a, 0xff, 0x33),
            (0xeb50, 0x3e0, 0x40),
            (0xe056, 0xf0, 0),
        ] {
            self.modify(true, reg, clear, set)?;
        }
        self.io.write8(BAR + 0x57, 0x10).map_err(|_| 163u32)?;
        for (reg, clear, set) in [
            (0xe040, 0x1000, 0),
            (0xea1c, 3, 1),
            (0xea1c, 4, 0),
            (0xe0c0, 0xffff, 0x4000),
            (0xe052, 0x88, 0x60),
            (0xd430, 0xfff, 0x45f),
            (0xeb54, 0, 1),
        ] {
            if reg == 0xe0c0 {
                self.ocp(true, reg, Some(set))?;
            } else {
                self.modify(true, reg, clear, set)?;
            }
        }
        let d0 = self.io.read8(BAR + 0xd0).map_err(|_| 163u32)?;
        self.io.write8(BAR + 0xd0, d0 | 0xc0).map_err(|_| 163u32)?;
        self.modify(true, 0xe080, 2, 0)?;
        for (reg, mask, value) in [
            (0xe63e, 0x0c30, 0x0020), (0xe0c0, 0xffff, 0x4000),
            (0xe052, 0x00e8, 0x0060), (0xe056, 0x00f0, 0),
            (0xd430, 0x0fff, 0x045f), (0xea84, 3, 3),
            (0xc0b4, 0x0d, 0x0d), (0xe080, 2, 0), (0xe614, 0x700, 0x200),
        ] {
            if self.ocp(true, reg, None)? & mask != value {
                return Err(163);
            }
        }
        self.io.delay_us(1);
        self.modify(true, 0xeb54, 1, 0)?;
        let v = self.io.read16(BAR + 0x1880).map_err(|_| 163u32)?;
        self.io
            .write16(BAR + 0x1880, v & !0x30)
            .map_err(|_| 163u32)?;
        self.ocp(true, 0xe098, Some(0xc302))?;
        for _ in 0..10 {
            if self.ocp(true, 0xe00e, None)? & 0x2000 == 0 {
                self.modify(true, 0xe040, 0, 3)?;
                self.modify(true, 0xc0ac, 0, 0x1f80)?;
                let value = self.read(0xf0)?;
                return self.write(0xf0, value & !(1 << 19));
            }
            self.io.delay_us(1000);
        }
        Err(163)
    }

    fn fifo_ready(&mut self) -> Result<(), u32> {
        for _ in 0..42 {
            if self.io.read8(BAR + 0xd3).map_err(|_| 163u32)? & 2 != 0 {
                return Ok(());
            }
            self.io.delay_us(100);
        }
        Err(163)
    }
}

pub unsafe fn prepare<I: Registers>(
    io: &mut I,
    identity: Identity,
    va: NonNull<u8>,
    window: &InboundWindow,
) -> Result<(), u32> {
    let mut chip = Controller { io };
    if identity.supported_method() != Some(Method::B51)
        || mac::reset(chip.io, BAR, identity.pci_id).map_err(|_| 163u32)? != Method::B51
    {
        return Err(163);
    }
    #[cfg(feature = "tso")]
    crate::status(274).store(
        u32::from(identity.supported_method() == Some(Method::B51)),
        core::sync::atomic::Ordering::Relaxed,
    );
    progress(5);
    let mcu = chip.io.read8(BAR + 0xd3).map_err(|_| 163u32)?;
    chip.io
        .write8(BAR + 0xd3, mcu & !0x80)
        .map_err(|_| 163u32)?;
    let oob = chip.ocp(true, 0xe8de, None)?;
    chip.ocp(true, 0xe8de, Some(oob & !(1 << 14)))?;
    chip.fifo_ready()?;
    for (address, value) in [(0xc0aa, 0x07d0), (0xc0a6, 0x0150), (0xc01e, 0x5555)] {
        chip.ocp(true, address, Some(value))?;
    }
    chip.fifo_ready()?;
    let power = chip.io.read8(BAR + 0x6f).map_err(|_| 163u32)?;
    chip.io
        .write8(BAR + 0x6f, power | 0xc0)
        .map_err(|_| 163u32)?;
    chip.modify(false, 0xa400, 0x0800, 0)?;
    #[cfg(feature = "supervised")]
    crate::phase(4);
    chip.firmware()?;
    chip.ocp(false, 0xa436, Some(0x801e))?;
    if chip.ocp(false, 0xa438, None)? != 0x16 {
        return Err(164);
    }
    let deadline = ticks();
    while chip.ocp(false, 0xa400, None)? & 0x8000 != 0 {
        if ticks().wrapping_sub(deadline) >= 14_400_000 {
            return Err(164);
        }
        chip.io.delay_us(1000);
    }
    #[cfg(feature = "supervised")]
    crate::phase(5);
    chip.phy().map_err(|_| 163u32)?;
    progress(6);
    if mac::reset(chip.io, BAR, identity.pci_id).map_err(|_| 163u32)? != Method::B51 {
        return Err(163);
    }
    let cplus = chip.io.read16(BAR + 0xe0).map_err(|_| 166u32)?;
    #[cfg(feature="lan-port")]
    let cplus = cplus | if rx_checksum_requested() {1<<5} else {0};
    chip.io.write16(BAR + 0xe0, cplus & CPCMD_MASK).map_err(|_| 166u32)?;
    chip.mac_setup().map_err(|_| 163u32)?;
    let start = ticks();
    loop {
        if chip.ocp(false, 0xa402, None)? & 0x24 == 0x24
            && chip.io.read8(BAR + 0x6c).map_err(|_| 163u32)? & 2 != 0
        {
            break;
        }
        if ticks().wrapping_sub(start) > 24_000_000 * 90 {
            return Err(165);
        }
        chip.io.delay_us(1000);
    }
    #[cfg(feature = "lan-dhcp")]
    crate::status(6).store(
        chip.io.read16(BAR + 0x6c).map_err(|_| 165u32)?.into(),
        core::sync::atomic::Ordering::Release,
    );
    progress(7);
    #[cfg(feature = "supervised")]
    crate::phase(12);
    let mut mac = [0; 6];
    for source in [0x19e0, 0] {
        for (i, byte) in mac.iter_mut().enumerate() {
            *byte = chip.io.read8(BAR + source + i as u64).map_err(|_| 163u32)?;
        }
        if mac != [0; 6] && mac[0] & 1 == 0 {
            break;
        }
    }
    #[cfg(feature = "lan-dhcp")]
    {
        mac = [
            2,
            0x52,
            0x35,
            0x53,
            0,
            if cfg!(feature = "lan2") { 3 } else { 2 },
        ];
    }
    if mac[0] & 1 != 0 || mac == [0; 6] {
        return Err(163);
    }
    for (offset, value) in [
        (4, u32::from(u16::from_le_bytes([mac[4], mac[5]]))),
        (0, u32::from_le_bytes(mac[..4].try_into().unwrap())),
    ] {
        chip.write(offset, value)?;
        let mask = if offset == 4 { 0xffff } else { u32::MAX };
        if chip.read(offset)? & mask != value {
            return Err(163);
        }
    }
    if chip.ocp(true, 0xeb58, None)? & 1 != u16::from(EXTENDED_TX)
        || chip.ocp(true, 0xe63e, None)? & 0xc00 != 0
        || chip.io.read16(BAR + 0x4800).map_err(|_| 163u32)? & 0x1c != 0
    {
        return Err(163);
    }
    let va = va.as_ptr() as usize;
    let private = if WIDE_RING && cfg!(feature = "lan2") {
        if window.size == 0x500000 { 0x24800000 } else { 0x24600000 }
    } else { window.physical };
    let memory = |offset: usize, size| -> Result<DmaMemory, u32> {
        let cpu = va.checked_add(offset).ok_or(160u32)?;
        let physical = private.checked_add(offset as u64).ok_or(160u32)?;
        cpu.checked_add(size).ok_or(160u32)?;
        physical.checked_add(size as u64).ok_or(160u32)?;
        Ok(DmaMemory {
            cpu: NonNull::new(cpu as *mut u8).ok_or(160u32)?,
            physical,
            size,
        })
    };
    let mut dma = unsafe {
        B51Dma::<_>::from_verified_memory(
            chip.io,
            BAR,
            memory(0, DESC_BYTES)?,
            memory(BUFFER_OFFSET, BUFFER_BYTES)?,
            window,
        )
    }
    .map_err(|_| 160u32)?;
    unsafe { dma.clear_after_stop() };
    let (tx, rx, buffers) = dma.bus_addresses();
    let mut ring = Ring::<SLOTS>::new(buffers, BUFFER_BYTES as u64).map_err(|_| 166u32)?;
    let (_, rx_payload) = dma.rx_queue_addresses(0).ok_or(166u32)?;
    ring = ring.with_private_rx(rx_payload).map_err(|_| 166u32)?;
    ring.initialize(&mut dma).map_err(|_| 166u32)?;
    #[cfg(feature = "lan-port")]
    let mut secondary_tx = if TX_QUEUES == 2 {
        let payload_offset = SLOTS * crate::ring::BUFFER;
        let mut secondary = Ring::<SLOTS>::new(buffers + payload_offset as u64,
            (BUFFER_BYTES - payload_offset) as u64).map_err(|_| 166u32)?;
        dma.select_tx_queue(1).map_err(|_| 166u32)?;
        secondary.initialize_tx(&mut dma).map_err(|_| 166u32)?;
        dma.select_tx_queue(0).map_err(|_| 166u32)?;
        let descriptor = dma.tx_queue_address(1).ok_or(166u32)?;
        let mut chip = Controller { io: dma.registers() };
        for (offset, value) in [(0x2104, (descriptor >> 32) as u32), (0x2100, descriptor as u32)] {
            chip.write(offset, value)?;
            if chip.read(offset)? != value { return Err(166); }
        }
        let control = chip.ocp(true, 0xe63e, None)? | (1 << 10);
        chip.ocp(true, 0xe63e, Some(control))?;
        if chip.ocp(true, 0xe63e, None)? & 0xc00 != 0x400 { return Err(166); }
        Some(secondary)
    } else { None };
    let mut rss_rings = [const { None }; RX_QUEUES - 1];
    for (index, entry) in rss_rings.iter_mut().enumerate() {
        let queue = index + 1;
        let (descriptor, payload) = dma.rx_queue_addresses(queue).ok_or(166u32)?;
        let mut receive = Ring::<SLOTS>::new(buffers, BUFFER_BYTES as u64)
            .and_then(|ring| ring.with_private_rx(payload)).map_err(|_| 166u32)?;
        dma.select_rx_queue(queue).map_err(|_| 166u32)?;
        receive.initialize_rx(&mut dma).map_err(|_| 166u32)?;
        *entry = Some(receive);
        let base = BAR + 0x4000 + index as u64 * 8;
        let regs = dma.registers();
        for (offset, value) in [(4, (descriptor >> 32) as u32), (0, descriptor as u32)] {
            regs.write32(base + offset, value).map_err(|_| 166u32)?;
            if regs.read32(base + offset).map_err(|_| 166u32)? != value { return Err(166); }
        }
    }
    dma.select_rx_queue(0).map_err(|_| 166u32)?;
    let regs = dma.registers();
    for (offset, value) in [(0x24, (tx >> 32) as u32), (0x20, tx as u32), (0xe8, (rx >> 32) as u32), (0xe4, rx as u32)] {
        regs.write32(BAR + offset, value).map_err(|_| 166u32)?;
        if regs.read32(BAR + offset).map_err(|_| 166u32)? != value {
            return Err(166);
        }
    }
    regs.write16(BAR + 0xda, 1518).map_err(|_| 166u32)?;
    regs.write32(BAR + RSS_CTRL, 0).map_err(|_| 166u32)?;
    regs.write8(BAR + 0x50, 0).map_err(|_| 166u32)?;
    if regs.read16(BAR + 0xda).map_err(|_| 166u32)? != 1518
        || regs.read8(BAR + 0x37).map_err(|_| 166u32)? & 0x1c != 0
    {
        return Err(166);
    }
    if regs.read16(BAR + 0xe0).map_err(|_| 166u32)? & CPCMD_MASK != cplus & CPCMD_MASK
    {
        return Err(166);
    }
    regs.write32(BAR + 0x3c, u32::MAX).map_err(|_| 166u32)?;
    if regs.read32(BAR + 0x38).map_err(|_| 166u32)? != 0 {
        return Err(166);
    }
    if RX_QUEUES > 1 { configure_rss(regs, RX_QUEUES)?; }
    progress(8);
    #[cfg(all(feature = "lan-port", not(test)))]
    {
        let snapshot = |regs: &mut _| pcie::link_snapshot(|offset|
            ConfigIo::read32(regs, DBI + u64::from(offset)).ok());
        let mps = option_env!("R5S_B51_MPS").and_then(|value| value.parse().ok());
        let mrrs = option_env!("R5S_B51_MRRS").and_then(|value| value.parse().ok());
        let mut root = snapshot(regs);
        let mut root_change = None;
        if mps == Some(256) && root.is_some_and(|v| (v >> 16) & 7 >= 1 && (v >> 19) & 7 == 0) {
            let mut endpoint = DirectEndpoint { io: regs, dbi: DBI, config_aperture: CONFIG, buses: BUSES };
            let peer = pcie::link_snapshot(|offset| endpoint.read_dword(offset).ok());
            if peer.zip(root).is_some_and(|(peer, root)| (peer >> 16) & 7 >= 1
                && peer & 0x3ff == root & 0x3ff)
                && let Some(cap) = pcie::conventional_capability(|offset|
                    ConfigIo::read32(regs, DBI + u64::from(offset)).ok(), 0x10).filter(|cap| *cap <= 0xec)
            {
                let address = DBI + u64::from(cap + 8);
                let original = ConfigIo::read32(regs,address).map_err(|_|173u32)? & 0xffff;
                let requested = (original & !(7 << 5)) | (1 << 5);
                regs.write32(address,requested).map_err(|_|173u32)?;
                root = snapshot(regs);
                if root.is_some_and(|v| (v >> 19) & 7 == 1) {
                    root_change = Some((address, original));
                } else {
                    regs.write32(address,original).map_err(|_|173u32)?;
                    root = snapshot(regs);
                }
            }
        }
        let mut endpoint = DirectEndpoint { io: regs, dbi: DBI, config_aperture: CONFIG, buses: BUSES };
        if mps.is_some() || mrrs.is_some() {
            let supported_mps = mps.filter(|bytes: &u16| root.is_some_and(|v| *bytes <= (128u16 << ((v >> 19) & 7))));
            if let Err(error) = pcie::tune_endpoint_transactions(&mut endpoint, root.unwrap_or(0), supported_mps, mrrs) {
                if matches!(error, pcie::TransactionError::Io(_)) {return Err(173);}
                if let Some((address,original)) = root_change {
                    regs.write32(address,original).map_err(|_|173u32)?;
                    root = snapshot(regs);
                    endpoint = DirectEndpoint { io: regs, dbi: DBI, config_aperture: CONFIG, buses: BUSES };
                }
            }
        }
        let endpoint = pcie::link_snapshot(|offset| endpoint.read_dword(offset).ok());
        crate::status(277).store(endpoint.unwrap_or(0), core::sync::atomic::Ordering::Relaxed);
        crate::status(278).store(root.unwrap_or(0), core::sync::atomic::Ordering::Relaxed);
    }
    regs.write32(DBI + 4, 6).map_err(|_| 173u32)?;
    if regs.read32(DBI + 4).map_err(|_| 173u32)? & 7 != 6 {
        return Err(173);
    }
    pcie::set_endpoint_command(*regs, DBI, CONFIG, BUSES, 6).map_err(|_| 173u32)?;
    progress(9);
    let tx_config = 0x03000700 | if cfg!(feature = "tx-no-close") { 1 << 6 } else { 0 };
    regs.write32(BAR + 0x40, tx_config).map_err(|_| 166u32)?;
    let rx_config = 0x40000f0a
        | if RX_QUEUES > 1 { RX_DESC_V3 } else { 0 }
        | u32::from(cfg!(feature = "lan-bridge") || cfg!(all(feature = "lan2", feature = "lan-port")));
    regs.write32(BAR + 0x44, rx_config).map_err(|_| 166u32)?;
    if regs.read32(BAR + 0x40).map_err(|_| 166u32)? & 0x03000740 != tx_config
        || regs.read32(BAR + 0x44).map_err(|_| 166u32)? & 0x79c00f3f != rx_config
    {
        return Err(166);
    }
    let engines = if cfg!(feature = "tx-diagnostic") { 0x04 } else { 0x0c };
    regs.write8(BAR + 0x37, engines).map_err(|_| 171u32)?;

    if regs.read8(BAR + 0x37).map_err(|_| 171u32)? & 0x1c != engines {
        return Err(171);
    }
    #[cfg(feature = "tx-no-close")]
    for queue in (0..TX_QUEUES).rev() {
        dma.select_tx_queue(queue).map_err(|_| 166u32)?;
        dma.enable_no_close().map_err(|_| 166u32)?;
    }
    #[cfg(feature = "tx-diagnostic")]
    {
        const TALLY_OFFSET: usize = 0xff000;
        let tally_bus = window.translate(window.physical.checked_add(TALLY_OFFSET as u64)
            .ok_or(160u32)?, 4096).ok_or(160u32)?;
        for (i, v) in [va as u32, window.physical as u32, tx as u32, buffers as u32].into_iter().enumerate() {
            record(28 + i, v);
        }
        for (i, v) in [(va + 4096) as u32, window.physical as u32 + 4096, tally_bus as u32 + 256,
            tally_bus as u32 + 2304, window.physical as u32 + 0xff100, window.physical as u32 + 0xff900].into_iter().enumerate() {
            record(240 + i, v);
        }
        return tx_diagnostic(&mut dma, &mut ring, { va.checked_add(TALLY_OFFSET + 4096).ok_or(160u32)?; (va + TALLY_OFFSET) as *mut u32 }, tally_bus);
    }
    #[cfg(feature = "lan-port")]
    return serve(&mut dma, &mut ring, &mut rss_rings, secondary_tx.as_mut(), identity);
    #[cfg(all(feature = "lan-dhcp", not(feature = "tx-diagnostic"), not(feature = "lan-port")))]
    return serve(&mut dma, &mut ring);
    #[cfg(not(feature = "lan-dhcp"))]
    {
        let mut received = false;
        let mut frame = [0u8; 1514];
        let start = ticks();
        loop {
            if ticks().wrapping_sub(start) > 24_000_000u64 * 300 {
                return Err(if received { 172 } else { 168 });
            }
            if dma.registers().read32(BAR + 0x3c).map_err(|_| 171u32)? & 0x8000 != 0 {
                return Err(171);
            }
            if let Rx::Frame(len) = ring
                .receive(&mut dma, &mut frame, 1 << 21)
                .map_err(|_| 171u32)?
            {
                received = true;
                progress(10);
                if let Some(reply) = crate::arp::reply(&frame[..len], mac) {
                    progress(11);
                    ring.send(&mut dma, &reply).map_err(|_| 171u32)?;
                    progress(12);
                    let sent = ticks();
                    while dma.status(false, 0).map_err(|_| 169u32)? & OWN != 0 {
                        if ticks().wrapping_sub(sent) > 24_000_000 {
                            return Err(169);
                        }
                    }
                    if dma.registers().read32(BAR + 0x3c).map_err(|_| 171u32)? & 0x8000 != 0 {
                        return Err(171);
                    }
                    progress(13);
                    return Ok(());
                }
            }
        }
    }
}

pub fn shutdown<I: Registers>(io: &mut I, identity: Identity) -> bool {
    let drained = stop_and_drain(io, identity);
    let isolated = pcie::set_endpoint_command(io, DBI, CONFIG, BUSES, 0).is_ok();
    let root_off = io.write32(DBI + 4, 0).is_ok() && io.read32(DBI + 4).is_ok_and(|v| v & 7 == 0);
    drained && isolated && root_off
}

fn stop_and_drain<I: Registers>(io: &mut I, identity: Identity) -> bool {
    let stopped = identity.supported_method() == Some(Method::B51)
        && mac::reset(io, BAR, identity.pci_id).is_ok();
    let accessible = pcie::set_endpoint_command(io, DBI, CONFIG, BUSES, 2).is_ok();
    let drained = accessible && transactions_drained(io);
    stopped && drained
}

fn transactions_drained<I: ConfigIo>(io: &mut I) -> bool {
    let mut ep = DirectEndpoint {
        io,
        dbi: DBI,
        config_aperture: CONFIG,
        buses: BUSES,
    };
    let Some(cap) = pcie::conventional_capability(|offset| ep.read_dword(offset).ok(), 0x10) else {
        return false;
    };
    if cap > 0xf4 { return false; }
    for _ in 0..100 {
        match ep.read_dword(cap + 8) {
            Ok(v) if v & (1 << 21) == 0 => return true,
            Ok(_) => ep.io.delay_us(1000),
            Err(_) => return false,
        }
    }
    false
}

#[cfg(feature = "lan-dhcp")]
pub struct LanService {
    server: r5s_router_core::ethernet::EthernetRouter,
    frame: [u8; 1514],
    reply: [u8; 1514],
    tx_slot: usize,
    pending: Option<(u64, u8)>,
    deferred_rx: Option<usize>,
    #[cfg(feature = "tx-self-test")]
    announcement: (u8, Option<u64>),
}

#[cfg(feature = "lan-dhcp")]
impl LanService {
    pub fn new(seed: u32) -> Self {
        Self {
            server: r5s_router_core::ethernet::EthernetRouter::new(seed),
            frame: [0; 1514],
            reply: [0; 1514],
            tx_slot: 0,
            pending: None,
            deferred_rx: None,
            #[cfg(feature = "tx-self-test")]
            announcement: (0, None),
        }
    }

    pub fn poll(&mut self, dma: &mut impl Dma, ring: &mut Ring<SLOTS>, now: u64) -> Result<(), u32> {
        self.complete_tx(dma, now)?;
        use r5s_router_core::Port;
        let port = if cfg!(feature = "lan2") { Port::Lan2 } else { Port::Lan1 };
        let rx = match self.deferred_rx {
            Some(len) if self.pending.is_none() => {
                self.deferred_rx = None;
                Rx::Frame(len)
            }
            Some(_) => return Ok(()),
            None => {
                let rx = ring.receive(dma, &mut self.frame, 0xf << 19).map_err(|_| 171u32)?;
                if matches!(rx, Rx::Frame(_)) {
                    progress(10);
                }
                rx
            }
        };
        #[cfg(feature = "supervised")]
        crate::phase(7);
        let output = match rx {
            Rx::Frame(len) => {
                if self.pending.is_some() {
                    self.deferred_rx = Some(len);
                    return Ok(());
                }
                let frame = &self.frame[..len];
                let result = self.server.receive(port, frame, now, &mut self.reply);
                if result.is_ok() {
                    match dhcp_kind(frame, false) {
                        Some(1) => progress(14),
                        Some(3) => progress(18),
                        _ => {}
                    }
                    if frame.get(12..14) == Some(&[8, 6]) && result.as_ref().is_ok_and(Option::is_some) {
                        progress(11);
                    }
                }
                result.ok().flatten()
            }
            Rx::Rejected => {
                progress(19);
                None
            }
            Rx::Pending | Rx::External(..) => None,
        };
        if self.pending.is_some() {
            return Ok(());
        }
        let output = output.or_else(|| self.server.tick_lan(port, now, &mut self.reply).ok().flatten());
        #[cfg(feature = "supervised")]
        crate::phase(6);
        if self.pending.is_none() {
            let packet = output.filter(|(egress, _)| *egress == port).map(|(_, len)| {
                (&self.reply[..len], dhcp_kind(&self.reply[..len], true).unwrap_or(0))
            });
            #[cfg(feature = "tx-self-test")]
            let packet = packet.or_else(|| {
                let (attempts, due) = &mut self.announcement;
                let deadline = *due.get_or_insert_with(|| now.saturating_add(500));
                if *attempts == 10 || now < deadline {
                    return None;
                }
                *attempts += 1;
                *due = Some(now.saturating_add(2000));
                Some((&ANNOUNCEMENT[..], 255))
            });
            if let Some((frame, kind)) = packet {
                ring.send(dma, frame).map_err(|_| 171u32)?;
                progress(if kind == 255 { 21 } else { 12 });
                self.pending = Some((now, kind));
            }
        }
        self.complete_tx(dma, now)
    }

    fn complete_tx(&mut self, dma: &mut impl Dma, now: u64) -> Result<(), u32> {
        if let Some((start, kind)) = self.pending {
            if dma.status(false, self.tx_slot).map_err(|_| 169u32)? & OWN != 0 {
                if now.saturating_sub(start) >= 1000 {
                    if kind == 255 {
                        progress(23);
                    }
                    return Err(169);
                }
                return Ok(());
            }
            self.pending = None;
            self.tx_slot = (self.tx_slot + 1) % SLOTS;
            progress(if kind == 255 { 22 } else { 13 });
            match kind {
                2 => progress(17),
                5 => progress(15),
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(any(feature = "tx-self-test", feature = "tx-diagnostic"))]
const ANNOUNCE_MAC_END: u8 = if cfg!(feature = "lan2") { 3 } else { 2 };
#[cfg(any(feature = "tx-self-test", feature = "tx-diagnostic"))]
const ANNOUNCEMENT: [u8; 42] = [
    255, 255, 255, 255, 255, 255, 2, 0x52, 0x35, 0x53, 0, ANNOUNCE_MAC_END, 8, 6,
    0, 1, 8, 0, 6, 4, 0, 1, 2, 0x52, 0x35, 0x53, 0, ANNOUNCE_MAC_END,
    10, 44, 1, 1, 0, 0, 0, 0, 0, 0, 10, 44, 1, 1,
];

#[cfg(feature = "tx-diagnostic")]
pub const REPORT_WORDS: usize = 248;

#[cfg(any(feature = "tx-diagnostic", feature = "lan-port"))]
fn record(index: usize, value: u32) {
    crate::status(index).store(value, core::sync::atomic::Ordering::Release);
}

#[cfg(feature = "tx-diagnostic")]
pub fn init_report() {
    for i in 25..REPORT_WORDS { record(i, u32::MAX); }
    record(25, 2);
    record(247, 0);
    record(26, u32::from(EXTENDED_TX) | (u32::from(cfg!(feature = "tx-check")) << 1)
        | (u32::from(cfg!(feature = "tally-rx-first")) << 2));
    record(27, 0);
    record(128, 0);
    record(136, 0);
}

#[cfg(any(feature = "tx-diagnostic", feature = "lan-port"))]
pub fn inspect_aperture<I: ConfigIo>(io: &mut I, window: &InboundWindow, expected_limit: Option<u32>) -> bool {
    let end = window.bus + window.size - 1;
    let mut valid = expected_limit.is_some();
    for region in 0..8 {
        let base = DBI + 0x300100 + 0x200 * region as u64;
        let mut fields = [u32::MAX; 8];
        let offsets = [0, 4, 8, 12, 16, 32, 20, 24];
        for (i, offset) in offsets.into_iter().enumerate() {
            match io.read32(base + offset) {
                Ok(v) => fields[i] = v,
                Err(_) => {
                    if valid { io.inbound_step(8, 2, base + offset, u32::MAX, u32::MAX); }
                    valid = false;
                }
            }
            record(64 + region * 8 + i, fields[i]);
        }
        let [kind, control, lo, hi, last, last_hi, _, _] = fields;
        if region == 0 {
            let expected = [0, 1 << 31, window.bus as u32, 0, expected_limit.unwrap_or(end as u32), 0, window.physical as u32, 0];
            if let Some(i) = (0..8).find(|&i| fields[i] != expected[i]) {
                if valid { io.inbound_step(8, 8, base + offsets[i], expected[i], fields[i]); }
                valid = false;
            }
        } else if control & (1 << 31) != 0 {
            let start = u64::from(hi) << 32 | u64::from(lo);
            let limit = u64::from(last_hi) << 32 | u64::from(last);
            // Range inversion, BAR matching, and unknown match rules cannot exclude an alias.
            if !(control == 1 << 31 && kind == 0 && start <= limit
                && (end < start || limit < window.bus))
            {
                if valid {
                    io.inbound_step(8, 9, base + 4, 0, control);
                    record(44, region as u32);
                }
                valid = false;
            }
        }
    }
    record(27, u32::from(valid));
    valid
}

#[cfg(feature = "tx-diagnostic")]
fn tx_diagnostic<I: Registers>(
    dma: &mut B51Dma<I>,
    ring: &mut Ring<SLOTS>,
    tally: *mut u32,
    tally_bus: u64,
) -> Result<(), u32> {
    #[cfg(feature = "tx-no-close")]
    let snapshot = |dma: &mut B51Dma<I>, index| {
        let regs = dma.registers();
        for (index, offset) in [crate::dma::TX_TAIL, crate::dma::TX_CLOSE].into_iter().enumerate()
            .map(|(offset_index, offset)| (index + offset_index, offset))
        {
            let value = regs.read16(BAR + offset).map_or(u32::MAX, u32::from);
            crate::status(index).store(value, core::sync::atomic::Ordering::Release);
        }
    };
    #[cfg(feature = "tx-no-close")]
    snapshot(dma, 13);
    let (tx, _, packet) = dma.bus_addresses();
    let regs = dma.registers();
    for (i, offset) in [0x20, 0x24, 0xe4, 0xe8, 0x40].into_iter().enumerate() {
        record(48 + i, regs.read32(BAR + offset).unwrap_or(u32::MAX));
    }
    record(53, regs.read8(BAR + 0x37).map_or(u32::MAX, u32::from));
    record(54, regs.read16(BAR + 0x6c).map_or(u32::MAX, u32::from));
    record(55, regs.read32(BAR + 0x3c).unwrap_or(u32::MAX));
    record(56, regs.read32(DBI + 4).unwrap_or(u32::MAX));
    let mut ep = DirectEndpoint { io: regs, dbi: DBI, config_aperture: CONFIG, buses: BUSES };
    for (i, off) in [4, 0x78, 0x80].into_iter().enumerate() {
        record(57 + i, ep.read_dword(off).unwrap_or(u32::MAX));
    }
    let result = (|| {
        ring.send_observed(dma, &ANNOUNCEMENT, |backend, slot| {
            let words = backend.tx_descriptor_words(slot)?;
            for (i, value) in words.into_iter().enumerate() { record(32 + i, value); }
            if cfg!(feature = "tx-check") && (slot != 0 || tx & 255 != 0
                || words != [0xb000003c, 0, packet as u32, (packet >> 32) as u32, 0, 0, 0, 0])
            {
                record(246, 0);
                return Err(crate::ring::Error::Layout);
            }
            if cfg!(feature = "tx-check") { record(246, 1); }
            Ok(())
        }).map_err(|_| 171u32)?;
        #[cfg(feature = "tx-no-close")]
        snapshot(dma, 62);
        progress(21);
        let start = ticks();
        loop {
            let flags = dma.status(false, 0).map_err(|_| 171u32)?;
            crate::status(9).store(flags, core::sync::atomic::Ordering::Release);
            if flags & OWN == 0 {
                progress(22);
                return Ok(());
            }
            if ticks().wrapping_sub(start) >= 24_000_000 {
                progress(23);
                return Err(169);
            }
            dma.registers().delay_us(1000);
        }
    })();
    #[cfg(feature = "tx-no-close")]
    snapshot(dma, 15);
    if let Ok(words) = dma.tx_descriptor_words(0) {
        for (i, value) in words.into_iter().enumerate() { record(40 + i, value); }
    }
    let regs = dma.registers();
    let mac_status = regs.read32(BAR + 0x3c).map_err(|_| ());
    let root_status = regs.read32(DBI + 4).map_err(|_| ());
    let mut endpoint = DirectEndpoint { io: regs, dbi: DBI, config_aperture: CONFIG, buses: BUSES };
    let endpoint_status = endpoint.read_dword(4).map_err(|_| ());
    if endpoint.read_dword(0x100).is_ok_and(|v| v & 0xffff == 1) {
        record(60, endpoint.read_dword(0x104).unwrap_or(u32::MAX));
        record(61, endpoint.read_dword(0x110).unwrap_or(u32::MAX));
    }
    const PCI_ERRORS: u32 = 0xf900_0000;
    let mut error = false;
    for (index, sample, mask) in [
        (0, mac_status, 0x8008),
        (1, root_status, PCI_ERRORS),
        (2, endpoint_status, PCI_ERRORS),
    ] {
        match sample {
            Ok(value) => {
                crate::status(10 + index).store(value, core::sync::atomic::Ordering::Release);
                if value & mask != 0 {
                    progress(24 + index as u32);
                    error = true;
                }
            }
            Err(()) => {
                progress(27);
                error = true;
            }
        }
    }
    let io = endpoint.io;
    let first = if cfg!(feature = "tally-rx-first") { 0 } else { dump_tally(io, tally, tally_bus, false) };
    record(21, first);
    let second = if first == 31 {
        0
    } else if io.write8(BAR + 0x37, 0x0c).is_err() {
        30
    } else {
        dump_tally(io, unsafe { tally.add(512) }, tally_bus + 2048, true)
    };
    record(22, second);
    progress(if second == 0 { first } else { second });
    result.and(if error || first == 31 || second == 31 { Err(171) } else { Ok(()) })
}

#[cfg(feature = "tx-diagnostic")]
fn dump_tally<I: Registers>(io: &mut I, page: *mut u32, bus: u64, rx: bool) -> u32 {
    const DUMP: u32 = 8;
    const START: usize = 64;
    const END: usize = START + 48;
    const SENTINEL: u32 = 0xa5a5_a5a5;
    let base = if rx { 136 } else { 128 };
    let Ok(chip) = io.read8(BAR + 0x37) else { return 30 };
    record(19 + usize::from(rx), chip.into());
    if chip & 0x1c != if rx { 12 } else { 4 } { return 30; }
    record(base, 1);
    if !(0..21).any(|_| {
        io.delay_us(10);
        let cmd = io.read32(BAR + 0x10).unwrap_or(u32::MAX);
        record(base + 1, cmd);
        cmd != u32::MAX && cmd & (DUMP | 1) == 0
    }) {
        return 30;
    }
    for index in 0..512 { unsafe { page.add(index).write_volatile(SENTINEL) }; }
    crate::mmio::ordered();
    let read = |index| unsafe { page.add(index).read_volatile() };
    record(base + 2, read(START));
    let address = bus + (START * 4) as u64;
    record(base, 2);
    if io.write32(BAR + 0x14, (address >> 32) as u32)
        .and_then(|()| io.write32(BAR + 0x10, address as u32))
        .and_then(|()| io.write32(BAR + 0x10, address as u32 | DUMP)).is_err() { return 30; }
    record(base, 3);
    let mut completed = false;
    for poll in 1..=21 {
        io.delay_us(10);
        let cmd = io.read32(BAR + 0x10).unwrap_or(u32::MAX);
        record(base + 3, cmd);
        record(base + 4, poll);
        if cmd == u32::MAX { break; }
        if cmd & DUMP == 0 {
            completed = true;
            record(base, 4);
            break;
        }
    }
    crate::mmio::ordered();
    let guard = (0..START).chain(END..512).find(|&i| read(i) != SENTINEL);
    record(base + 5, guard.map_or(0, |i| i as u32 + 1));
    let mut changed = 0;
    for i in 0..48 {
        let word = u32::from_le(read(START + i));
        record(if rx {192} else {144} + i, word);
        if i < 2 { record(if rx {17} else {23} + i, word); }
        changed += u32::from(word != SENTINEL);
    }
    record(base + 6, changed);
    if guard.is_some() { 31 } else if !completed { 30 } else if changed != 0 { 28 } else { 29 }
}

#[cfg(all(feature = "lan-dhcp", not(feature = "tx-diagnostic"), not(feature = "lan-port")))]
fn serve<I: Registers>(dma: &mut B51Dma<I>, ring: &mut Ring<SLOTS>) -> Result<(), u32> {
    let mut service = LanService::new(ticks() as u32);
    loop {
        #[cfg(feature = "supervised")]
        crate::phase(6);
        if dma.registers().read32(BAR + 0x3c).map_err(|_| 171u32)? & 0x8000 != 0 {
            return Err(171);
        }
        service.poll(dma, ring, ticks() / 24_000)?;
        #[cfg(feature = "zc")]
        zc::flush_counts(zc::META_VA);
    }
}

#[cfg(feature = "lan-port")]
fn tx_checksum_options(frame: &[u8]) -> u32 {
    use crate::ring::{TX_IPV4_CHECKSUM,TX_TCP_CHECKSUM,TX_UDP_CHECKSUM};
    use r5s_router_core::{u16at,validate_ip,transport_checksum};
    if frame.len()<34 || frame[12..14]!=[8,0] {return 0;}
    let ip=&frame[14..];
    let Ok(length)=validate_ip(ip) else {return 0;};
    if frame.len()<175 {return TX_IPV4_CHECKSUM;}
    let options=match ip[9] {
        6 if length>=40 && (20..=length-20).contains(&(usize::from(ip[32]>>4)*4)) => TX_TCP_CHECKSUM,
        17 if length>=28 && usize::from(u16at(ip,24))==length-20 && u16at(ip,26)!=0 => TX_UDP_CHECKSUM,
        _ => TX_IPV4_CHECKSUM,
    };
    if options!=TX_IPV4_CHECKSUM && transport_checksum(ip,length)!=0 {TX_IPV4_CHECKSUM} else {options}
}

#[cfg(all(feature = "lan-port", feature = "zc"))]
fn lan_worker(port: usize, frame: &[u8]) -> Option<Option<crate::zc::TransportPin>> {
    use crate::zc;
    use r5s_router_core::steering;
    if !zc::lan_fast_path() { return None; }
    let (key, hash) = steering::direct_lan_endpoint(frame)?;
    Some(zc::pin_transport(key, hash, |worker| {
        zc::RefQueue::depth(zc::fast_lan_requests(port, worker)).unwrap_or(zc::RQ_COUNT)
    }))
}

#[cfg(feature = "lan-port")]
fn serve<I: Registers>(
    dma: &mut B51Dma<I>, ring: &mut Ring<SLOTS>, rss_rings: &mut [Option<Ring<SLOTS>>; RX_QUEUES - 1],
    mut secondary_tx: Option<&mut Ring<SLOTS>>, identity: Identity,
) -> Result<(), u32> {
    use crate::{queue, status};
    #[cfg(feature = "zc")]
    use crate::zc;
    use core::sync::atomic::Ordering::{Relaxed, Release};
    const QUEUE: usize = 0x63000000;
    const RX_MASK: u32 = 0xf << 19;
    #[cfg(feature = "zc")]
    const LAN_PORT: usize = cfg!(feature = "lan2") as usize;
    #[cfg(feature = "zc")]
    crate::zc::init(crate::zc::META_VA);
    const RX_BATCH: u32 = queue::batch(option_env!("R5S_B51_RX_BATCH"), 1);
    let tx_checksum = if cfg!(feature = "lan2") {
        option_env!("R5S_LAN2_TX_IPV4_CHECKSUM")
    } else {
        option_env!("R5S_LAN1_TX_IPV4_CHECKSUM")
    } == Some("1");
    status(263).store(u32::from(tx_checksum),Release);
    status(276).store(SLOTS as u32, Release);
    status(279).store(RX_QUEUES as u32, Release);
    status(285).store(TX_QUEUES as u32, Release);
    let mut next_rx = 0;
    let mut frame = [0; 1514];
    let mut submitted_at = [[0u64; SLOTS]; TX_QUEUES];
    #[cfg(feature = "zc")]
    const PARTITION: usize = if cfg!(feature = "lan2") {
        crate::zc::PARTITION_LAN2
    } else {
        crate::zc::PARTITION_LAN1
    };
    #[cfg(feature = "zc")]
    let mut tx_refs = [[None; SLOTS]; TX_QUEUES];
    // Pool buffer offered to the re-arm of the next consumed private slot.
    #[cfg(feature = "zc")]
    let mut migration: Option<u64> = None;
    #[cfg(feature = "zc")]
    let mut next_tx_source = [0; TX_QUEUES];
    #[cfg(feature = "zc")]
    let mut direct_tx_slots = [[0usize; SLOTS]; TX_QUEUES];
    // Download-fallback return tokens riding copied submissions; 0 = none.
    #[cfg(feature = "zc")]
    let mut token_slots = [[0u32; SLOTS]; TX_QUEUES];
    let mut last_pending_tx = 0;
    let mut sample_at = 0;
    let mut phy = 0;
    #[cfg(feature = "tso")]
    let mut tso = TsoState::new(crate::zc::META_VA, crate::zc::POOL_VA, crate::zc::POOL_PA);
    #[cfg(feature = "tso-test")]
    let mut tso_test_done = false;
    #[cfg(feature = "tso")]
    {
        status(273).store(1, Relaxed);
        status(275).store(u32::from(crate::tso::TSO_IPV6), Relaxed);
    }
    let result = (|| loop {
        let now = ticks() / 24_000;
        if now >= sample_at {
            phy = dma.registers().read16(BAR + 0x6c).map_err(|_| 171u32)?;
            status(6).store(phy.into(), Relaxed);
            status(32).store(u32::from(phy & 2 != 0), Release);
            #[cfg(feature = "zc")]
            if option_env!("R5S_WORKER_LAN_EGRESS") == Some("1") {
                status(284).store(zc::worker_lan_tx_depths(LAN_PORT), Relaxed);
            }
            sample_at = now.saturating_add(10);
        }
        let irq = dma.registers().read32(BAR + 0x3c).map_err(|_| 171u32)?;
        let errors = irq & (0x8000 | 0x10 | 0x08 | 0x02);
        if errors != 0 {
            status(42).fetch_or(errors, Relaxed);
            if errors & 0x10 != 0 { status(37).fetch_add(1, Relaxed); }
            if errors & 0x08 != 0 { status(259).fetch_add(1, Relaxed); }
            if errors & 0x8008 != 0 { return Err(171); }
            dma.registers().write32(BAR + 0x3c, errors).map_err(|_| 171u32)?;
        }
        let mut busy = false;
        let rx_batch = if errors & 0x10 != 0 { RX_BATCH * 2 } else { RX_BATCH };
        let tx_batch = queue::batch(option_env!("R5S_B51_TX_BATCH"), queue::COUNT);
        for _ in 0..2 {
            let mut pending_tx = 0;
            for tx_queue in 0..TX_QUEUES {
                dma.select_tx_queue(tx_queue).map_err(|_| 171u32)?;
                let ring = if tx_queue == 0 { &mut *ring }
                    else { secondary_tx.as_deref_mut().ok_or(171u32)? };
                let submitted_at = &mut submitted_at[tx_queue];
                #[cfg(feature = "zc")]
                let (tx_refs, direct_tx_slots, next_tx_source, token_slots) =
                    (&mut tx_refs[tx_queue], &mut direct_tx_slots[tx_queue], &mut next_tx_source[tx_queue],
                     &mut token_slots[tx_queue]);
                #[cfg(any(feature = "zc", feature = "tso"))]
                let oldest_tx = dma.oldest_tx_slot();
                let completed = dma.reclaim_tx().map_err(|_| {
                    status(259).fetch_add(1, Relaxed);
                    171u32
                })? as u32;
                #[cfg(feature = "zc")]
                for offset in 0..completed as usize {
                    let slot = (oldest_tx + offset) % SLOTS;
                    let direct_refs = core::mem::take(&mut direct_tx_slots[slot]);
                    if direct_refs != 0 { retire_direct_tso(direct_refs); }
                    // Verified TX completion returns the shared buffer to the pool.
                    if let Some(r) = tx_refs[slot].take() {
                        let _ = crate::zc::complete(crate::zc::META_VA, &r);
                    }
                    // A completed (or confirmed-discarded) submission retires its
                    // fallback token exactly once.
                    let token = core::mem::take(&mut token_slots[slot]);
                    if token != 0 { crate::zc::retire_lan_token(crate::zc::META_VA, token); }
                }
                #[cfg(feature = "tso")]
                tso.reclaim(ring, oldest_tx, completed as usize);
                busy |= completed != 0;
                if completed != 0 {
                    status(39).fetch_add(completed, Relaxed);
                    status(286 + tx_queue).fetch_add(completed, Relaxed);
                    status(260).fetch_add(1, Relaxed);
                    status(261).fetch_max(completed, Relaxed);
                }
                if dma.tx_inflight() != 0 && now - submitted_at[dma.oldest_tx_slot()] >= 1000 {
                    status(40).fetch_add(1, Relaxed);
                    status(259).fetch_add(1, Relaxed);
                    return Err(169);
                }
                if phy & 2 != 0 {
                    #[cfg(feature = "tso-test")]
                    if tx_queue == 0 && !tso_test_done && dma.tx_inflight() == 0 {
                        tso_test_done = true;
                        tso_testgen_run(&mut tso, dma, ring, PARTITION, submitted_at, now)?;
                    }
                    if option_env!("R5S_B51_TX_BATCH").is_some() {
                        dma.begin_tx_batch();
                    }
                    #[cfg(feature = "zc")]
                    if tx_queue == 0 && dma.tx_inflight() < SLOTS - 1 {
                        let slot = (dma.oldest_tx_slot() + dma.tx_inflight()) % SLOTS;
                        let sent = unsafe { queue::consume_in_place(crate::zc::lan_control_tx(LAN_PORT),
                            |frame, _| ring.send(dma, frame)) }.map_err(|_| 171u32)?;
                        if let Some(result) = sent {
                            result.map_err(|_| 171u32)?;
                            submitted_at[slot] = now;
                            status(38).fetch_add(1, Relaxed);
                            busy = true;
                        }
                    }
                    #[cfg(feature = "zc")]
                    let sources = if option_env!("R5S_WORKER_LAN_EGRESS")==Some("1") {
                        1 + zc::WORKER_TX_LANES
                    } else {1};
                    #[cfg(feature = "zc")]
                    let mut ready = {
                        let all = (1u32 << sources) - 1;
                        if TX_QUEUES == 1 {all} else if tx_queue == 0 {1} else {all & !1}
                    };
                    'tx: for _ in 0..tx_batch {
                        if dma.tx_inflight() == SLOTS - 1 {
                            status(258).fetch_add(1, Relaxed);
                            break;
                        }
                        #[cfg(feature = "zc")]
                        let (copy_queue, copy_revision, direct_refs, token) = {
                            use crate::zc::{self, RefQueue};
                            let queue = zc::link_queue(zc::LINKS_VA,
                                if cfg!(feature = "lan2") { zc::Q_LAN2_TX } else { zc::Q_LAN1_TX });
                            let mut candidate = None;
                            let mut scan = ready;
                            while let Some(source) = zc::next_ready_lane(scan, *next_tx_source) {
                                let bit = 1u32 << source;
                                scan &= !bit;
                                #[cfg(feature = "tso")]
                                if tso.software[source].is_some() {
                                    let (progress,fence)=tso.resume_software(source,dma,ring,submitted_at,now,1)?;
                                    if let Some((slot,stamp))=fence { direct_tx_slots[slot]=stamp; }
                                    if progress {
                                        busy=true;
                                        *next_tx_source=(source+1)%sources;
                                        continue 'tx;
                                    }
                                    ready &= !bit;
                                    continue;
                                }
                                let refs = if source == 0 {queue} else {zc::worker_lan_refs(LAN_PORT, source - 1)};
                                #[cfg(feature = "tso")]
                                {
                                    if let Some(head) = RefQueue::peek(refs).map_err(|_| 171u32)? {
                                        let flags = head.flags & crate::tso::FLAG_TSO_MASK;
                                        if flags != 0 {
                                            let count = (flags == crate::tso::FLAG_TSO_FIRST)
                                                .then(|| crate::tso::queued_group(refs, head)).flatten();
                                            let Some(group) = count else {
                                                RefQueue::pop(refs).map_err(|_| 171u32)?;
                                                let _ = zc::reject(zc::META_VA, &head);
                                                if source!=0 {
                                                    zc::retire_worker_lan_tx(refs);
                                                    zc::retire_lan_token(zc::META_VA, zc::direct_lan_token(&head));
                                                }
                                                tso.invalid = tso.invalid.saturating_add(1);
                                                busy = true;
                                                continue;
                                            };
                                            let software = head.flags & crate::tso::FLAG_TSO_GRO != 0
                                                && option_env!("R5S_GRO_TSO") != Some("1");
                                            let needed = if software {1} else {group.fragments} as usize;
                                            if SLOTS - 1 - dma.tx_inflight() < needed
                                                || (!software && tso.pending.iter().all(Option::is_some))
                                            {
                                                ready &= !bit;
                                                continue;
                                            }
                                            if source!=0 {
                                                let valid=(0..group.fragments).all(|index|
                                                    RefQueue::peek_at(refs,index).ok().flatten().is_some_and(|r|
                                                        zc::status(zc::META_VA,r.id())==(zc::CPU,r.generation())));
                                                let frame=unsafe {core::slice::from_raw_parts(
                                                    (zc::POOL_VA+head.id() as usize*zc::BUFFER) as *const u8,head.len as usize)};
                                                if !valid || !zc::worker_lan_tx_valid(zc::FAST_WAN_CONTROL,frame,LAN_PORT,head.epoch,now) {
                                                    for _ in 0..group.fragments {
                                                        let r=RefQueue::pop(refs).map_err(|_| 171u32)?.ok_or(171u32)?;
                                                        let _=zc::reject(zc::META_VA,&r);
                                                        zc::retire_worker_lan_tx(refs);
                                                        zc::retire_lan_token(zc::META_VA, zc::direct_lan_token(&r));
                                                    }
                                                    busy=true;
                                                    continue;
                                                }
                                                tso.direct=refs | group.fragments as usize
                                                    | ((zc::direct_lan_token(&head) as usize) << 32);
                                                tso.direct_epoch=head.epoch;
                                            }
                                            tso.source=source;
                                            let first=RefQueue::pop(refs).map_err(|_| 171u32)?.ok_or(171u32)?;
                                            tso_group(&mut tso,dma,ring,refs,first,submitted_at,now)?;
                                            if tso.direct!=0 {retire_direct_tso(core::mem::take(&mut tso.direct));}
                                            *next_tx_source=(source+1)%sources;
                                            busy=true;
                                            continue 'tx;
                                        }
                                    }
                                }
                                if let Some(record) = RefQueue::pop(refs).map_err(|_| 171u32)? {
                                    candidate = Some((source, record));
                                    *next_tx_source = (source + 1) % sources;
                                    break;
                                }
                                ready &= !bit;
                            }
                            let Some((source,r)) = candidate else { break };
                            let direct_refs = if source == 0 {0} else {zc::worker_lan_refs(LAN_PORT, source - 1)};
                            let token = if source != 0 { zc::direct_lan_token(&r) }
                                else if r.flags & zc::FLAG_LAN_TOKEN != 0 { r.meta } else { 0 };
                            if r.flags & zc::FLAG_COPY == 0 {
                                if zc::status(zc::META_VA, r.id()) != (zc::CPU, r.generation()) {
                                    zc::count(zc::META_VA, zc::C_OWNERSHIP_ERRORS);
                                    if direct_refs != 0 {zc::retire_worker_lan_tx(direct_refs);}
                                    if token != 0 {zc::retire_lan_token(zc::META_VA, token);}
                                    continue;
                                }
                                let length = r.len as usize;
                                let pa = match zc::buffer_physical(zc::POOL_PA, r.id()) {
                                    Some(pa) if (14..=1514).contains(&length) => pa,
                                    _ => {
                                        let _ = zc::reject(zc::META_VA, &r);
                                        if direct_refs != 0 {zc::retire_worker_lan_tx(direct_refs);}
                                        if token != 0 {zc::retire_lan_token(zc::META_VA, token);}
                                        continue;
                                    }
                                };
                                let bus = match dma.translate_external(pa, zc::BUFFER as u64) {
                                    Ok(bus) => bus,
                                    Err(_) => {
                                        zc::count(zc::META_VA, zc::C_MAPPING_FAILURES);
                                        let _ = zc::reject(zc::META_VA, &r);
                                        if direct_refs != 0 {zc::retire_worker_lan_tx(direct_refs);}
                                        if token != 0 {zc::retire_lan_token(zc::META_VA, token);}
                                        continue;
                                    }
                                };
                                // The pool is Normal non-cacheable: zero-fill the Ethernet
                                // minimum from the CPU side and order the payload before the
                                // descriptor is published.
                                let buffer = unsafe {
                                    core::slice::from_raw_parts_mut(
                                        (zc::POOL_VA + r.id() as usize * zc::BUFFER) as *mut u8, zc::BUFFER)
                                };
                                if source != 0 && !zc::worker_lan_tx_valid(zc::FAST_WAN_CONTROL, &buffer[..length], LAN_PORT,r.epoch,now) {
                                    let _ = zc::reject(zc::META_VA, &r);
                                    zc::retire_worker_lan_tx(direct_refs);
                                    if token != 0 {zc::retire_lan_token(zc::META_VA, token);}
                                    status(41).fetch_add(1, Relaxed);
                                    continue;
                                }
                                // Worker-prepared refs carry their final payload
                                // (padding and checksum seeds written and the buffer
                                // cache-maintained on the worker core); only the
                                // insertion options are derived here read-only.
                                let prepared = r.flags & zc::FLAG_WAN_READY != 0;
                                if !prepared && length < 60 {
                                    buffer[length..60].fill(0);
                                }
                                let options = if prepared {
                                    if r.flags & zc::FLAG_WAN_TRANSPORT_READY != 0 {
                                        match zc::nat_tx_checksum_kind(&buffer[..length], r.flags) {
                                            zc::TxChecksum::None => 0,
                                            zc::TxChecksum::Ipv4 => crate::ring::TX_IPV4_CHECKSUM,
                                            zc::TxChecksum::Tcp => crate::ring::TX_TCP_CHECKSUM,
                                            zc::TxChecksum::Udp => crate::ring::TX_UDP_CHECKSUM,
                                        }
                                    } else { 0 }
                                } else if zc::TX_CHECKSUM_REQUESTED && tx_checksum {
                                    match zc::prepare_nat_tx_checksum(&mut buffer[..length], r.flags, true) {
                                        zc::TxChecksum::None => 0,
                                        zc::TxChecksum::Ipv4 => crate::ring::TX_IPV4_CHECKSUM,
                                        zc::TxChecksum::Tcp => crate::ring::TX_TCP_CHECKSUM,
                                        zc::TxChecksum::Udp => crate::ring::TX_UDP_CHECKSUM,
                                    }
                                } else { 0 };
                                if !prepared {
                                    zc::maintain_payload(&r, zc::PayloadCache::BeforeTx(length.max(60))).map_err(|_| 171u32)?;
                                }
                                let slot = (dma.oldest_tx_slot() + dma.tx_inflight()) % SLOTS;
                                // A failed doorbell can follow descriptor publication.
                                if options == 0 { ring.send_external(dma, bus, length, 0) }
                                else { ring.send_external_checksum(dma, bus, length, options) }
                                    .map_err(|_| 171u32)?;
                                if options != 0 { status(262).fetch_add(1, Relaxed); }
                                zc::submit(zc::META_VA, &r).map_err(|_| 171u32)?;
                                tx_refs[slot] = Some(r);
                                direct_tx_slots[slot] = direct_refs;
                                token_slots[slot] = token;
                                submitted_at[slot] = now;
                                status(38).fetch_add(1, Relaxed);
                                busy = true;
                                continue;
                            }
                            if source == 0 {(QUEUE + queue::DIRECTION, 0, 0, token)}
                            else {(zc::worker_lan_frames(LAN_PORT, source - 1), r.epoch, direct_refs, token)}
                        };
                        #[cfg(not(feature = "zc"))]
                        let (copy_queue, copy_revision) = (QUEUE + queue::DIRECTION, 0);
                        let slot = (dma.oldest_tx_slot() + dma.tx_inflight()) % SLOTS;
                        let sent = unsafe { queue::consume_in_place(copy_queue, |frame, tag| {
                            #[cfg(feature = "zc")]
                            if direct_refs != 0 && !crate::zc::worker_lan_tx_valid(crate::zc::FAST_WAN_CONTROL, frame, LAN_PORT,copy_revision,now) {
                                status(41).fetch_add(1, Relaxed);
                                return None;
                            }
                            (tag == copy_revision).then(|| {
                                #[cfg(feature = "zc")]
                                let validated = direct_refs != 0 && crate::zc::TX_CHECKSUM_REQUESTED;
                                #[cfg(not(feature = "zc"))]
                                let validated = false;
                                let options=if tx_checksum && !validated {tx_checksum_options(frame)} else {0};
                                if options!=0 {
                                    ring.send_checksum(dma,frame,options)?;
                                    status(262).fetch_add(1,Relaxed);
                                    Ok(())
                                } else {ring.send(dma,frame)}
                            })
                        }) }.map_err(|_| 171u32)?;
                        #[cfg(feature = "zc")]
                        if sent.is_none() { return Err(171); }
                        #[cfg(feature = "zc")]
                        if direct_refs != 0 && matches!(sent, Some(None)) {
                            crate::zc::retire_worker_lan_tx(direct_refs);
                            if token != 0 { crate::zc::retire_lan_token(crate::zc::META_VA, token); }
                            continue;
                        }
                        let Some(Some(sent)) = sent else {
                            #[cfg(feature = "zc")]
                            if token != 0 { crate::zc::retire_lan_token(crate::zc::META_VA, token); }
                            break
                        };
                        sent.map_err(|_| {
                            status(259).fetch_add(1, Relaxed);
                            171u32
                        })?;
                        #[cfg(feature = "zc")]
                        { direct_tx_slots[slot] = direct_refs; token_slots[slot] = token; }
                        submitted_at[slot] = now;
                        status(38).fetch_add(1, Relaxed);
                        busy = true;
                    }
                    dma.end_tx_batch().map_err(|_| 171u32)?;
                }
                pending_tx += dma.tx_inflight() as u32;
            }
            if pending_tx != last_pending_tx {
                status(256).store(pending_tx, Relaxed);
                status(257).fetch_max(pending_tx, Relaxed);
                last_pending_tx = pending_tx;
            }
            dma.select_tx_queue(0).map_err(|_| 171u32)?;
            if phy & 2 != 0 {
                #[cfg(feature = "rx-gro")]
                let mut ingress = zc::IngressBatch::new(zc::META_VA);
                for _ in 0..rx_batch {
                    #[cfg(feature = "rx-gro")]
                    {
                        let drops = ingress.flush_expired(|| ticks() / 24);
                        if drops != 0 { status(41).fetch_add(drops, Relaxed); }
                    }
                    let queue_id = if RX_QUEUES == 1 { 0 } else {
                        let mut found = None;
                        for step in 0..RX_QUEUES {
                            let queue_id = (next_rx + step) % RX_QUEUES;
                            dma.select_rx_queue(queue_id).map_err(|_| 171u32)?;
                            let receive = if queue_id == 0 { &*ring } else {
                                rss_rings[queue_id - 1].as_ref().ok_or(171u32)?
                            };
                            if receive.rx_ready(dma).map_err(|_| 171u32)? {
                                found = Some(queue_id);
                                break;
                            }
                        }
                        let Some(queue_id) = found else { break };
                        next_rx = (queue_id + 1) % RX_QUEUES;
                        queue_id
                    };
                    let ring = if queue_id == 0 { &mut *ring } else {
                        rss_rings[queue_id - 1].as_mut().ok_or(171u32)?
                    };
                    #[cfg(feature = "zc")]
                    let pool_received = {
                        let mut pool_received = false;
                        match ring.receive_external(dma, RX_MASK).map_err(|_| 171u32)? {
                            Rx::External(slot, bus, length) => {
                                pool_received = true;
                                if RX_QUEUES > 1 { status(280 + queue_id).fetch_add(1, Relaxed); }
                                status(33).fetch_add(1, Relaxed);
                                status(34).fetch_add(length as u32, Relaxed);
                                status(35).fetch_add(1, Relaxed);
                                status(43).fetch_add(1, Relaxed);
                                // The reference is reconstructed from the buffer address
                                // and its current ownership generation.
                                let r = dma.external_physical(bus).ok().and_then(|pa| {
                                    let offset = pa.checked_sub(crate::zc::POOL_PA)?;
                                    if offset >= crate::zc::POOL_BYTES as u64
                                        || offset % crate::zc::BUFFER as u64 != 0 { return None; }
                                    let id = (offset / crate::zc::BUFFER as u64) as u32;
                                    let (state, generation) = crate::zc::status(crate::zc::META_VA, id);
                                    (state == crate::zc::RX_DMA).then(|| crate::zc::Ref::new(id, generation))
                                });
                                let Some(mut r) = r else {
                                    crate::zc::count(crate::zc::META_VA, crate::zc::C_OWNERSHIP_ERRORS);
                                    status(36).fetch_add(1, Relaxed);
                                    ring.arm_rx_external(dma, slot, None).map_err(|_| 171u32)?;
                                    continue;
                                };
                                zc::maintain_payload(&r, zc::PayloadCache::AfterRx(length)).map_err(|_| 171u32)?;
                                let buffer = unsafe {
                                    core::slice::from_raw_parts(
                                        (crate::zc::POOL_VA + r.id() as usize * crate::zc::BUFFER) as *const u8,
                                        crate::zc::BUFFER)
                                };
                                let verified = if rx_checksum_requested() && length >= 34
                                    && buffer[12..15] == [8, 0, 0x45] {
                                    ring.rx_transport_checksum()
                                } else {
                                    0
                                };
                                let published = crate::zc::publish(crate::zc::META_VA, &r).is_ok();
                                r.len = length as u16;
                                r.meta = verified;
                                let queued = published && if let Some(admission) = lan_worker(LAN_PORT, &buffer[..length]) {
                                    admission.is_some_and(|pin| zc::route_revision(zc::FAST_WAN_CONTROL, None).is_some_and(|revision| {
                                        r.epoch = revision;
                                        r.flags = ((LAN_PORT + 1) as u16) << zc::LAN_INGRESS_SHIFT;
                                        let queue = zc::fast_lan_requests(LAN_PORT, pin.worker);
                                        #[cfg(feature = "rx-gro")]
                                        {
                                            let first = crate::tso::gro_starts(&buffer[..length]);
                                            let drops = unsafe {
                                                ingress.offer_pinned_pool(queue, r, first, ticks() / 24, Some(pin),
                                                    |a, b| crate::tso::gro_pair(a, b).is_some())
                                            };
                                            if drops != 0 { status(41).fetch_add(drops, Relaxed); }
                                            true
                                        }
                                        #[cfg(not(feature = "rx-gro"))]
                                        { zc::RefQueue::push(queue, &r) == Ok(true) }
                                    }))
                                } else {
                                    #[cfg(feature = "rx-gro")]
                                    {
                                        let drops = ingress.flush();
                                        if drops != 0 { status(41).fetch_add(drops, Relaxed); }
                                    }
                                    let queue = zc::link_queue(zc::LINKS_VA,
                                        if LAN_PORT == 0 { zc::Q_LAN1_RX } else { zc::Q_LAN2_RX });
                                    zc::RefQueue::push(queue, &r) == Ok(true)
                                };
                                if !queued {
                                    if published {
                                        let _ = crate::zc::reject(crate::zc::META_VA, &r);
                                    }
                                    status(41).fetch_add(1, Relaxed);
                                }
                                // Re-arm with a fresh pool buffer, or fall back to the
                                // private copy-path buffer when the pool is exhausted.
                                let fresh = crate::zc::alloc(crate::zc::META_VA, PARTITION);
                                if let Some(r) = fresh {
                                    zc::maintain_payload(&r, zc::PayloadCache::BeforeRx).map_err(|_| 171u32)?;
                                }
                                let armed = fresh.as_ref().and_then(|fresh| {
                                    let pa = crate::zc::buffer_physical(crate::zc::POOL_PA, fresh.id())?;
                                    dma.translate_external(pa, crate::zc::BUFFER as u64).ok()
                                });
                                match ring.arm_rx_external(dma, slot, armed) {
                                    Ok(()) => {
                                        if armed.is_none() {
                                            if let Some(fresh) = fresh {
                                                let _ = crate::zc::recycle(crate::zc::META_VA, &fresh);
                                            }
                                        }
                                    }
                                    Err(_) => {
                                        if let Some(fresh) = fresh {
                                            let _ = crate::zc::recycle(crate::zc::META_VA, &fresh);
                                        }
                                        return Err(171);
                                    }
                                }
                                progress(10);
                                busy = true;
                            }
                            Rx::Rejected => {
                                pool_received = true;
                                status(35).fetch_add(1, Relaxed);
                                status(36).fetch_add(1, Relaxed);
                                status(43).fetch_add(1, Relaxed);
                                busy = true;
                            }
                            _ => {}
                        }
                        pool_received
                    };
                    #[cfg(not(feature = "zc"))]
                    let pool_received = false;
                    if pool_received { continue; }
                    #[cfg(feature = "zc")]
                    {
                        // An idle single ring has no descriptor to migrate. RSS already checked OWN.
                        if ring.current_rx_external() || (RX_QUEUES == 1 && !ring.rx_ready(dma).map_err(|_| 171u32)?) { break; }
                        // Offer a pool buffer for this slot's re-arm so private slots
                        // migrate onto the shared pool as frames flow.
                        if migration.is_none() {
                            let fresh = crate::zc::alloc(crate::zc::META_VA, PARTITION);
                            if let Some(r) = fresh {
                                zc::maintain_payload(&r, zc::PayloadCache::BeforeRx).map_err(|_| 171u32)?;
                            }
                            migration = fresh.as_ref().and_then(|fresh| {
                                let pa = crate::zc::buffer_physical(crate::zc::POOL_PA, fresh.id())?;
                                dma.translate_external(pa, crate::zc::BUFFER as u64).ok()
                            });
                            if migration.is_none() {
                                if let Some(fresh) = fresh {
                                    let _ = crate::zc::recycle(crate::zc::META_VA, &fresh);
                                }
                            }
                        }
                        ring.migrate_next_rx(migration);
                    }
                    let consumed = match ring.receive(dma, &mut frame, RX_MASK).map_err(|_| 171u32)? {
                        Rx::Frame(length) => {
                            #[cfg(feature = "rx-gro")]
                            {
                                let drops = ingress.flush();
                                if drops != 0 { status(41).fetch_add(drops, Relaxed); }
                            }
                            if RX_QUEUES > 1 { status(280 + queue_id).fetch_add(1, Relaxed); }
                            status(33).fetch_add(1, Relaxed);
                            status(34).fetch_add(length as u32, Relaxed);
                            status(35).fetch_add(1, Relaxed);
                            status(43).fetch_add(1, Relaxed);
                            let verified=if rx_checksum_requested() && length>=34 && frame[12..15]==[8,0,0x45] {
                                ring.rx_transport_checksum()
                            } else {0};
                            let copy = || queue::push_meta(QUEUE, 0, &frame[..length], verified);
                            #[cfg(feature = "zc")]
                            let queued = if let Some(admission) = lan_worker(LAN_PORT, &frame[..length]) {
                                match (admission, zc::route_revision(zc::FAST_WAN_CONTROL, None)) {
                                    (Some(pin), Some(revision)) => zc::RefQueue::push_copy(zc::fast_lan_requests(LAN_PORT, pin.worker), || {
                                        let tag = (revision << queue::FORWARD_TAG_SHIFT) | ((LAN_PORT as u32 + 1) << 2);
                                        queue::push_meta(zc::fast_lan_frames(LAN_PORT, pin.worker), tag, &frame[..length], verified)
                                    }),
                                    _ => Ok(false),
                                }
                            } else {
                                zc::RefQueue::push_copy(zc::link_queue(zc::LINKS_VA,
                                    if LAN_PORT == 0 { zc::Q_LAN1_RX } else { zc::Q_LAN2_RX }), copy)
                            };
                            #[cfg(not(feature = "zc"))]
                            let queued = copy();
                            if !queued.map_err(|_| 171u32)? {
                                status(41).fetch_add(1, Relaxed);
                            }
                            #[cfg(feature = "zc")]
                            {
                                crate::zc::count(crate::zc::META_VA, crate::zc::C_COPIED_PACKETS);
                                crate::zc::add_copied_bytes(crate::zc::META_VA, length as u32);
                                zc::count_rx_copy(PARTITION, length);
                            }
                            progress(10);
                            busy = true;
                            true
                        }
                        Rx::Rejected => {
                            status(35).fetch_add(1, Relaxed);
                            status(36).fetch_add(1, Relaxed);
                            status(43).fetch_add(1, Relaxed);
                            busy = true;
                            true
                        }
                        Rx::Pending | Rx::External(..) => false
                    };
                    if !consumed { break; }
                    #[cfg(feature = "zc")]
                    {
                        // The re-arm consumed the offered buffer (or armed private).
                        migration = None;
                    }
                }
                #[cfg(feature = "rx-gro")]
                {
                    let drops = ingress.flush();
                    if drops != 0 { status(41).fetch_add(drops, Relaxed); }
                }
            }
            if !busy { break; }
        }
        #[cfg(feature = "zc")]
        if phy & 2 == 0 && option_env!("R5S_WORKER_LAN_EGRESS") == Some("1") {
            for lane in 0..zc::WORKER_TX_LANES {
                let refs = zc::worker_lan_refs(LAN_PORT, lane);
                for _ in 0..queue::batch(option_env!("R5S_B51_TX_BATCH"), queue::COUNT) {
                    let Some(r) = zc::RefQueue::pop(refs).map_err(|_| 171u32)? else {break};
                    if r.flags & zc::FLAG_COPY != 0 {
                        let removed = unsafe {queue::consume_in_place(zc::worker_lan_frames(LAN_PORT, lane), |_, _| ())};
                        if removed != Ok(Some(())) {return Err(171);}
                    } else {
                        let _ = zc::reject(zc::META_VA, &r);
                    }
                    zc::retire_worker_lan_tx(refs);
                    zc::retire_lan_token(zc::META_VA, zc::direct_lan_token(&r));
                    status(41).fetch_add(1, Relaxed);
                    busy = true;
                }
            }
        }
        #[cfg(feature = "zc")]
        if zc::lan_fast_path() {
            for worker in 0..4 {
                let refs = zc::fast_lan_requests(LAN_PORT, worker);
                if zc::RefQueue::depth(refs).is_ok_and(|depth| depth != 0) && zc::RefQueue::claim_wake(refs) {
                    sel4_microkit::Channel::new(10 + worker).notify();
                }
            }
        }
        #[cfg(feature = "tso")]
        tso_publish(&tso);
        if !busy { sel4::r#yield(); }
    })();
    #[cfg(feature = "zc")]
    {
        if option_env!("R5S_WORKER_LAN_EGRESS") == Some("1") {
            for lane in 0..zc::WORKER_TX_LANES {
                zc::close_worker_lan_tx(zc::worker_lan_refs(LAN_PORT, lane));
            }
        }
        let io = dma.registers();
        if stop_and_drain(io, identity) {
            #[cfg(feature = "tso")]
            for pending in tso.software.iter_mut().filter_map(Option::take) {
                for r in pending.refs.iter().flatten() {let _=zc::reject(zc::META_VA,r);}
                if pending.direct!=0 {retire_direct_tso(pending.direct);}
            }
            // A confirmed software discard retires every in-flight submission:
            // buffers, direct-lane tallies and download-fallback return tokens.
            #[cfg(feature = "tso")]
            for pending in tso.pending.iter_mut().filter_map(Option::take) {
                for r in pending.refs.iter().flatten() {
                    if zc::status(zc::META_VA,r.id())==(zc::TX_DMA,r.generation()) {let _=zc::cancel_stopped_tx(zc::META_VA,r);}
                    else {let _=zc::reject(zc::META_VA,r);}
                }
                if pending.direct!=0 {retire_direct_tso(pending.direct);}
            }
            for queue in 0..TX_QUEUES {
                for slot in 0..SLOTS {
                    if let Some(r) = tx_refs[queue][slot].take() {
                        let _ = zc::cancel_stopped_tx(zc::META_VA, &r);
                    }
                    let refs = core::mem::take(&mut direct_tx_slots[queue][slot]);
                    if refs != 0 { retire_direct_tso(refs); }
                    let token = core::mem::take(&mut token_slots[queue][slot]);
                    if token != 0 { zc::retire_lan_token(zc::META_VA, token); }
                }
            }
        }
        if option_env!("R5S_WORKER_LAN_EGRESS") == Some("1") {
            for lane in 0..zc::WORKER_TX_LANES {
                zc::release_worker_lan_consumer(zc::worker_lan_refs(LAN_PORT, lane));
            }
        }
    }
    #[cfg(not(feature = "zc"))]
    let _ = identity;
    result
}

#[cfg(feature = "lan-dhcp")]
fn dhcp_kind(frame: &[u8], reply: bool) -> Option<u8> {
    if frame.get(12..14)? != [8, 0]
        || frame.get(14) != Some(&0x45)
        || frame.get(23) != Some(&17)
        || frame.get(34..38)?
            != if reply {
                [0, 67, 0, 68]
            } else {
                [0, 68, 0, 67]
            }
    {
        return None;
    }
    let size = usize::from(u16::from_be_bytes(frame.get(38..40)?.try_into().ok()?));
    let payload = frame.get(42..34 + size)?;
    if reply {
        r5s_router_core::dhcp::reply_options(payload)
    } else {
        r5s_router_core::dhcp::options(payload)
    }
    .ok()
    .map(|options| options.kind)
}

// ---- LAN TSO glue (docs/tso-design.md §4-§7) ------------------------------

/// Outstanding hardware requests tracked until their LAST descriptor
/// retires. Small and bounded: each request consumes at least two ring
/// slots. The driver defers a queued group while the table is full.
#[cfg(any(feature = "tso", test))]
pub const TSO_PENDING: usize = 8;

#[cfg(any(feature = "tso", test))]
#[derive(Clone, Copy)]
pub struct TsoPending {
    pub tso_id: u32,
    direct: usize,
    pub nfrags: u8,
    pub refs: [Option<crate::zc::Ref>; crate::tso::TSO_MAX_FRAGS],
}

#[cfg(any(feature = "tso", test))]
struct SoftwareTso {
    request: crate::tso::TsoRequest,
    refs: [Option<crate::zc::Ref>; crate::tso::TSO_MAX_FRAGS],
    next: u32,
    direct: usize,
    epoch: u32,
    last: usize,
}

#[cfg(any(feature = "zc", test))]
fn retire_direct_tso(stamp: usize) {
    let refs=(stamp as u32 & !0xff) as usize;
    for _ in 0..(stamp & 0xff).max(1) {crate::zc::retire_worker_lan_tx(refs);}
    crate::zc::retire_lan_token(crate::zc::META_VA, (stamp >> 32) as u32);
}

#[cfg(any(feature = "tso", test))]
pub struct TsoState {
    pub meta: usize,
    pub pool_va: usize,
    pub pool_pa: u64,
    ids: core::sync::atomic::AtomicU32,
    pub pending: [Option<TsoPending>; TSO_PENDING],
    software: [Option<SoftwareTso>; 1 + crate::zc::WORKER_TX_LANES],
    source: usize,
    direct: usize,
    direct_epoch: u32,
    pub requests: u32,
    pub hw_frames: u32,
    pub hw_completed: u32,
    pub hw_segments_est: u32,
    pub fallback_frames: u32,
    pub fallback_segments: u32,
    pub invalid: u32,
    pub desc_errors: u32,
    pub completion_failures: u32,
    pub queue_drops: u32,
}

#[cfg(any(feature = "tso", test))]
impl TsoState {
    pub fn new(meta: usize, pool_va: usize, pool_pa: u64) -> Self {
        Self {
            meta,
            pool_va,
            pool_pa,
            ids: core::sync::atomic::AtomicU32::new(1),
            pending: [None; TSO_PENDING],
            software: [const {None};1 + crate::zc::WORKER_TX_LANES],
            source: 0,
            direct: 0,
            direct_epoch: 0,
            requests: 0,
            hw_frames: 0,
            hw_completed: 0,
            hw_segments_est: 0,
            fallback_frames: 0,
            fallback_segments: 0,
            invalid: 0,
            desc_errors: 0,
            completion_failures: 0,
            queue_drops: 0,
        }
    }

    /// Driver-local nonzero request id (0 marks "no request" in completion
    /// reporting and is skipped on wraparound).
    fn next_id(&self) -> u32 {
        let mut id = self.ids.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if id == 0 {
            id = self.ids.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
        id
    }

    fn reject_all(&mut self, refs: &[crate::zc::Ref]) {
        for r in refs {
            let _ = crate::zc::reject(self.meta, r);
        }
    }

    /// Hand one validated request to the ring: hardware TSO when the ring
    /// accepts it, software segmentation otherwise (contract §5). The
    /// source references are freed in every outcome.
    pub fn submit<I: Registers, const E: bool, const Q: usize, const T: usize>(
        &mut self,
        dma: &mut PackedDma<I, SLOTS, E, Q, T>,
        ring: &mut Ring<SLOTS>,
        request: crate::tso::TsoRequest,
        refs: &[crate::zc::Ref],
        submitted_at: &mut [u64; SLOTS],
        now: u64,
    ) -> Result<(), u32> {
        self.requests = self.requests.saturating_add(1);
        let n = request.nfrags as usize;
        let mut frags = [crate::ring::TsoFrag { bus: 0, length: 0 }; crate::tso::TSO_MAX_FRAGS];
        for (i, frag) in request.frags[..n].iter().enumerate() {
            frags[i] = crate::ring::TsoFrag { bus: frag.physical, length: frag.length as usize };
        }
        let driver = crate::ring::TsoRequest {
            ip_version: request.ip_version,
            protocol: request.protocol,
            ip_hdr_len: request.ip_hdr_len,
            tcp_hdr_len: request.tcp_hdr_len,
            mss: request.mss,
            flags: request.flags,
            head_len: request.head_len,
            payload_len: request.payload_len,
            tso_id: request.tso_id,
            frags: &frags[..n],
        };
        let Some(index)=self.pending.iter().position(Option::is_none).filter(|_| {
            request.flags & crate::tso::SOFTWARE_SEGMENTATION==0
            && request.head_len as u32+request.payload_len.min(u32::from(request.mss))<=1514
        }) else {
            return self.fallback(dma,ring,&request,refs,submitted_at,now);
        };
        let mut record=TsoPending {tso_id:request.tso_id,direct:core::mem::take(&mut self.direct),
            nfrags:n as u8,refs:[None;crate::tso::TSO_MAX_FRAGS]};
        for (i,r) in refs.iter().enumerate() {
            crate::zc::maintain_payload(r,crate::zc::PayloadCache::BeforeTx(crate::zc::BUFFER)).map_err(|_|171u32)?;
            record.refs[i]=Some(*r);
        }
        self.pending[index]=Some(record);
        match ring.send_tso(dma,&driver) {
            Ok(first)=>{
                for (i,r) in refs.iter().enumerate() {
                    let _=crate::zc::submit(self.meta,r);
                    submitted_at[(first+i)%SLOTS]=now;
                }
                self.hw_frames=self.hw_frames.saturating_add(1);
                self.hw_segments_est=self.hw_segments_est.saturating_add(request.payload_len.div_ceil(u32::from(request.mss)));
            }
            Err(crate::ring::Error::Layout | crate::ring::Error::Backend | crate::ring::Error::Stopped)=>{
                self.desc_errors=self.desc_errors.saturating_add(1);
                return Err(171);
            }
            Err(_)=>{
                self.direct=self.pending[index].take().unwrap().direct;
                self.fallback(dma,ring,&request,refs,submitted_at,now)?;
            }
        }
        Ok(())
    }

    fn fallback<I: Registers, const E: bool, const Q: usize, const T: usize>(
        &mut self,
        dma: &mut PackedDma<I, SLOTS, E, Q, T>,
        ring: &mut Ring<SLOTS>,
        request: &crate::tso::TsoRequest,
        refs: &[crate::zc::Ref],
        submitted_at: &mut [u64; SLOTS],
        now: u64,
    ) -> Result<(),u32> {
        let source=self.source;
        if self.software[source].is_some() {return Err(171);}
        let mut pending=SoftwareTso {request:*request,refs:[None;crate::tso::TSO_MAX_FRAGS],
            next:0,direct:core::mem::take(&mut self.direct),epoch:self.direct_epoch,last:0};
        pending.request.mss=pending.request.mss.min(1514u32.saturating_sub(pending.request.head_len) as u16);
        for (i,r) in refs.iter().enumerate() {
            pending.request.frags[i].physical=(self.pool_va+r.id() as usize*crate::zc::BUFFER) as u64;
            pending.refs[i]=Some(*r);
        }
        let direct=pending.direct!=0;
        self.software[source]=Some(pending);
        if !direct {let _=self.resume_software(source,dma,ring,submitted_at,now,if cfg!(test) {16} else {1})?;}
        Ok(())
    }

    fn resume_software<I: Registers,const E: bool,const Q: usize,const T: usize>(
        &mut self,source:usize,dma:&mut PackedDma<I,SLOTS,E,Q,T>,ring:&mut Ring<SLOTS>,
        submitted_at:&mut [u64;SLOTS],now:u64,budget:u32,
    ) -> Result<(bool,Option<(usize,usize)>),u32> {
        let Some(pending)=self.software[source].as_mut() else {return Ok((false,None));};
        let stale=pending.direct!=0 && {
            let first=pending.refs[0].unwrap();
            let bytes=unsafe {core::slice::from_raw_parts(pending.request.frags[0].physical as *const u8,first.len as usize)};
            !crate::zc::worker_lan_tx_valid(crate::zc::FAST_WAN_CONTROL,bytes,cfg!(feature="lan2") as usize,pending.epoch,now)
        };
        let before=pending.next;
        let mut scratch=[0u8;2161];
        let mut backend_error=false;
        let limit=budget.min((SLOTS-1-dma.tx_inflight()) as u32);
        let result=if stale {Ok(true)} else {
            crate::tso::segment_window(&pending.request,&mut scratch,&mut pending.next,limit,&mut |frame| {
                let slot=(dma.oldest_tx_slot()+dma.tx_inflight())%SLOTS;
                match ring.send(dma,frame) {
                    Ok(())=>{submitted_at[slot]=now;pending.last=slot;Ok(())}
                    Err(crate::ring::Error::Busy)=>Err(crate::tso::TsoError::Busy),
                    Err(_)=>{backend_error=true;Err(crate::tso::TsoError::Busy)}
                }
            })
        };
        if backend_error {return Err(171);}
        let progress=pending.next!=before;
        if matches!(result,Ok(false)|Err(crate::tso::TsoError::Busy)) {return Ok((progress,None));}
        let done=self.software[source].take().unwrap();
        let fence=if done.direct==0 {None}
            else if done.next!=0 && (done.last+SLOTS-dma.oldest_tx_slot())%SLOTS<dma.tx_inflight() {
                Some((done.last,done.direct))
            } else {retire_direct_tso(done.direct);None};
        for r in done.refs.iter().flatten() {let _=crate::zc::reject(self.meta,r);}
        if stale || result.is_err() {self.invalid=self.invalid.saturating_add(1);}
        else {
            self.fallback_frames=self.fallback_frames.saturating_add(1);
            self.fallback_segments=self.fallback_segments.saturating_add(done.next);
            if done.request.flags & crate::tso::GRO_SOFTWARE!=0 {
                crate::zc::counter(self.meta,crate::zc::C_GRO_SW_SEGMENTS).fetch_add(done.next,core::sync::atomic::Ordering::Relaxed);
            }
        }
        Ok((true,fence))
    }

    /// Release one request's buffers once `retired_tso` reports its id
    /// (contract §4/§1): nothing is freed before the LAST descriptor
    /// retires, and a report without a tracked request counts as an
    /// anomaly.
    pub fn reclaim(&mut self, ring: &mut Ring<SLOTS>, oldest: usize, completed: usize) {
        if completed == 0 {
            return;
        }
        let mut out = [0u32; 32];
        let n = ring.retired_tso(oldest, completed, &mut out);
        for id in &out[..n] {
            let found = self.pending.iter_mut().find(|p| p.is_some_and(|q| q.tso_id == *id));
            match found {
                Some(slot) => {
                    let done = slot.take().unwrap();
                    self.hw_completed = self.hw_completed.saturating_add(1);
                    if done.direct!=0 {retire_direct_tso(done.direct);}
                    for r in done.refs.iter().flatten() {
                        let _ = crate::zc::complete(self.meta, r);
                    }
                }
                None => self.completion_failures = self.completion_failures.saturating_add(1),
            }
        }
    }
}

/// Drain one TSO reference group starting at the popped FLAG_TSO_FIRST
/// reference (contract §6 ref-group path). Malformed groups free every
/// collected reference and count as invalid; the queue is SPSC so a group
/// is contiguous.
#[cfg(any(feature = "tso", test))]
pub fn tso_group<I: Registers, const E: bool, const Q: usize, const T: usize>(
    state: &mut TsoState,
    dma: &mut PackedDma<I, SLOTS, E, Q, T>,
    ring: &mut Ring<SLOTS>,
    queue: usize,
    first: crate::zc::Ref,
    submitted_at: &mut [u64; SLOTS],
    now: u64,
) -> Result<(), u32> {
    use crate::tso::{GroupCollector, GroupRef, FLAG_TSO_CONT, FLAG_TSO_LAST, TSO_MAX_FRAGS};
    let mut collector = GroupCollector::new();
    let mut refs = [first; TSO_MAX_FRAGS];
    let mut n = 0usize;
    let mut cur = first;
    let mut bad = false;
    let mut orphan: Option<crate::zc::Ref> = None;
    loop {
        if crate::zc::status(state.meta, cur.id()) != (crate::zc::CPU, cur.generation()) {
            crate::zc::count(state.meta, crate::zc::C_OWNERSHIP_ERRORS);
            bad = true;
            break;
        }
        let bus = crate::zc::buffer_physical(state.pool_pa, cur.id())
            .and_then(|pa| dma.translate_external(pa, crate::zc::BUFFER as u64).ok());
        let Some(bus) = bus else {
            crate::zc::count(state.meta, crate::zc::C_MAPPING_FAILURES);
            orphan = Some(cur);
            bad = true;
            break;
        };
        refs[n] = cur;
        n += 1;
        let group = GroupRef { flags: cur.flags, meta: cur.meta, len: cur.len };
        match collector.push(group, bus) {
            Ok(true) => break,
            Ok(false) => {}
            Err(_) => {
                bad = true;
                break;
            }
        }
        if n == TSO_MAX_FRAGS {
            // Group over capacity without LAST: the next reference stays
            // queued and the collected run is rejected below.
            bad = true;
            break;
        }
        match crate::zc::RefQueue::peek(queue) {
            Ok(Some(next)) if next.flags & (FLAG_TSO_CONT | FLAG_TSO_LAST) != 0 => {
                let _ = crate::zc::RefQueue::pop(queue);
                cur = next;
            }
            Ok(Some(_)) => {
                bad = true;
                break;
            }
            _ => {
                bad = true;
                break;
            }
        }
    }
    if bad {
        state.invalid = state.invalid.saturating_add(1);
        for r in &refs[..n] {
            let _ = crate::zc::reject(state.meta, r);
        }
        if let Some(r) = orphan {
            let _ = crate::zc::reject(state.meta, &r);
        }
        return Ok(());
    }
    let id = state.next_id();
    match collector.request(id) {
        Ok(request) => state.submit(dma, ring, request, &refs[..n], submitted_at, now),
        Err(_) => {
            state.invalid = state.invalid.saturating_add(1);
            for r in &refs[..n] {
                let _ = crate::zc::reject(state.meta, r);
            }
            Ok(())
        }
    }
}

/// Board test source (contract §9): three oversized IPv4 requests built
/// over freshly allocated pool buffers and submitted through the same
/// interface as queue groups. Runs once per PD lifetime once the link is
/// up and the ring is idle; as the first id consumer it receives 1..=3.
#[cfg(any(feature = "tso-test", test))]
pub fn tso_testgen_run<I: Registers, const E: bool, const Q: usize, const T: usize>(
    state: &mut TsoState,
    dma: &mut PackedDma<I, SLOTS, E, Q, T>,
    ring: &mut Ring<SLOTS>,
    partition: usize,
    submitted_at: &mut [u64; SLOTS],
    now: u64,
) -> Result<(), u32> {
    use crate::tso::TSO_MAX_FRAGS;
    for _ in 0..3 {
        let mut refs = [None; TSO_MAX_FRAGS];
        for slot in &mut refs {
            *slot = crate::zc::alloc(state.meta, partition);
        }
        if refs.iter().any(|r| r.is_none()) {
            for r in refs.iter().flatten() {
                let _ = crate::zc::recycle(state.meta, r);
            }
            return Ok(());
        }
        let mut owned = [crate::zc::Ref::new(0, 0); TSO_MAX_FRAGS];
        let mut addrs = [0u64; TSO_MAX_FRAGS];
        let mut mapped = true;
        for (i, r) in refs.iter().enumerate() {
            let r = r.unwrap();
            let _ = crate::zc::publish(state.meta, &r);
            owned[i] = r;
            addrs[i] = match crate::zc::buffer_physical(state.pool_pa, r.id())
                .and_then(|pa| dma.translate_external(pa, crate::zc::BUFFER as u64).ok())
            {
                Some(bus) => bus,
                None => {
                    mapped = false;
                    0
                }
            };
        }
        if !mapped {
            for r in &owned {
                let _ = crate::zc::reject(state.meta, r);
            }
            return Ok(());
        }
        let mut slices: [&mut [u8]; TSO_MAX_FRAGS] = core::array::from_fn(|i| unsafe {
            core::slice::from_raw_parts_mut(
                (state.pool_va + owned[i].id() as usize * crate::zc::BUFFER) as *mut u8,
                crate::zc::BUFFER,
            )
        });
        let spec = crate::tso_testgen::TestSpec::default_ipv4(state.next_id());
        match crate::tso_testgen::build(&spec, &mut slices, &addrs) {
            Ok(request) => state.submit(dma, ring, request, &owned, submitted_at, now)?,
            Err(_) => {
                state.invalid = state.invalid.saturating_add(1);
                for r in &owned {
                    let _ = crate::zc::reject(state.meta, r);
                }
            }
        }
    }
    Ok(())
}

/// Publish the contract §7 counter block at status(264..=275); 273 marks
/// the running feature, 274 is stored at probe time, 275 carries flags.
#[cfg(feature = "tso")]
fn tso_publish(state: &TsoState) {
    use core::sync::atomic::Ordering::Relaxed;
    crate::status(275).store(crate::tso::status_flags(crate::tso::TSO_IPV6, state.hw_completed), Relaxed);
    for (i, value) in [
        state.requests,
        state.hw_frames,
        state.hw_segments_est,
        state.fallback_frames,
        state.fallback_segments,
        state.invalid,
        state.desc_errors,
        state.completion_failures,
        state.queue_drops,
    ]
    .into_iter()
    .enumerate()
    {
        crate::status(264 + i).store(value, Relaxed);
    }
}
