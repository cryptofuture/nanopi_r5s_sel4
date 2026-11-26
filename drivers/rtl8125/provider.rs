//! RK3568 reset and reference-clock controls for the captured R5S PCIe topology.
use super::pcie::ConfigIo;

const CRU: u64 = 0xfdd20000;
const PMUCRU: u64 = 0xfdd00000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetLine {
    Lan1Pipe,
    Lan2Pipe,
    Combo2Apb,
    Combo2Phy,
    Pcie3Phy,
}

pub fn set_reset<I: ConfigIo>(
    io: &mut I, line: ResetLine, asserted: bool,
) -> Result<bool, I::Error> {
    // Reset IDs are from the captured DT. The RK3568 CRU binds the
    // Rockchip reset controller at 0x400 with 16 reset lines per register.
    let id = match line {
        ResetLine::Lan1Pipe => 0xa1,
        ResetLine::Lan2Pipe => 0xb1,
        ResetLine::Combo2Apb => 0x1c8,
        ResetLine::Combo2Phy => 0x1c9,
        ResetLine::Pcie3Phy => 0x1be,
    };
    let address = CRU + 0x400 + (id / 16) * 4;
    let mask: u32 = 1 << (id % 16);
    io.write32(address, (mask << 16) | if asserted { mask } else { 0 })?;
    Ok((io.read32(address)? & mask != 0) == asserted)
}

