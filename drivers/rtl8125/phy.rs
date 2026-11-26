// SPDX-License-Identifier: GPL-2.0-only
//! RK3568-specific PHY sequences from the pinned U-Boot Rockchip PHY drivers.

use super::pcie::ConfigIo;

pub const COMBO2: u64 = 0xfe840000;
pub const COMBO2_GRF: u64 = 0xfdc90000;
pub const PCIE3_GRF: u64 = 0xfdcb8000;

/// Program LAN1's combo PHY after the reference clock and APB are available
/// and while the provider holds the PHY in reset. The caller selects the
/// verified reference rate and releases reset only after success.
pub fn configure_combo2<I: ConfigIo>(io: &mut I, reference_hz: u32) -> Result<bool, I::Error> {
    let clock_select = match reference_hz {
        25_000_000 => 1,
        100_000_000 => 2,
        _ => return Ok(false),
    };
    let ssc = io.read32(COMBO2 + 0x7c)?;
    io.write32(COMBO2 + 0x7c, (ssc & !(3 << 4)) | (1 << 4))?;
    for (offset, value) in [(0, 0x1000), (4, 0), (8, 0x0101), (12, 0x0200)] {
        io.write32(COMBO2_GRF + offset, 0xffff0000 | value)?;
    }
    io.write32(COMBO2_GRF + 4, (3 << 29) | (clock_select << 13))?;
    if reference_hz == 100_000_000 {
        // RK3568 PCIe needs the 100 MHz PLL tuning before combo PHY reset is released.
        let kvco = io.read32(COMBO2 + 0x80)?;
        io.write32(COMBO2 + 0x80, (kvco & !0x1c) | 0x08)?;
        io.write32(COMBO2 + 0x2c, 4)?;
        let divider = io.read32(COMBO2 + 0x14)?;
        io.write32(COMBO2 + 0x14, (divider & !0xc0) | 0x40)?;
        io.write32(COMBO2 + 0x44, 0x32)?;
        io.write32(COMBO2 + 0x28, 0xf0)?;
    }
    Ok(true)
}

pub trait Pcie3Io: ConfigIo {
    fn set_phy_reset(&mut self, asserted: bool) -> Result<(), Self::Error>;
}

/// Program the shared Gen3 PHY for the pinned R5S lane mapping <1 2>.
/// Both PHY reference clocks and its APB clock must already be enabled.
/// This initializes the shared PHY once, before training either of its ports.
pub fn start_pcie3<I: Pcie3Io>(io: &mut I) -> Result<bool, I::Error> {
    io.set_phy_reset(true)?;
    io.delay_us(1);
    let ready = (|| {
        io.write32(PCIE3_GRF + 0x24, 0x80008000)?;
        io.write32(PCIE3_GRF + 0x18, 0x000f0001)?;
        io.write32(PCIE3_GRF + 0x04, 0x80008000)?;
        io.set_phy_reset(false)?;
        io.delay_us(1);
        for _ in 0..500 {
            if io.read32(PCIE3_GRF + 0x80)? & (1 << 14) != 0 {
                return Ok(true);
            }
            io.delay_us(1);
        }
        Ok(false)
    })();
    if !matches!(ready, Ok(true)) {
        io.set_phy_reset(true)?;
    }
    ready
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use self::std::vec::Vec;

    #[derive(Default)]
    struct Model {
        writes: Vec<(u64, u32)>,
        reset: bool,
        lock: bool,
        reads: usize,
    }
    impl ConfigIo for Model {
        type Error = ();
        fn read32(&mut self, p: u64) -> Result<u32, ()> {
            if [0x7c, 0x80, 0x14].contains(&p.wrapping_sub(COMBO2)) {
                return Ok(0xffff);
            }
            assert_eq!(p, PCIE3_GRF + 0x80);
            assert!(!self.reset);
            self.reads += 1;
            Ok(if self.lock { 1 << 14 } else { 0 })
        }
        fn write32(&mut self, p: u64, value: u32) -> Result<(), ()> {
            self.writes.push((p,value));
            Ok(())
        }
        fn delay_us(&mut self, _: u32) {}
    }
    impl Pcie3Io for Model {
        fn set_phy_reset(&mut self, asserted: bool) -> Result<(), ()> {
            self.reset = asserted;
            Ok(())
        }
    }
    #[test]
    fn combo_uses_captured_100mhz_selector() {
        let mut io = Model::default();
        assert_eq!(configure_combo2(&mut io, 24_000_000), Ok(false));
        assert!(io.writes.is_empty());
        assert_eq!(configure_combo2(&mut io, 100_000_000), Ok(true));
        assert_eq!(io.writes.as_slice(), &[
            (COMBO2 + 0x7c, 0xffdf),
            (COMBO2_GRF, 0xffff1000),
            (COMBO2_GRF + 4, 0xffff0000),
            (COMBO2_GRF + 8, 0xffff0101),
            (COMBO2_GRF + 12, 0xffff0200),
            (COMBO2_GRF + 4, 0x60004000),
            (COMBO2 + 0x80, 0xffeb),
            (COMBO2 + 0x2c, 4),
            (COMBO2 + 0x14, 0xff7f),
            (COMBO2 + 0x44, 0x32),
            (COMBO2 + 0x28, 0xf0),
        ]);
    }
    #[test]
    fn combo_25mhz_uses_only_its_clock_selector_and_common_setup() {
        let mut io = Model::default();
        assert_eq!(configure_combo2(&mut io, 25_000_000), Ok(true));
        assert_eq!(io.writes.len(), 6);
        assert_eq!(io.writes.last(), Some(&(COMBO2_GRF + 4, 0x60002000)));
    }

    #[test]
    fn pcie3_bifurcation_and_bounded_failure() {
        for lock in [true, false] {
            let mut io = Model { lock, ..Model::default() };
            assert_eq!(start_pcie3(&mut io), Ok(lock));
            assert_eq!(io.reset, !lock);
            assert_eq!(io.reads, if lock { 1 } else { 500 });
            assert_eq!(io.writes.as_slice(), &[
                (PCIE3_GRF + 0x24, 0x80008000),
                (PCIE3_GRF + 0x18, 0x000f0001),
                (PCIE3_GRF + 0x04, 0x80008000),
            ]);
        }
    }
}
