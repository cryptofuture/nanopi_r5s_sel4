//! Outbound configuration access for the captured RK3568 single-lane ports.

#[derive(Debug, PartialEq, Eq)]
pub struct ConfigRead {
    pub address: u64,
    pub outbound: Option<Outbound>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Outbound {
    pub base: u64,
    pub limit: u64,
    pub target: u32,
    pub transaction_type: u32,
}

pub fn config_read(
    dbi: u64,
    aperture: (u64, u64),
    buses: (u8, u8),
    bus: u8,
    device: u8,
    function: u8,
    offset: u16,
) -> Option<ConfigRead> {
    let relative = bus.checked_sub(buses.0)?;
    if bus > buses.1 || device > 31 || function > 7 || offset >= 4096 || offset % 4 != 0 {
        return None;
    }
    if relative <= 1 && device != 0 {
        return None;
    }
    if relative == 0 {
        if function != 0 {
            return None;
        }
        return Some(ConfigRead { address: dbi.checked_add(offset.into())?, outbound: None });
    }
    let (base, size) = aperture;
    if size < 4096 || base % 4096 != 0 || size % 4096 != 0 {
        return None;
    }
    Some(ConfigRead {
        address: base.checked_add(offset.into())?,
        outbound: Some(Outbound {
            base,
            limit: base.checked_add(size - 1)?,
            target: (u32::from(relative) << 24) | (u32::from(device) << 19) | (u32::from(function) << 16),
            transaction_type: if relative == 1 { 4 } else { 5 },
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_relative_buses_and_preserves_high_dbi_bits() {
        let dbi = 0x3c0400000;
        let aperture = (0xf2000000, 0x100000);
        let buses = (16, 31);
        assert_eq!(config_read(dbi, aperture, buses, 16, 0, 0, 0x18).unwrap().address, dbi + 0x18);
        assert_eq!(config_read(dbi, aperture, buses, 17, 0, 0, 0).unwrap().outbound,
                   Some(Outbound { base: aperture.0, limit: 0xf20fffff, target: 0x01000000, transaction_type: 4 }));
        let downstream = config_read(dbi, aperture, buses, 18, 3, 1, 0).unwrap().outbound.unwrap();
        assert_eq!((downstream.target, downstream.transaction_type), (0x02190000, 5));
    }

    #[test]
    fn rejects_unsafe_config_addresses() {
        for (bus, device, function, offset) in [(15,0,0,0), (32,0,0,0), (17,1,0,0), (16,0,1,0), (17,0,0,2)] {
            assert!(config_read(0x3c0400000, (0xf2000000, 0x100000), (16,31), bus, device, function, offset).is_none());
        }
        assert!(config_read(0, (u64::MAX - 4095, 8192), (0,15), 1, 0, 0, 0).is_none());
    }
}

pub fn conventional_capability(mut read: impl FnMut(u16) -> Option<u32>, kind: u8) -> Option<u16> {
    let mut next = (read(0x34)? & 0xff) as u16;
    let mut visited = 0u64;
    while (0x40..=0xfc).contains(&next) && next & 3 == 0 {
        let bit = 1u64 << (next / 4);
        if visited & bit != 0 { return None; }
        visited |= bit;
        let header = read(next)?;
        if header as u8 == kind { return Some(next); }
        next = ((header >> 8) & 0xff) as u16;
    }
    None
}

pub fn link_snapshot(mut read: impl FnMut(u16) -> Option<u32>) -> Option<u32> {
    let base = conventional_capability(&mut read, 0x10)?;
    if base > 0xec { return None; }
    let device = read(base + 4)?;
    let control = read(base + 8)?;
    let link = read(base + 0x10)? >> 16;
    if [device, control].contains(&u32::MAX) || link == 0xffff { return None; }
    Some(1 << 31 | (device & 7) << 16 | ((control >> 5) & 7) << 19
         | ((control >> 12) & 7) << 22 | (link & 0x3ff))
}

#[test]
fn pcie_link_snapshot_is_read_only_and_detects_corrupt_lists() {
    let read = |offset| match offset {
        0x34 => Some(0x50),
        0x50 => Some(0x7005),
        0x70 => Some(0x10),
        0x74 => Some(2),
        0x78 => Some(1 << 5 | 3 << 12),
        0x80 => Some(0x13 << 16),
        _ => None,
    };
    assert_eq!(link_snapshot(read), Some(1 << 31 | 2 << 16 | 1 << 19 | 3 << 22 | 0x13));
    assert_eq!(conventional_capability(|i| (i == 0x34 || i == 0x50).then_some(
        if i == 0x34 { 0x50 } else { 0x5005 }), 0x10), None);
}

pub trait ConfigIo {
    type Error;
    fn read32(&mut self, address: u64) -> Result<u32, Self::Error>;
    fn write32(&mut self, address: u64, value: u32) -> Result<(), Self::Error>;
    fn delay_us(&mut self, microseconds: u32);
    fn startup_phase(&mut self, _phase: u32) {}
    fn inbound_step(&mut self, _operation: u32, _category: u32, _address: u64, _expected: u32, _observed: u32) {}
}

impl<I: ConfigIo> ConfigIo for &mut I {
    type Error = I::Error;
    fn read32(&mut self, address: u64) -> Result<u32, Self::Error> { (**self).read32(address) }
    fn write32(&mut self, address: u64, value: u32) -> Result<(), Self::Error> { (**self).write32(address, value) }
    fn delay_us(&mut self, us: u32) { (**self).delay_us(us) }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProbeError<E> {
    InvalidLayout,
    WindowTimeout,
    InboundMode,
    InboundAlignment,
    InboundWidth,
    Io(E),
    UnexpectedIdentity(u32),
}

pub(crate) fn set_enabled<I: ConfigIo>(
    io: &mut I, control: u64, enable: bool,
) -> Result<(), ProbeError<I::Error>> {
    const ENABLE: u32 = 1 << 31;
    io.write32(control, if enable { ENABLE } else { 0 }).map_err(ProbeError::Io)?;
    for _ in 0..5 {
        if (io.read32(control).map_err(ProbeError::Io)? & ENABLE != 0) == enable {
            return Ok(());
        }
        io.delay_us(10_000);
    }
    Err(ProbeError::WindowTimeout)
}

/// Call after link training and bridge bus numbering. The caller owns outbound
/// region 1 exclusively and supplies ordered Device-memory register access.
fn config_access<I: ConfigIo, T>(
    io: &mut I,
    dbi: u64,
    aperture: (u64, u64),
    buses: (u8, u8),
    location: (u8, u8, u8),
    offset: u16,
    access: impl FnOnce(&mut I, u64) -> Result<T, I::Error>,
) -> Result<T, ProbeError<I::Error>> {
    let (bus, device, function) = location;
    let read = config_read(dbi, aperture, buses, bus, device, function, offset)
        .ok_or(ProbeError::InvalidLayout)?;
    let Some(window) = read.outbound else {
        return access(io, read.address).map_err(ProbeError::Io);
    };
    let atu = dbi.checked_add(0x300200).ok_or(ProbeError::InvalidLayout)?;
    atu.checked_add(0x20).ok_or(ProbeError::InvalidLayout)?;

    set_enabled(io, atu + 4, false)?;
    for (register, value) in [
        (0x08, window.base as u32),
        (0x0c, (window.base >> 32) as u32),
        (0x10, window.limit as u32),
        (0x20, (window.limit >> 32) as u32),
        (0x14, window.target),
        (0x18, 0),
        (0x00, window.transaction_type),
    ] {
        io.write32(atu + register, value).map_err(ProbeError::Io)?;
    }
    let identity = set_enabled(io, atu + 4, true)
        .and_then(|()| access(io, read.address).map_err(ProbeError::Io));
    set_enabled(io, atu + 4, false)?;
    identity
}

pub fn read_config<I: ConfigIo>(
    io: &mut I, dbi: u64, aperture: (u64, u64), buses: (u8, u8),
    location: (u8, u8, u8), offset: u16,
) -> Result<u32, ProbeError<I::Error>> {
    config_access(io, dbi, aperture, buses, location, offset, I::read32)
}

pub fn probe_rtl8125<I: ConfigIo>(
    io: &mut I,
    dbi: u64,
    aperture: (u64, u64),
    buses: (u8, u8),
) -> Result<(), ProbeError<I::Error>> {
    let secondary = buses.0.checked_add(1).ok_or(ProbeError::InvalidLayout)?;
    let identity = read_config(io, dbi, aperture, buses, (secondary, 0, 0), 0)?;
    if identity != 0x8125_10ec {
        return Err(ProbeError::UnexpectedIdentity(identity));
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn probes_only_after_window_enable_and_releases_it() {
    const CONTROL: u64 = 0x3c0300204;
    struct Mock {
        control: u32,
        stuck: bool,
        reads: usize,
        delays: usize,
    }
    impl ConfigIo for Mock {
        type Error = ();
        fn read32(&mut self, address: u64) -> Result<u32, ()> {
            if address == CONTROL {
                return Ok(self.control);
            }
            assert_eq!(address, 0xf4000000);
            assert_eq!(self.control, 1 << 31);
            self.reads += 1;
            Ok(0x8125_10ec)
        }
        fn write32(&mut self, address: u64, value: u32) -> Result<(), ()> {
            assert!((CONTROL - 4..=CONTROL + 28).contains(&address));
            if address == CONTROL && !self.stuck {
                self.control = value;
            }
            Ok(())
        }
        fn delay_us(&mut self, _: u32) {
            self.delays += 1;
        }
    }
    for stuck in [false, true] {
        let mut io = Mock { control: 0, stuck, reads: 0, delays: 0 };
        let result = probe_rtl8125(&mut io, 0x3c0000000, (0xf4000000, 0x100000), (0, 15));
        assert_eq!(io.control, 0);
        if stuck {
            assert_eq!(result, Err(ProbeError::WindowTimeout));
            assert_eq!((io.reads, io.delays), (0, 5));
        } else {
            assert_eq!(result, Ok(()));
            assert_eq!(io.reads, 1);
        }
    }
}

/// Size a memory BAR from its mask after a PCI sizing transaction with decoding disabled.
/// A 64-bit BAR consumes the following slot. I/O, reserved and unimplemented BARs fail closed.
pub fn memory_bar_size(original: u32, mask_low: u32, mask_high: Option<u32>) -> Option<u64> {
    if original & 1 != 0 || mask_low == 0 {
        return None;
    }
    let low = mask_low & !0xf;
    let (mask, width) = match ((original >> 1) & 3, mask_high) {
        (0, None) => (u64::from(low), u64::from(u32::MAX)),
        (2, Some(high)) => ((u64::from(high) << 32) | u64::from(low), u64::MAX),
        _ => return None,
    };
    let size = (width ^ mask).checked_add(1)?;
    if size < 16 || !size.is_power_of_two() {
        return None;
    }
    Some(size)
}

/// Place a BAR in a captured outbound aperture. CPU and PCI bus addresses
/// may differ, and alignment applies to the address programmed into the BAR.
pub fn allocate_bar(
    cpu_base: u64,
    bus_base: u64,
    window_size: u64,
    used: u64,
    size: u64,
    wide: bool,
) -> Option<(u64, u64, u64)> {
    if size < 16 || !size.is_power_of_two() {
        return None;
    }
    let bus = bus_base.checked_add(used)?.checked_add(size - 1)? & !(size - 1);
    let offset = bus.checked_sub(bus_base)?;
    let end = offset.checked_add(size)?;
    let last_bus = bus.checked_add(size - 1)?;
    let cpu = cpu_base.checked_add(offset)?;
    cpu.checked_add(size - 1)?;
    if end > window_size || (!wide && last_bus > u64::from(u32::MAX)) {
        return None;
    }
    Some((cpu, bus, end))
}

#[cfg(test)]
#[test]
fn bar_sizing_and_nonidentity_translation() {
    assert_eq!(memory_bar_size(0, 0xffff_c000, None), Some(0x4000));
    assert_eq!(memory_bar_size(4, 0xffff_c004, Some(u32::MAX)), Some(0x4000));
    assert_eq!(memory_bar_size(4, 0x0000_0004, Some(u32::MAX)), Some(1 << 32));
    assert_eq!(memory_bar_size(0, 0xffff_a000, None), None);
    assert_eq!(memory_bar_size(1, 0xffff_c001, None), None);
    assert_eq!(memory_bar_size(4, 0xffff_c004, None), None);
    assert_eq!(allocate_bar(0xf4200000, 0x41000000, 0x100000, 0x1000, 0x4000, false),
               Some((0xf4204000, 0x41004000, 0x8000)));
    assert!(allocate_bar(0xf4200000, 0x41000000, 0x7000, 0x1000, 0x4000, false).is_none());
    assert!(allocate_bar(0x300000000, 0x300000000, 0x100000, 0, 0x4000, false).is_none());
    assert_eq!(allocate_bar(0x300000000, 0x300000000, 0x100000, 0, 0x4000, true),
               Some((0x300000000, 0x300000000, 0x4000)));
}

pub trait LinkIo: ConfigIo {
    fn set_perst(&mut self, asserted: bool) -> Result<(), Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum LinkError<E> {
    InvalidLayout,
    Io(E),
    Timeout,
}

/// Run the RK3568 LTSSM sequence after clocks, PHY, RC mode and DBI lane
/// configuration are established. The port must have no active traffic.
pub fn train_link<I: LinkIo>(io: &mut I, apb: u64) -> Result<(), LinkError<I::Error>> {
    let status = apb.checked_add(0x300).ok_or(LinkError::InvalidLayout)?;
    io.write32(apb, 0x000c0008).map_err(LinkError::Io)?;
    io.set_perst(true).map_err(LinkError::Io)?;
    let training = (|| {
        io.write32(apb + 0x104, 0).map_err(LinkError::Io)?;
        io.write32(apb, 0x000c000c).map_err(LinkError::Io)?;
        io.delay_us(100_000);
        io.set_perst(false).map_err(LinkError::Io)?;
        for _ in 0..10 {
            if io.read32(status).map_err(LinkError::Io)? & 0x3003f == 0x30011 {
                return Ok(());
            }
            io.delay_us(100_000);
        }
        Err(LinkError::Timeout)
    })();
    if training.is_err() {
        let reset = io.set_perst(true);
        let disable = io.write32(apb, 0x000c0008);
        reset.and(disable).map_err(LinkError::Io)?;
    }
    training
}

#[cfg(test)]
#[test]
fn link_training_waits_for_refclk_and_resets_on_timeout() {
    const APB: u64 = 0xfe260000;
    struct Mock { elapsed: u32, reset: bool, enabled: bool, ready: bool }
    impl ConfigIo for Mock {
        type Error = ();
        fn read32(&mut self, address: u64) -> Result<u32, ()> {
            assert_eq!(address, APB + 0x300);
            assert!(self.enabled && !self.reset && self.elapsed >= 100_000);
            Ok(if self.ready { 0x30011 } else { 0x30010 })
        }
        fn write32(&mut self, address: u64, value: u32) -> Result<(), ()> {
            match (address - APB, value) {
                (0, 0x000c0008) => self.enabled = false,
                (0, 0x000c000c) => { assert!(self.reset); self.enabled = true; }
                (0x104, 0) => assert!(!self.enabled && self.reset),
                _ => panic!("unexpected APB write"),
            }
            Ok(())
        }
        fn delay_us(&mut self, us: u32) { self.elapsed += us; }
    }
    impl LinkIo for Mock {
        fn set_perst(&mut self, asserted: bool) -> Result<(), ()> {
            if !asserted { assert!(self.elapsed >= 100_000); }
            self.reset = asserted;
            Ok(())
        }
    }
    for ready in [true, false] {
        let mut io = Mock { elapsed: 0, reset: false, enabled: false, ready };
        assert_eq!(train_link(&mut io, APB), if ready { Ok(()) } else { Err(LinkError::Timeout) });
        assert_eq!(io.enabled, ready);
        assert_eq!(io.reset, !ready);
        assert_eq!(io.elapsed, if ready { 100_000 } else { 1_100_000 });
    }
}

/// Exclusive access to one identified endpoint. The backend must use a
/// 16-bit command write, or write zero to the W1C status halfword.
pub trait EndpointConfig {
    type Error;
    fn read_dword(&mut self, offset: u16) -> Result<u32, Self::Error>;
    fn write_dword(&mut self, offset: u16, value: u32) -> Result<(), Self::Error>;
    fn disable_decode(&mut self) -> Result<(), Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum TransactionError<E> {
    Layout,
    Io(E),
    Readback,
}

pub fn tune_endpoint_transactions<E: EndpointConfig>(
    io: &mut E, root_link: u32, mps: Option<u16>, mrrs: Option<u16>,
) -> Result<(), TransactionError<E::Error>> {
    let encode = |bytes: u16| match bytes {
        128 | 256 | 512 | 1024 | 2048 | 4096 => Ok(bytes.trailing_zeros() - 7),
        _ => Err(TransactionError::Layout),
    };
    let mps = mps.map(encode).transpose()?;
    let mrrs = mrrs.map(encode).transpose()?;
    if root_link & (1 << 31) == 0 || root_link & 0xf == 0
        || root_link & 0x3f0 == 0 || (root_link >> 19) & 7 > (root_link >> 16) & 7
        || mrrs.is_some_and(|value| !(2..=4).contains(&value))
    {
        return Err(TransactionError::Layout);
    }
    let base = conventional_capability(|offset| io.read_dword(offset).ok(), 0x10)
        .filter(|offset| *offset <= 0xec).ok_or(TransactionError::Layout)?;
    let command = io.read_dword(4).map_err(TransactionError::Io)?;
    let device = io.read_dword(base + 4).map_err(TransactionError::Io)?;
    let control = io.read_dword(base + 8).map_err(TransactionError::Io)?;
    let link = io.read_dword(base + 0x10).map_err(TransactionError::Io)? >> 16;
    let upstream_mps = (root_link >> 19) & 7;
    let endpoint_mps = mps.unwrap_or((control >> 5) & 7);
    if command & 4 != 0 || [device, control].contains(&u32::MAX)
        || link & 0x3ff != root_link & 0x3ff
        || endpoint_mps > (device & 7).min(upstream_mps).min(5)
    {
        return Err(TransactionError::Layout);
    }
    let mut requested = control & 0xffff;
    if let Some(mps) = mps {
        requested = (requested & !(7 << 5)) | mps << 5;
    }
    if let Some(mrrs) = mrrs {
        requested = (requested & !(7 << 12)) | mrrs << 12;
    }
    if requested != control & 0xffff {
        // The upper halfword is Device Status with write-one-to-clear bits.
        io.write_dword(base + 8, requested).map_err(TransactionError::Io)?;
        let observed = io.read_dword(base + 8).map_err(TransactionError::Io)?;
        if observed & 0xffff != requested {
            io.write_dword(base + 8, control & 0xffff).map_err(TransactionError::Io)?;
            return Err(TransactionError::Readback);
        }
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn endpoint_transaction_parameters_respect_link_and_preserve_status() {
    struct Endpoint {
        command: u32,
        control: u32,
        writes: u32,
        ignore_control: bool,
    }
    impl EndpointConfig for Endpoint {
        type Error = ();
        fn read_dword(&mut self, offset: u16) -> Result<u32, ()> {
            match offset {
                4 => Ok(self.command),
                0x34 => Ok(0x70),
                0x70 => Ok(0x10),
                0x74 => Ok(2),
                0x78 => Ok(self.control | 0x000f_0000),
                0x80 => Ok(0x13 << 16),
                _ => Err(()),
            }
        }
        fn write_dword(&mut self, offset: u16, value: u32) -> Result<(), ()> {
            assert_eq!(offset, 0x78);
            assert_eq!(value >> 16, 0);
            self.writes += 1;
            if !self.ignore_control {
                self.control = value;
            }
            Ok(())
        }
        fn disable_decode(&mut self) -> Result<(), ()> { unreachable!() }
    }
    let root = 1 << 31 | 2 << 16 | 1 << 19 | 0x13;
    let mut io = Endpoint { command: 2, control: 0x2018, writes: 0, ignore_control: false };
    assert_eq!(tune_endpoint_transactions(&mut io, root, Some(512), None), Err(TransactionError::Layout));
    assert_eq!(tune_endpoint_transactions(&mut io, root ^ 1, None, Some(1024)), Err(TransactionError::Layout));
    io.command |= 4;
    assert_eq!(tune_endpoint_transactions(&mut io, root, None, Some(1024)), Err(TransactionError::Layout));
    io.command &= !4;
    assert_eq!(io.writes, 0);
    for (bytes, encoding) in [(512, 2), (1024, 3), (2048, 4)] {
        tune_endpoint_transactions(&mut io, root, Some(256), Some(bytes)).unwrap();
        assert_eq!(io.control, encoding << 12 | 0x38);
    }
    assert_eq!(io.writes, 3);
    tune_endpoint_transactions(&mut io, root, Some(256), Some(2048)).unwrap();
    assert_eq!(io.writes, 3);
    io.ignore_control = true;
    assert_eq!(tune_endpoint_transactions(&mut io, root, None, Some(512)), Err(TransactionError::Readback));
    assert_eq!(io.writes, 5);
    assert_eq!(io.control, 0x4038);
}

#[derive(Debug, PartialEq, Eq)]
pub enum BarError<E> {
    InvalidSlot,
    InvalidBar,
    Io(E),
}

/// Probe one BAR on an exclusively owned, stopped endpoint. Decoding and
/// bus mastering remain disabled on all return paths after a successful
/// disable. The original BAR contents are restored before returning.
pub fn size_bar<E: EndpointConfig>(
    io: &mut E, slot: u8,
) -> Result<(u64, bool), BarError<E::Error>> {
    if slot >= 6 {
        return Err(BarError::InvalidSlot);
    }
    io.disable_decode().map_err(BarError::Io)?;
    let offset = 0x10 + u16::from(slot) * 4;
    let low = io.read_dword(offset).map_err(BarError::Io)?;
    if low & 1 != 0 || !matches!((low >> 1) & 3, 0 | 2) {
        return Err(BarError::InvalidBar);
    }
    let wide = low & 6 == 4;
    let high = if wide {
        if slot == 5 {
            return Err(BarError::InvalidSlot);
        }
        Some(io.read_dword(offset + 4).map_err(BarError::Io)?)
    } else {
        None
    };
    let result = (|| {
        io.write_dword(offset, u32::MAX).map_err(BarError::Io)?;
        if wide {
            io.write_dword(offset + 4, u32::MAX).map_err(BarError::Io)?;
        }
        let mask = io.read_dword(offset).map_err(BarError::Io)?;
        let mask_high = if wide {
            Some(io.read_dword(offset + 4).map_err(BarError::Io)?)
        } else {
            None
        };
        memory_bar_size(low, mask, mask_high).map(|size| (size, wide)).ok_or(BarError::InvalidBar)
    })();
    let restore_high = match high {
        Some(value) => io.write_dword(offset + 4, value),
        None => Ok(()),
    };
    let restore_low = io.write_dword(offset, low);
    restore_high.and(restore_low).map_err(BarError::Io)?;
    result
}

#[cfg(test)]
#[test]
fn sizing_restores_bar_even_if_mask_read_fails() {
    struct Endpoint {
        words: [u32; 2],
        disabled: bool,
        fail: bool,
    }
    impl EndpointConfig for Endpoint {
        type Error = ();
        fn read_dword(&mut self, off: u16) -> Result<u32, ()> {
            assert!(self.disabled);
            let index = (off - 0x10) as usize / 4;
            if self.words[index] == u32::MAX {
                if self.fail { return Err(()); }
                return Ok(if index == 0 { 0xffff_c004 } else { u32::MAX });
            }
            Ok(self.words[index])
        }
        fn write_dword(&mut self, off: u16, value: u32) -> Result<(), ()> {
            assert!(self.disabled);
            self.words[(off - 0x10) as usize / 4] = value;
            Ok(())
        }
        fn disable_decode(&mut self) -> Result<(), ()> {
            self.disabled = true;
            Ok(())
        }
    }
    let original = [0xf4200004, 3];
    for fail in [false, true] {
        let mut io = Endpoint { words: original, disabled: false, fail };
        assert_eq!(size_bar(&mut io, 0), if fail { Err(BarError::Io(())) } else { Ok((0x4000,true)) });
        assert_eq!(io.words, original);
        assert!(io.disabled);
    }
}

/// Configure the root port while its endpoint is in PERST and LTSSM is off.
/// Bus numbers are relative to the controller, as in the CFG0/CFG1 requests.
/// Decoding and bus mastering remain off until outbound and DMA setup.
pub fn configure_root<I: ConfigIo>(
    io: &mut I, apb: u64, dbi: u64, buses: (u8, u8), generation: u8,
) -> Result<(), LinkError<I::Error>> {
    let subordinate = buses.1.checked_sub(buses.0).ok_or(LinkError::InvalidLayout)?;
    if subordinate == 0 || !matches!(generation, 1..=3)
        || dbi.checked_add(0x100014).is_none() || apb.checked_add(0x180).is_none()
    {
        return Err(LinkError::InvalidLayout);
    }
    io.write32(apb, 0x000c0008).map_err(LinkError::Io)?;
    io.write32(apb + 0x180, 0x00100010).map_err(LinkError::Io)?;
    io.write32(apb, 0x00f00040).map_err(LinkError::Io)?;

    // Clear command and write zero to the W1C status bits.
    io.write32(dbi + 0x04, 0).map_err(LinkError::Io)?;
    let bus_register = dbi + 0x18;
    let latency = io.read32(bus_register).map_err(LinkError::Io)? & 0xff000000;
    let bus_config = latency | (u32::from(subordinate) << 16) | 0x0100;
    io.write32(bus_register, bus_config).map_err(LinkError::Io)?;
    if io.read32(bus_register).map_err(LinkError::Io)? & 0xffffff != bus_config & 0xffffff {
        return Err(LinkError::InvalidLayout);
    }

    let dbi_protect = dbi + 0x8bc;
    let protect = io.read32(dbi_protect).map_err(LinkError::Io)? & !1;
    io.write32(dbi_protect, protect | 1).map_err(LinkError::Io)?;
    let configured = (|| {
        io.write32(dbi + 0x100010, 0)?;
        io.write32(dbi + 0x100014, 0)?;
        for (offset, mask, value) in [
            (0x08, 0xffff0000, 0x06040000),
            (0x7c, 0xf, u32::from(generation)),
            (0xa0, 0xf, u32::from(generation)),
            (0x710, 0x003f0080, 0x00010000),
            (0x80c, 0x00001f00, 0x00020100),
        ] {
            let register = dbi + offset;
            let previous = io.read32(register)?;
            io.write32(register, (previous & !mask) | value)?;
        }
        Ok(())
    })();
    io.write32(dbi_protect, protect).map_err(LinkError::Io)?;
    configured.map_err(LinkError::Io)
}

#[cfg(test)]
#[test]
fn configures_each_root_without_enabling_bus_mastering() {
    extern crate std;
    use std::collections::BTreeMap;
    struct RegisterModel(BTreeMap<u64, u32>);
    impl ConfigIo for RegisterModel {
        type Error = ();
        fn read32(&mut self, p: u64) -> Result<u32, ()> {
            Ok(*self.0.get(&p).unwrap_or(&0))
        }
        fn write32(&mut self, p: u64, value: u32) -> Result<(), ()> {
            self.0.insert(p, value);
            Ok(())
        }
        fn delay_us(&mut self, _: u32) {}
    }
    for (apb, dbi, start, speed) in [
        (0xfe260000, 0x3c0000000, 0, 2),
        (0xfe270000, 0x3c0400000, 16, 3),
    ] {
        let mut io = RegisterModel(BTreeMap::new());
        io.0.insert(dbi + 0x18, 0x42000000);
        io.0.insert(dbi + 0x04, 7);
        assert_eq!(configure_root(&mut io, apb, dbi, (start, start + 15), speed), Ok(()));
        assert_eq!(io.0[&apb], 0x00f00040);
        assert_eq!(io.0[&(dbi + 0x18)], 0x420f0100);
        assert_eq!(io.0[&(dbi + 0x04)], 0);
        assert_eq!(io.0[&(dbi + 0x08)], 0x06040000);
        assert_eq!(io.0[&(dbi + 0x8bc)], 0);
        assert_eq!(io.0[&(dbi + 0xa0)], u32::from(speed));
    }
}

pub struct DirectEndpoint<'a, I> {
    pub io: &'a mut I,
    pub dbi: u64,
    pub config_aperture: (u64, u64),
    pub buses: (u8, u8),
}

impl<I: ConfigIo> DirectEndpoint<'_, I> {
    fn location(&self) -> Result<(u8, u8, u8), ProbeError<I::Error>> {
        Ok((self.buses.0.checked_add(1).ok_or(ProbeError::InvalidLayout)?, 0, 0))
    }
}

impl<I: ConfigIo> EndpointConfig for DirectEndpoint<'_, I> {
    type Error = ProbeError<I::Error>;
    fn read_dword(&mut self, offset: u16) -> Result<u32, Self::Error> {
        let location = self.location()?;
        read_config(self.io, self.dbi, self.config_aperture, self.buses, location, offset)
    }
    fn write_dword(&mut self, offset: u16, value: u32) -> Result<(), Self::Error> {
        let location = self.location()?;
        config_access(self.io, self.dbi, self.config_aperture, self.buses, location, offset,
                      |io, address| io.write32(address, value))
    }
    fn disable_decode(&mut self) -> Result<(), Self::Error> {
        let command = self.read_dword(4)? & 0xfff8;
        self.write_dword(4, command)?;
        if self.read_dword(4)? & 7 != 0 {
            return Err(ProbeError::InvalidLayout);
        }
        Ok(())
    }
}

/// Install one memory aperture in outbound region 0. It must be reserved for
/// this root port and inaccessible to other owners. No PCI command bits change.
pub fn map_memory<I: ConfigIo>(
    io: &mut I, dbi: u64, cpu: u64, bus: u64, size: u64,
) -> Result<(), ProbeError<I::Error>> {
    if size < 4096 || (cpu | bus | size) & 4095 != 0 || bus.checked_add(size - 1).is_none() {
        return Err(ProbeError::InvalidLayout);
    }
    let limit = cpu.checked_add(size - 1).ok_or(ProbeError::InvalidLayout)?;
    let atu = dbi.checked_add(0x300000).ok_or(ProbeError::InvalidLayout)?;
    atu.checked_add(0x20).ok_or(ProbeError::InvalidLayout)?;
    set_enabled(io, atu + 4, false)?;
    for (offset, value) in [
        (0x08, cpu as u32), (0x0c, (cpu >> 32) as u32),
        (0x10, limit as u32), (0x20, (limit >> 32) as u32),
        (0x14, bus as u32), (0x18, (bus >> 32) as u32), (0, 0),
    ] {
        io.write32(atu + offset, value).map_err(ProbeError::Io)?;
        if io.read32(atu + offset).map_err(ProbeError::Io)? != value {
            return Err(ProbeError::InvalidLayout);
        }
    }
    let enabled = set_enabled(io, atu + 4, true);
    if enabled.is_err() {
        set_enabled(io, atu + 4, false)?;
    }
    enabled
}

/// Program a below-4-GiB address-match inbound memory region. The caller
/// owns region zero and must hold endpoint bus mastering off throughout.
pub fn map_inbound<I: ConfigIo>(
    io: &mut I, dbi: u64, window: &crate::dma::InboundWindow,
) -> Result<u32, ProbeError<I::Error>> {
    let size = window.size;
    io.inbound_step(1, 1, dbi, 0x1000, size as u32);
    if size < 4096 || (window.physical | window.bus | size) & 4095 != 0 {
        return Err(ProbeError::InvalidLayout);
    }
    let bus_end = window.bus.checked_add(size - 1)
        .filter(|&end| end <= u64::from(u32::MAX)).ok_or(ProbeError::InvalidLayout)?;
    window.physical.checked_add(size - 1)
        .filter(|&end| end <= u64::from(u32::MAX)).ok_or(ProbeError::InvalidLayout)?;
    let viewport = dbi.checked_add(0x900).ok_or(ProbeError::InvalidLayout)?;
    let region = dbi.checked_add(0x300100).ok_or(ProbeError::InvalidLayout)?;
    region.checked_add(0x20).ok_or(ProbeError::InvalidLayout)?;
    macro_rules! read {
        ($op:expr, $addr:expr, $expected:expr) => {{
            let address = $addr;
            io.inbound_step($op, 2, address, $expected, u32::MAX);
            io.read32(address).map_err(ProbeError::Io)?
        }};
    }
    macro_rules! write {
        ($op:expr, $addr:expr, $value:expr) => {{
            let address = $addr;
            let value = $value;
            io.inbound_step($op, 3, address, value, u32::MAX);
            io.write32(address, value).map_err(ProbeError::Io)?;
        }};
    }
    let mode = read!(2, viewport, u32::MAX);
    io.inbound_step(2, 4, viewport, u32::MAX, mode);
    if mode != u32::MAX {
        return Err(ProbeError::InboundMode);
    }

    let control = region + 4;
    let enabled = 1 << 31;
    {
        let (operation, expected) = (3, 0);
        write!(operation, control, expected);
        let mut ready = false;
        for _ in 0..5 {
            let actual = read!(operation, control, expected);
            io.inbound_step(operation, 5, control, expected, actual);
            if actual & enabled == expected {
                ready = true;
                break;
            }
            io.delay_us(10_000);
        }
        if !ready { return Err(ProbeError::WindowTimeout); }
    }

    let limit_register = region + 0x10;
    write!(4, limit_register, 0);
    let fixed = read!(4, limit_register, 0);
    // DesignWare reports alignment as 1 << fls(LIMIT after writing zero).
    let alignment = 1u64 << (u32::BITS - fixed.leading_zeros());
    let low_mask = (alignment - 1) as u32;
    io.inbound_step(4, 6, limit_register, alignment as u32, fixed);
    if (window.bus | window.physical | size) & (alignment - 1) != 0 {
        return Err(ProbeError::InboundAlignment);
    }
    write!(5, limit_register, u32::MAX);
    let low = read!(5, limit_register, u32::MAX);
    io.inbound_step(5, 7, limit_register, !low_mask, low);
    if low & !low_mask != !low_mask || low & fixed != fixed {
        return Err(ProbeError::InboundWidth);
    }
    // The upper limit remains zero for an aperture wholly below 4 GiB.
    let limit_readback = bus_end as u32 & low;
    for (offset, value, expected) in [
        (0x08, window.bus as u32, window.bus as u32), (0x0c, 0, 0),
        (0x10, bus_end as u32, limit_readback), (0x20, 0, 0),
        (0x14, window.physical as u32, window.physical as u32), (0x18, 0, 0), (0, 0, 0),
    ] {
        let address = region + offset;
        write!(6, address, value);
        let actual = read!(6, address, expected);
        io.inbound_step(6, 8, address, expected, actual);
        if actual != expected {
            return Err(ProbeError::InvalidLayout);
        }
    }

    let result = (|| {
        write!(7, control, enabled);
        for _ in 0..5 {
            let actual = read!(7, control, enabled);
            io.inbound_step(7, 5, control, enabled, actual);
            if actual & enabled != 0 {
                io.inbound_step(7, 8, control, enabled, actual);
                return if actual == enabled { Ok(limit_readback) } else { Err(ProbeError::InvalidLayout) };
            }
            io.delay_us(10_000);
        }
        Err(ProbeError::WindowTimeout)
    })();
    if result.is_err() {
        let _ = set_enabled(io, control, false);
    }
    result
}

pub fn set_endpoint_command<I: ConfigIo>(
    io: &mut I, dbi: u64, aperture: (u64, u64), buses: (u8, u8), command: u32,
) -> Result<(), ProbeError<I::Error>> {
    if command & !7 != 0 {
        return Err(ProbeError::InvalidLayout);
    }
    let mut endpoint = DirectEndpoint {
        io, dbi, config_aperture: aperture, buses,
    };
    endpoint.write_dword(4, command)?;
    if endpoint.read_dword(4)? & 7 != command {
        return Err(ProbeError::InvalidLayout);
    }
    Ok(())
}

/// Assign a sized BAR with decoding disabled. A failed write/readback leaves
/// decoding disabled, so the caller can stop the port without using the BAR.
pub fn program_bar<I: EndpointConfig>(
    io: &mut I, slot: u8, bus: u64, size: u64, wide: bool,
) -> Result<(), BarError<I::Error>> {
    if slot >= 6 || (wide && slot == 5) {
        return Err(BarError::InvalidSlot);
    }
    if size < 16 || !size.is_power_of_two() || bus & (size - 1) != 0
        || bus.checked_add(size - 1).is_none_or(|end| !wide && end > u64::from(u32::MAX))
    {
        return Err(BarError::InvalidBar);
    }
    io.disable_decode().map_err(BarError::Io)?;
    let offset = 0x10 + u16::from(slot) * 4;
    let original = io.read_dword(offset).map_err(BarError::Io)?;
    if original & 7 != if wide { 4 } else { 0 } {
        return Err(BarError::InvalidBar);
    }
    let low = bus as u32 | (original & 0xf);
    io.write_dword(offset, low).map_err(BarError::Io)?;
    if wide {
        io.write_dword(offset + 4, (bus >> 32) as u32).map_err(BarError::Io)?;
    }
    if io.read_dword(offset).map_err(BarError::Io)? != low
        || (wide && io.read_dword(offset + 4).map_err(BarError::Io)? != (bus >> 32) as u32)
    {
        return Err(BarError::InvalidBar);
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn inbound_matches_pci_address_and_targets_cpu_pool() {
    extern crate std;
    use std::collections::BTreeMap;
    #[derive(Default)]
    struct Model {
        regs: BTreeMap<u64, u32>,
        reject_target: bool,
        limit_mask: u32,
        limit_read_zero: u32,
        stuck_enable: bool,
        fail_read: Option<u64>,
        step: (u32, u32, u64, u32, u32),
    }
    impl ConfigIo for Model {
        type Error = ();
        fn read32(&mut self, address: u64) -> Result<u32, ()> {
            if Some(address) == self.fail_read { return Err(()); }
            if address & 0xfff == 0x900 {
                return Ok(u32::MAX);
            }
            Ok(self.regs.get(&address).copied().unwrap_or(0))
        }
        fn write32(&mut self, address: u64, value: u32) -> Result<(), ()> {
            if self.stuck_enable && address & 0xfff == 0x104 && value != 0 { return Ok(()); }
            if !(self.reject_target && address & 0xfff == 0x114) {
                self.regs.insert(address, if address & 0xfff == 0x110 { (value | self.limit_mask) & !self.limit_read_zero } else { value });
            }
            Ok(())
        }
        fn delay_us(&mut self, _: u32) {}
        fn inbound_step(&mut self, op: u32, category: u32, address: u64, expected: u32, actual: u32) {
            self.step = (op, category, address, expected, actual);
        }
    }
    let window = crate::dma::InboundWindow { physical: 0x24200000, bus: 0x80000000, size: 0x100000 };
    for dbi in [0x3c0000000, 0x3c0400000] {
        let mut io = Model::default();
        assert_eq!(map_inbound(&mut io, dbi, &window), Ok(0x800fffff));
        assert_eq!(io.regs[&(dbi + 0x300108)], 0x80000000);
        assert_eq!(io.regs[&(dbi + 0x300110)], 0x800fffff);
        assert_eq!(io.regs[&(dbi + 0x300114)], 0x24200000);
        assert_eq!(io.regs[&(dbi + 0x300104)], 1 << 31);
    }
    let dbi = 0x3c0000000;
    let control = dbi + 0x300104;
    let mut rk3568 = Model { limit_mask: 0xfff0, limit_read_zero: 0xf, ..Model::default() };
    assert_eq!(map_inbound(&mut rk3568, dbi, &window), Ok(0x800ffff0));
    assert_eq!(rk3568.regs[&(dbi + 0x300110)], 0x800ffff0);
    assert!(!rk3568.regs.contains_key(&(dbi + 0x300010)));
    let mut stuck = Model { stuck_enable: true, ..Model::default() };
    assert_eq!(map_inbound(&mut stuck, dbi, &window), Err(ProbeError::WindowTimeout));
    assert_eq!(stuck.step, (7, 5, control, 1 << 31, 0));
    assert_eq!(stuck.regs[&control], 0);
    let mut inaccessible = Model { fail_read: Some(dbi + 0x300110), ..Model::default() };
    assert_eq!(map_inbound(&mut inaccessible, dbi, &window), Err(ProbeError::Io(())));
    assert_eq!(inaccessible.step, (4, 2, dbi + 0x300110, 0, u32::MAX));
    let mut io = Model { limit_mask: 0x1fffff, ..Model::default() };
    assert_eq!(map_inbound(&mut io, 0x3c0000000, &window), Err(ProbeError::InboundAlignment));
    assert_eq!(io.regs[&0x3c0300104], 0);
    let mut io = Model { reject_target: true, ..Model::default() };
    assert_eq!(map_inbound(&mut io, 0x3c0000000, &window), Err(ProbeError::InvalidLayout));
    assert_eq!(io.regs[&0x3c0300104], 0);
    let invalid = crate::dma::InboundWindow { bus: 0xfffff000, ..window };
    let mut io = Model::default();
    assert_eq!(map_inbound(&mut io, 0x3c0000000, &invalid), Err(ProbeError::InvalidLayout));
    assert!(io.regs.is_empty());
}