/// U-Boot's rk3568_pmuclk init ungates the two shared reference clocks in
/// PMU_CLKGATE_CON2. The upstream PLL and mux must already be configured.
pub fn enable_gen3_reference_gates<I: ConfigIo>(io: &mut I) -> Result<bool, I::Error> {
    let register = PMUCRU + 0x188;
    let mask = 3 << 13;
    io.write32(register, mask << 16)?;
    Ok(io.read32(register)? & mask == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Model {
        address: u64,
        value: u32,
        stuck: bool,
    }
    impl ConfigIo for Model {
        type Error = ();
        fn read32(&mut self, address: u64) -> Result<u32, ()> {
            assert_eq!(self.address, address);
            Ok(self.value)
        }
        fn write32(&mut self, address: u64, word: u32) -> Result<(), ()> {
            assert_eq!(self.address, address);
            let mask = word >> 16;
            assert_eq!(word as u16 as u32 & !mask, 0);
            if !self.stuck {
                self.value = (self.value & !mask) | (word & mask);
            }
            Ok(())
        }
        fn delay_us(&mut self, _: u32) {}
    }
    #[test]
    fn verified_reset_mask_does_not_touch_adjacent_lines() {
        for (line, offset, bit) in [
            (ResetLine::Lan1Pipe, 0x428, 1),
            (ResetLine::Lan2Pipe, 0x42c, 1),
            (ResetLine::Combo2Apb, 0x470, 8),
            (ResetLine::Combo2Phy, 0x470, 9),
            (ResetLine::Pcie3Phy, 0x46c, 14),
        ] {
            let mask = 1 << bit;
            let mut io = Model { address: CRU + offset, value: 0xffff ^ mask, stuck: false };
            assert_eq!(set_reset(&mut io, line, true), Ok(true));
            assert_eq!(io.value, 0xffff);
            assert_eq!(set_reset(&mut io, line, false), Ok(true));
            assert_eq!(io.value, 0xffff ^ mask);
            io.stuck = true;
            assert_eq!(set_reset(&mut io, line, true), Ok(false));
        }
    }
    #[test]
    fn reference_clock_write_is_masked_and_verified() {
        let mut io = Model { address: PMUCRU + 0x188, value: 0xffff, stuck: true };
        assert_eq!(enable_gen3_reference_gates(&mut io), Ok(false));
        io.stuck = false;
        assert_eq!(enable_gen3_reference_gates(&mut io), Ok(true));
        assert_eq!(io.value, 0x9fff);
    }
}

const PMU: u64 = 0xfdd90000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderError<E> {
    Io(E),
    ReferenceClock,
    GateReadback,
    PipePowerTimeout,
    PipeIdleTimeout,
}

fn update<I: ConfigIo>(
    io: &mut I, address: u64, mask: u32, value: u32,
) -> Result<(), ProviderError<I::Error>> {
    io.write32(address, (mask << 16) | value).map_err(ProviderError::Io)?;
    if io.read32(address).map_err(ProviderError::Io)? & mask != value {
        return Err(ProviderError::GateReadback);
    }
    Ok(())
}

fn wait_clear<I: ConfigIo>(
    io: &mut I, address: u64, mask: u32,
) -> Result<bool, ProviderError<I::Error>> {
    for _ in 0..1000 {
        if io.read32(address).map_err(ProviderError::Io)? & mask == 0 {
            return Ok(true);
        }
        io.delay_us(10);
    }
    Ok(false)
}

/// Shared provider startup, before either PCIe controller is used. The caller
/// owns the PIPE domain and keeps both endpoints in reset. There is no shared
/// domain shutdown in a per-port recovery path.
///
/// Source: Linux v6.1 clk-rk3568.c PD_PIPE gates, PMU CLKSEL_CON9, and
/// pm_domains.c RK3568_PD_PIPE. PLL layout and 200 MHz policy from pinned U-Boot.
pub fn prepare_shared<I: ConfigIo>(io: &mut I) -> Result<(), ProviderError<I::Error>> {
    let mode = io.read32(PMUCRU + 0x80).map_err(ProviderError::Io)? & 3;
    let con0 = io.read32(PMUCRU).map_err(ProviderError::Io)?;
    let con1 = io.read32(PMUCRU + 4).map_err(ProviderError::Io)?;
    let divisors = (con1 & 63) * ((con0 >> 12) & 7) * ((con1 >> 6) & 7);
    if mode != 1 || con1 & ((1 << 13) | (1 << 12) | (1 << 10)) != (1 << 12) | (1 << 10)
        || divisors == 0 || 24 * (con0 & 0xfff) != 200 * divisors
    {
        return Err(ProviderError::ReferenceClock);
    }

    // PIPE ACLK and PCLK, then the PHY's APB parent and leaves.
    update(io, CRU + 0x328, 3, 0)?;
    update(io, CRU + 0x384, (1 << 3) | (1 << 8), 0)?;
    update(io, CRU + 0x388, 1 << 6, 0)?;

    // Combo PHY2: PPLL phase zero (100 MHz), divider one. Enable the
    // divided reference and both PCIe3 phase gates.
    update(io, PMUCRU + 0x124, 0xf00, 0x800)?;
    update(io, PMUCRU + 0x188, (1 << 11) | (3 << 13), 0)?;

    io.write32(PMU + 0xa0, 1 << 24).map_err(ProviderError::Io)?;
    if !wait_clear(io, PMU + 0x98, 1 << 8)? {
        return Err(ProviderError::PipePowerTimeout);
    }
    io.write32(PMU + 0x50, 1 << 27).map_err(ProviderError::Io)?;
    if !wait_clear(io, PMU + 0x60, 1 << 11)?
        || !wait_clear(io, PMU + 0x68, 1 << 11)?
    {
        return Err(ProviderError::PipeIdleTimeout);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lan {
    One,
    Two,
}

pub fn enable_root_clocks<I: ConfigIo>(io: &mut I, lan: Lan) -> Result<(), ProviderError<I::Error>> {
    let mask = match lan {
        Lan::One => 0x001f,
        Lan::Two => 0x1f00,
    };
    update(io, CRU + 0x330, mask, 0)
}

#[cfg(test)]
#[test]
fn power_up_shared_domain_only_after_clock_verification() {
    extern crate std;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct State {
        regs: BTreeMap<u64, u32>,
        writes: usize,
        delays: usize,
        blocked: bool,
    }
    impl ConfigIo for State {
        type Error = ();
        fn read32(&mut self, p: u64) -> Result<u32, ()> {
            if self.blocked && p == PMU + 0x98 {
                return Ok(1 << 8);
            }
            Ok(*self.regs.get(&p).unwrap_or(&0))
        }
        fn write32(&mut self, p: u64, word: u32) -> Result<(), ()> {
            self.writes += 1;
            let mask = word >> 16;
            let old = *self.regs.get(&p).unwrap_or(&0);
            if p == PMU + 0xa0 {
                assert_eq!(word, 1 << 24);
                assert_eq!(self.regs[&(PMUCRU + 0x124)] & 0xf00, 0x800);
            }
            if p == PMU + 0x50 {
                assert_eq!(word, 1 << 27);
            }
            self.regs.insert(p, (old & !mask) | (word & mask));
            Ok(())
        }
        fn delay_us(&mut self, _: u32) { self.delays += 1; }
    }

    let mut state = State::default();
    assert_eq!(prepare_shared(&mut state), Err(ProviderError::ReferenceClock));
    assert_eq!(state.writes, 0);
    state.regs.insert(PMUCRU + 0x80, 1);
    // 24 * 100 / (1 * 6 * 2) = 200 MHz in integer mode.
    state.regs.insert(PMUCRU, 100 | (6 << 12));
    state.regs.insert(PMUCRU + 4, 1 | (2 << 6) | (1 << 12) | (1 << 10));
    state.regs.insert(CRU + 0x330, 0xffff);
    state.blocked = true;
    assert_eq!(prepare_shared(&mut state), Err(ProviderError::PipePowerTimeout));
    assert_eq!(state.delays, 1000);
    assert!(!state.regs.contains_key(&(PMU + 0x50)));
    state.blocked = false;
    assert_eq!(prepare_shared(&mut state), Ok(()));
    enable_root_clocks(&mut state, Lan::One).unwrap();
    assert_eq!(state.regs[&(CRU + 0x330)], 0xffe0);
    enable_root_clocks(&mut state, Lan::Two).unwrap();
    assert_eq!(state.regs[&(CRU + 0x330)], 0xe0e0);
}
