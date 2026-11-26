//! RTL8125A/B quiescence based on the pinned r8169 DPDK reset path.
//! Full method-specific OOB, EPHY, MAC and PHY MCU setup is separate.

use crate::{mmio::{AccessError, DeviceIo}, pcie::ConfigIo, ring::{method, Method}};

pub trait Registers: ConfigIo {
    fn read8(&mut self, address: u64) -> Result<u8, Self::Error>;
    fn read16(&mut self, address: u64) -> Result<u16, Self::Error>;
    fn write8(&mut self, address: u64, value: u8) -> Result<(), Self::Error>;
    fn write16(&mut self, address: u64, value: u16) -> Result<(), Self::Error>;
}

impl<I: Registers> Registers for &mut I {
    fn read8(&mut self, address: u64) -> Result<u8, Self::Error> { (**self).read8(address) }
    fn read16(&mut self, address: u64) -> Result<u16, Self::Error> { (**self).read16(address) }
    fn write8(&mut self, address: u64, value: u8) -> Result<(), Self::Error> { (**self).write8(address, value) }
    fn write16(&mut self, address: u64, value: u16) -> Result<(), Self::Error> { (**self).write16(address, value) }
}

impl<F: FnMut(u32)> Registers for DeviceIo<'_, F> {
    fn read8(&mut self, address: u64) -> Result<u8, AccessError> {
        DeviceIo::read8(self, address)
    }
    fn read16(&mut self, address: u64) -> Result<u16, AccessError> {
        DeviceIo::read16(self, address)
    }
    fn write8(&mut self, address: u64, value: u8) -> Result<(), AccessError> {
        DeviceIo::write8(self, address, value)
    }
    fn write16(&mut self, address: u64, value: u16) -> Result<(), AccessError> {
        DeviceIo::write16(self, address, value)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResetError<E> {
    Io(E),
    Layout,
    Unsupported(u32),
    StopRequest,
    FifoTimeout,
    MitigationTimeout,
    ResetTimeout,
}

const CHIP_CMD: u64 = 0x37;
const IRQ_MASK: u64 = 0x38;
const RX_CONFIG: u64 = 0x44;
const MCU: u64 = 0xd3;
const MITIGATE: u64 = 0xe2;
const RX_GATE: u64 = 0xf2;
const STOP: u8 = 0x80;
const RESET: u8 = 0x10;

/// Caller verifies endpoint ownership and disables PCI bus mastering before
/// calling this quiescence path. On failure the NIC and pool must stay isolated.
pub fn reset<I: Registers>(
    io: &mut I, bar: u64, pci_id: u32,
) -> Result<Method, ResetError<I::Error>> {
    if bar.checked_add(RX_GATE).is_none() || bar & 3 != 0 {
        return Err(ResetError::Layout);
    }
    let config = io.read32(bar + 0x40).map_err(ResetError::Io)?;
    let revision = method(pci_id, config).ok_or(ResetError::Unsupported(config))?;
    io.write32(bar + IRQ_MASK, 0).map_err(ResetError::Io)?;
    let receive = io.read32(bar + RX_CONFIG).map_err(ResetError::Io)?;
    io.write32(bar + RX_CONFIG, receive & !0x3f).map_err(ResetError::Io)?;
    let gate = io.read8(bar + RX_GATE).map_err(ResetError::Io)?;
    io.write8(bar + RX_GATE, gate | 8).map_err(ResetError::Io)?;
    let command = io.read8(bar + CHIP_CMD).map_err(ResetError::Io)?;
    io.write8(bar + CHIP_CMD, command | STOP).map_err(ResetError::Io)?;
    let b = matches!(revision, Method::B50 | Method::B51);
    if b {
        io.delay_us(200);
    } else if !poll(io, 20, 10, |io| Ok(io.read8(bar + CHIP_CMD)? & STOP == 0))
        .map_err(ResetError::Io)?
    {
        return Err(ResetError::StopRequest);
    }
    if !poll(io, 3000, 50, |io| Ok(io.read8(bar + MCU)? & 0x30 == 0x30))
        .map_err(ResetError::Io)?
    {
        return Err(ResetError::FifoTimeout);
    }
    if b && !poll(io, 3000, 50, |io| Ok(io.read16(bar + MITIGATE)? & 0x103 == 0x103))
        .map_err(ResetError::Io)?
    {
        return Err(ResetError::MitigationTimeout);
    }

    let command = io.read8(bar + CHIP_CMD).map_err(ResetError::Io)?;
    io.write8(bar + CHIP_CMD, command & 0xc).map_err(ResetError::Io)?;
    io.write8(bar + CHIP_CMD, RESET).map_err(ResetError::Io)?;
    if !poll(io, 100, 100, |io| Ok(io.read8(bar + CHIP_CMD)? & (RESET | 0xc) == 0))
        .map_err(ResetError::Io)?
    {
        return Err(ResetError::ResetTimeout);
    }
    Ok(revision)
}

fn poll<I: Registers>(
    io: &mut I, attempts: usize, wait: u32, mut ready: impl FnMut(&mut I) -> Result<bool, I::Error>,
) -> Result<bool, I::Error> {
    for _ in 0..attempts {
        io.delay_us(wait);
        if ready(io)? { return Ok(true); }
    }
    Ok(false)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[derive(Default)]
    pub struct Model {
        pub signature: u32,
        pub doorbells: usize,
        pub tx_tail: u16,
        pub tx_close: u16,
        pub tx_tail_q1: u16,
        pub tx_close_q1: u16,
        pub writes: usize,
        pub mitigation_reads: usize,
        pub fifo_busy: bool,
        pub reset_stuck: bool,
        pub command: u8,
    }
    impl ConfigIo for Model {
        type Error = ();
        fn read32(&mut self, address: u64) -> Result<u32, ()> {
            Ok(match address {
                0x40 => self.signature,
                RX_CONFIG => 0xabc3f,
                _ => panic!("unexpected register"),
            })
        }
        fn write32(&mut self, address: u64, v: u32) -> Result<(), ()> {
            assert!(matches!((address,v), (IRQ_MASK, 0) | (RX_CONFIG, 0xabc00)));
            self.writes += 1;
            Ok(())
        }
        fn delay_us(&mut self, _: u32) {
            self.command &= !STOP;
            if !self.reset_stuck { self.command &= !RESET; }
        }
    }
    impl Registers for Model {
        fn read8(&mut self, address: u64) -> Result<u8, ()> {
            Ok(match address {
                CHIP_CMD => self.command,
                RX_GATE => 0,
                MCU => if self.fifo_busy { 0 } else { 0x30 },
                _ => panic!("unexpected register"),
            })
        }
        fn read16(&mut self, address: u64) -> Result<u16, ()> {
            match address {
                crate::dma::TX_TAIL => return Ok(self.tx_tail),
                crate::dma::TX_CLOSE => return Ok(self.tx_close),
                0x2804 => return Ok(self.tx_tail_q1),
                0x2806 => return Ok(self.tx_close_q1),
                _ => assert_eq!(address, MITIGATE),
            }
            self.mitigation_reads += 1;
            Ok(0x103)
        }
        fn write8(&mut self, address: u64, v: u8) -> Result<(), ()> {
            self.writes += 1;
            match address {
                CHIP_CMD => self.command = v,
                RX_GATE => assert_eq!(v, 8),
                _ => panic!("unexpected register"),
            }
            Ok(())
        }
        fn write16(&mut self, address: u64, value: u16) -> Result<(), ()> {
            if address == crate::dma::TX_TAIL {
                self.tx_tail = value;
                return Ok(());
            }
            if address == 0x2804 {
                self.tx_tail_q1 = value;
                return Ok(());
            }
            assert_eq!((address,value), (0x90,1));
            self.doorbells += 1;
            Ok(())
        }
    }

    #[test]
    fn reset_distinguishes_a_b_and_rejects_unknown_before_writes() {
        for (signature, expected, polls) in [
            (0x60900000, Method::A49, 0),
            (0x64100000, Method::B51, 1),
        ] {
            let mut io = Model { signature, ..Model::default() };
            assert_eq!(reset(&mut io, 0, 0x8125_10ec), Ok(expected));
            assert_eq!(io.command, 0);
            assert_eq!(io.mitigation_reads, polls);
            io.reset_stuck = true;
            assert_eq!(reset(&mut io, 0, 0x8125_10ec), Err(ResetError::ResetTimeout));
        }
        let mut io = Model { signature: 0x64200000, ..Model::default() };
        assert_eq!(reset(&mut io, 0, 0x8125_10ec), Err(ResetError::Unsupported(0x64200000)));
        assert_eq!(io.writes, 0);
    }

    #[test]
    fn a_fifo_timeout_never_authorizes_reinitialization() {
        let mut io = Model { signature: 0x64100000, fifo_busy: true, ..Model::default() };
        assert_eq!(reset(&mut io, 0, 0x8125_10ec), Err(ResetError::FifoTimeout));
        assert_eq!(io.mitigation_reads, 0);
        assert_ne!(io.writes, 0);
        assert_eq!(io.command & RESET, 0);
    }
}
