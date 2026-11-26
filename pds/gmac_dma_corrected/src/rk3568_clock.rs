const OSCILLATOR: u64 = 24_000_000;

fn pll(read: &impl Fn(u32) -> u32, mode: u32, base: u32) -> Option<u32> {
    match mode & 3 {
        0 => return Some(OSCILLATOR as u32),
        1 => {}
        _ => return None,
    }
    let c0 = read(base);
    let c1 = read(base + 4);
    if c1 & (1 << 13) != 0 || c1 & (1 << 10) == 0 { return None; }
    let divisor = u64::from(c0 >> 12 & 7) * u64::from(c1 & 63) * u64::from(c1 >> 6 & 7);
    let fraction = if c1 & (1 << 12) == 0 { read(base + 8) & 0xffffff } else { 0 };
    let feedback = (u64::from(c0 & 0xfff) << 24) | u64::from(fraction);
    let rate = (OSCILLATOR * feedback).checked_div(divisor << 24)?;
    u32::try_from(rate).ok().filter(|rate| *rate != 0)
}

fn decode(read: impl Fn(u32) -> u32) -> [u32; 6] {
    let core01 = read(0x100);
    let core23 = read(0x104);
    let mode = read(0xc0);
    let apll = || pll(&read, mode, 0);
    let gpll = || pll(&read, mode >> 6, 0x40);
    let cpu = if core01 & (1 << 15) != 0 {
        None
    } else if core01 & (1 << 7) != 0 || core01 & (1 << 6) == 0 {
        apll()
    } else {
        gpll()
    };
    let dividers = [core01, core01 >> 8, core23, core23 >> 8];
    let ddr = read(0x124);
    let ddr_src = match (ddr >> 6) & 3 {
        0 => pll(&read, mode >> 2, 0x20),
        1 => gpll(),
        2 => pll(&read, mode >> 4, 0x60),
        _ => None,
    };
    let mut result = [0; 6];
    if let Some(cpu) = cpu {
        for (out, divider) in result.iter_mut().zip(dividers) {
            *out = cpu / ((divider & 31) + 1);
        }
    }
    let dedicated = ddr & (1 << 15) != 0;
    let gate = if dedicated { 2 } else { 1 };
    let enabled = read(0x310) & gate == 0;
    if !dedicated && enabled {
        result[4] = ddr_src.unwrap_or(0) / ((ddr & 31) + 1);
    }
    let source = if dedicated { 4 } else { (ddr >> 6) & 3 };
    result[5] = 1 | (source << 1) | (u32::from(enabled) << 4);
    result
}

pub fn snapshot(read: impl Fn(u32) -> u32) -> [u32; 6] {
    const REGISTERS: [u32; 17] = [0, 4, 8, 0x20, 0x24, 0x28, 0x40, 0x44, 0x48,
        0x60, 0x64, 0x68, 0xc0, 0x100, 0x104, 0x124, 0x310];
    let sample = REGISTERS.map(&read);
    if REGISTERS.map(&read) != sample { return [0; 6]; }
    decode(|offset| sample[REGISTERS.iter().position(|&r| r == offset).unwrap()])
}

#[cfg(test)]
#[test]
fn arm_and_divided_ddr_rates_are_decoded_from_locked_pll() {
    let read = |register| match register {
        0xc0 => 1 | (1 << 2),
        0 => (1 << 12) | 68,
        0x20 => (1 << 12) | 65,
        4 | 0x24 => 2 | (1 << 6) | (1 << 10) | (1 << 12),
        0x100 => 1 << 8,
        0x104 => (1 << 8) | 3,
        0x124 => 1,
        _ => 0,
    };
    assert_eq!(snapshot(read), [816_000_000, 408_000_000, 204_000_000, 408_000_000, 390_000_000, 17]);
    assert_eq!(snapshot(|reg| if reg == 0x100 {1 << 15} else {read(reg)})[..4], [0; 4]);
    assert_eq!(snapshot(|reg| if reg == 4 {0} else {read(reg)})[..4], [0; 4]);
    assert_eq!(snapshot(|reg| if reg == 0x310 {1} else {read(reg)})[4..], [0, 1]);
    assert_eq!(snapshot(|reg| if reg == 0x124 {1 << 15} else {read(reg)})[4..], [0, 25]);
    let reads = core::cell::Cell::new(0);
    assert_eq!(snapshot(|reg| if reg == 0x124 {
        let count = reads.get();
        reads.set(count + 1);
        count
    } else {read(reg)}), [0; 6]);
}
