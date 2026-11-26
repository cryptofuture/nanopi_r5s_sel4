//! R5S PCIe bring-up sequence over a validated native MMIO owner.

use super::{
    pcie::{self, ConfigIo, EndpointConfig, LinkIo},
    dma::InboundWindow,
    ring,
    phy::{self, Pcie3Io},
    provider::{self, Lan, ProviderError, ResetLine},
};

#[derive(Debug, PartialEq, Eq)]
pub enum BringupError<E> {
    Io(E),
    Provider(ProviderError<E>),
    ResetReadback,
    GpioReadback,
    PhyNotReady,
    Link(pcie::LinkError<E>),
    Probe(pcie::ProbeError<E>),
    Bar(pcie::BarError<pcie::ProbeError<E>>),
    UnsupportedMethod(ring::Identity),
}

const GPIO0: u64 = 0xfdd60000;
const PMUGRF: u64 = 0xfdc20000;
const PMUCRU: u64 = 0xfdd00000;

fn output_mask(lan: Lan) -> u32 {
    match lan {
        Lan::One => 1 << 14,
        Lan::Two => 1,
    }
}

fn perst<I: ConfigIo>(io: &mut I, lan: Lan, asserted: bool) -> Result<(), BringupError<I::Error>> {
    let bit = output_mask(lan);
    // The pinned R5S U-Boot drives the captured active-high GPIO descriptor
    // low while waiting on refclk, then drives it high to release PERST.
    let value = if asserted { 0 } else { bit };
    io.write32(GPIO0, bit << 16 | value).map_err(BringupError::Io)?;
    if io.read32(GPIO0).map_err(BringupError::Io)? & bit != value {
        return Err(BringupError::GpioReadback);
    }
    Ok(())
}

fn gpio_prepare<I: ConfigIo>(io: &mut I, lan: Lan) -> Result<(), BringupError<I::Error>> {
    io.write32(PMUCRU + 0x184, 1 << 25).map_err(BringupError::Io)?;
    let version = io.read32(GPIO0 + 0x78).map_err(BringupError::Io)?;
    if version == 0 || version == u32::MAX {
        return Err(BringupError::GpioReadback);
    }
    // Program the reset level before taking ownership of the pin.
    perst(io, lan, true)?;
    let bit = output_mask(lan);
    io.write32(GPIO0 + 8, bit << 16 | bit).map_err(BringupError::Io)?;
    let (iomux, mask) = match lan {
        Lan::One => (PMUGRF + 0xc, 0xf << 8), // GPIO0_B6
        Lan::Two => (PMUGRF, 0xf),             // GPIO0_A0
    };
    io.write32(iomux, mask << 16).map_err(BringupError::Io)?;
    if io.read32(GPIO0 + 8).map_err(BringupError::Io)? & bit != bit
        || io.read32(iomux).map_err(BringupError::Io)? & mask != 0
    {
        return Err(BringupError::GpioReadback);
    }
    Ok(())
}

fn reset<I: ConfigIo>(
    io: &mut I, line: ResetLine, asserted: bool,
) -> Result<(), BringupError<I::Error>> {
    match provider::set_reset(io, line, asserted).map_err(BringupError::Io)? {
        true => Ok(()),
        false => Err(BringupError::ResetReadback),
    }
}

struct PortIo<'a, I> {
    inner: &'a mut I,
    lan: Lan,
}

impl<I: ConfigIo> ConfigIo for PortIo<'_, I> {
    type Error = BringupError<I::Error>;
    fn read32(&mut self, address: u64) -> Result<u32, Self::Error> {
        self.inner.read32(address).map_err(BringupError::Io)
    }
    fn write32(&mut self, address: u64, value: u32) -> Result<(), Self::Error> {
        self.inner.write32(address, value).map_err(BringupError::Io)
    }
    fn delay_us(&mut self, us: u32) { self.inner.delay_us(us); }
}
impl<I: ConfigIo> LinkIo for PortIo<'_, I> {
    fn set_perst(&mut self, asserted: bool) -> Result<(), Self::Error> {
        perst(self.inner, self.lan, asserted)
    }
}
impl<I: ConfigIo> Pcie3Io for PortIo<'_, I> {
    fn set_phy_reset(&mut self, asserted: bool) -> Result<(), Self::Error> {
        reset(self.inner, ResetLine::Pcie3Phy, asserted)
    }
}

pub struct R5sPcie<I> {
    io: I,
    phy_ready: [bool; 2],
}

impl<I: ConfigIo> R5sPcie<I> {
    /// Requires exclusive control of the mapped PCIe registers and the shared
    /// PIPE provider. Call once before allowing either endpoint to decode traffic.
    pub fn prepare(mut io: I) -> Result<Self, BringupError<I::Error>> {
        gpio_prepare(&mut io, Lan::One)?;
        gpio_prepare(&mut io, Lan::Two)?;
        reset(&mut io, ResetLine::Lan1Pipe, true)?;
        reset(&mut io, ResetLine::Lan2Pipe, true)?;
        provider::prepare_shared(&mut io).map_err(BringupError::Provider)?;
        Ok(Self { io, phy_ready: [false; 2] })
    }

    /// Size RTL8125 BAR2, expose its non-prefetchable MMIO aperture, and
    /// identify the silicon method. Keep PCI bus mastering disabled.
    /// The BAR aperture must already be mapped as Device memory by the owner.
    pub fn identify_method(
        &mut self, lan: Lan,
    ) -> Result<(ring::Identity, ring::Method), BringupError<I::Error>> {
        let result = self.read_method(lan);
        if result.is_err() {
            self.isolate(lan)?;
        }
        result
    }

    pub fn revision(&mut self, lan: Lan) -> Result<u8, BringupError<I::Error>> {
        let (dbi, cfg, bus) = match lan {
            Lan::One => (0x3c0000000, 0xf4000000, 0),
            Lan::Two => (0x3c0400000, 0xf2000000, 16),
        };
        pcie::read_config(&mut self.io, dbi, (cfg, 0x100000), (bus, bus + 15),
                          (bus + 1, 0, 0), 8)
            .map(|class_revision| class_revision as u8)
            .map_err(BringupError::Probe)
    }

    pub fn into_io(self) -> I { self.io }

    pub fn after_shared_prepare(io: I) -> Self {
        Self { io, phy_ready: [false; 2] }
    }

    pub fn hold_reset(io: &mut I, lan: Lan) -> Result<(), BringupError<I::Error>> {
        let endpoint = perst(io, lan, true);
        let root = reset(io, match lan { Lan::One => ResetLine::Lan1Pipe, Lan::Two => ResetLine::Lan2Pipe }, true);
        endpoint.and(root)
    }

    pub fn inbound_region0_control(&mut self, lan: Lan) -> Result<u32, BringupError<I::Error>> {
        let address = match lan {
            Lan::One => 0x3c0300104,
            Lan::Two => 0x3c0700104,
        };
        self.io.read32(address).map_err(BringupError::Io)
    }

    pub fn map_inbound(
        &mut self, lan: Lan, window: &InboundWindow,
    ) -> Result<u32, BringupError<I::Error>> {
        let dbi = match lan {
            Lan::One => 0x3c0000000,
            Lan::Two => 0x3c0400000,
        };
        pcie::map_inbound(&mut self.io, dbi, window).map_err(BringupError::Probe)
    }

    fn isolate(&mut self, lan: Lan) -> Result<(), BringupError<I::Error>> {
        let pipe = match lan {
            Lan::One => ResetLine::Lan1Pipe,
            Lan::Two => ResetLine::Lan2Pipe,
        };
        let endpoint_reset = perst(&mut self.io, lan, true);
        let root_reset = reset(&mut self.io, pipe, true);
        endpoint_reset.and(root_reset)
    }

    fn read_method(
        &mut self, lan: Lan,
    ) -> Result<(ring::Identity, ring::Method), BringupError<I::Error>> {
        let (dbi, cfg, first_bus, memory) = match lan {
            Lan::One => (0x3c0000000, 0xf4000000, 0, 0xf4200000),
            Lan::Two => (0x3c0400000, 0xf2000000, 16, 0xf2200000),
        };
        let buses = (first_bus, first_bus + 15);
        let aperture = (cfg, 0x100000);
        pcie::probe_rtl8125(&mut self.io, dbi, aperture, buses).map_err(BringupError::Probe)?;
        let location = (buses.0.checked_add(1).ok_or(BringupError::Probe(
            pcie::ProbeError::InvalidLayout,
        ))?, 0, 0);
        let pci_id = pcie::read_config(&mut self.io, dbi, aperture, buses, location, 0)
            .map_err(BringupError::Probe)?;
        let class_revision = pcie::read_config(&mut self.io, dbi, aperture, buses, location, 8)
            .map_err(BringupError::Probe)?;
        let (cpu, bus, size) = {
            let mut endpoint = pcie::DirectEndpoint {
                io: &mut self.io, dbi, config_aperture: aperture, buses,
            };
            let (size, wide) = pcie::size_bar(&mut endpoint, 2).map_err(BringupError::Bar)?;
            let (cpu, bus, _) = pcie::allocate_bar(memory, memory, 0x01e00000, 0, size, wide)
                .ok_or(BringupError::Bar(pcie::BarError::InvalidBar))?;
            if !(4096..=0x01e00000).contains(&size) {
                return Err(BringupError::Bar(pcie::BarError::InvalidBar));
            }
            #[cfg(feature = "lan1-only")]
            if cpu != memory || size < 0x8000 {
                return Err(BringupError::Bar(pcie::BarError::InvalidBar));
            }
            pcie::program_bar(&mut endpoint, 2, bus, size, wide).map_err(BringupError::Bar)?;
            (cpu, bus, size)
        };

        pcie::map_memory(&mut self.io, dbi, cpu, bus, size).map_err(BringupError::Probe)?;
        // Type-one bridge memory base and limit fields are in 1 MiB steps.
        // Outbound region 0 only admits the smaller, sized BAR window.
        let window_end = bus + size - 1;
        let bridge_window = ((bus >> 16) as u32 & 0xfff0) | (window_end as u32 & 0xfff00000);
        self.io.write32(dbi + 0x20, bridge_window).map_err(BringupError::Io)?;

        let signature = (|| {
            self.io.write32(dbi + 4, 2).map_err(BringupError::Io)?;
            let mut endpoint = pcie::DirectEndpoint {
                io: &mut self.io, dbi, config_aperture: aperture, buses,
            };
            endpoint.write_dword(4, 2).map_err(BringupError::Probe)?;
            if endpoint.read_dword(4).map_err(BringupError::Probe)? & 7 != 2 {
                return Err(BringupError::Probe(pcie::ProbeError::InvalidLayout));
            }
            self.io.startup_phase(11);
            self.io.read32(cpu + 0x40).map_err(BringupError::Io)
        })();
        let mut endpoint = pcie::DirectEndpoint {
            io: &mut self.io, dbi, config_aperture: aperture, buses,
        };
        let disable = endpoint.disable_decode().map_err(BringupError::Probe);
        let disable_root = self.io.write32(dbi + 4, 0).map_err(BringupError::Io);
        disable.and(disable_root)?;
        if self.io.read32(dbi + 4).map_err(BringupError::Io)? & 7 != 0 {
            return Err(BringupError::Probe(pcie::ProbeError::InvalidLayout));
        }
        let identity = ring::Identity {
            pci_id,
            revision: class_revision as u8,
            tx_config: signature?,
        };
        let method = identity.classify().ok_or(BringupError::UnsupportedMethod(identity))?;
        if identity.supported_method() != Some(method) {
            return Err(BringupError::UnsupportedMethod(identity));
        }
        Ok((identity, method))
    }

    /// Train and detect a direct endpoint. DMA/PCI memory decoding stay disabled.
    /// A retry may reset only this root. The Gen3 PHY won't be reset after startup.
    pub fn detect(&mut self, lan: Lan) -> Result<(), BringupError<I::Error>> {
        let (index, pipe, apb, dbi, cfg, bus, generation) = match lan {
            Lan::One => (0, ResetLine::Lan1Pipe, 0xfe260000, 0x3c0000000, 0xf4000000, 0, 2),
            Lan::Two => (1, ResetLine::Lan2Pipe, 0xfe270000, 0x3c0400000, 0xf2000000, 16, 3),
        };
        perst(&mut self.io, lan, true)?;
        reset(&mut self.io, pipe, true)?;
        self.io.startup_phase(8);
        if !self.phy_ready[index] {
            match lan {
                Lan::One => {
                    reset(&mut self.io, ResetLine::Combo2Phy, true)?;
                    reset(&mut self.io, ResetLine::Combo2Apb, false)?;
                    if !phy::configure_combo2(&mut self.io, 100_000_000).map_err(BringupError::Io)? {
                        return Err(BringupError::PhyNotReady);
                    }
                    reset(&mut self.io, ResetLine::Combo2Phy, false)?;
                }
                Lan::Two => {
                    if !phy::start_pcie3(&mut PortIo { inner: &mut self.io, lan })? {
                        return Err(BringupError::PhyNotReady);
                    }
                }
            }
            self.phy_ready[index] = true;
        }
        self.io.startup_phase(9);
        reset(&mut self.io, pipe, false)?;
        provider::enable_root_clocks(&mut self.io, lan).map_err(BringupError::Provider)?;
        let buses = (bus, bus + 15);
        let result = (|| {
            pcie::configure_root(&mut self.io, apb, dbi, buses, generation).map_err(BringupError::Link)?;
            let mut port = PortIo { inner: &mut self.io, lan };
            pcie::train_link(&mut port, apb).map_err(|e| match e {
                pcie::LinkError::Io(cause) => cause,
                pcie::LinkError::InvalidLayout => BringupError::Link(pcie::LinkError::InvalidLayout),
                pcie::LinkError::Timeout => BringupError::Link(pcie::LinkError::Timeout),
            })?;
            #[cfg(feature = "lan1-only")]
            if self.io.read32(dbi + 0x900).map_err(BringupError::Io)? != u32::MAX {
                return Err(BringupError::Probe(pcie::ProbeError::InboundMode));
            }
            self.io.startup_phase(10);
            pcie::probe_rtl8125(&mut self.io, dbi, (cfg, 0x100000), buses)
                .map_err(BringupError::Probe)
        })();
        if result.is_err() {
            self.isolate(lan)?;
        }
        result
    }
}

#[cfg(test)]
#[test]
fn both_roots_detect_and_one_failed_retrain_preserves_the_other() {
    extern crate std;
    use std::{collections::BTreeMap, vec::Vec};

    struct Board {
        registers: BTreeMap<u64, u32>,
        failed: bool,
        gen3_resets: usize,
        phases: Vec<u32>,
        reject_lan1_decode_clear: bool,
    }
    impl ConfigIo for Board {
        type Error = ();
        fn read32(&mut self, p: u64) -> Result<u32, ()> {
            Ok(match p {
                0xfdd60078 => 0x1001157c,
                0xfdcb8080 => 1 << 14,
                0xfe260300 if self.failed => 0,
                0xfe260300 | 0xfe270300 => 0x30011,
                0xf4000000..=0xf4000fff | 0xf2000000..=0xf2000fff => {
                    let dbi = if p >= 0xf4000000 { 0x3c0000000 } else { 0x3c0400000 };
                    assert_eq!(self.registers[&(dbi + 0x300204)], 1 << 31);
                    assert_eq!(self.registers[&(dbi + 0x300214)], 1 << 24);
                    let stored = *self.registers.get(&p).unwrap_or(&0);
                    match p & 0xfff {
                        0 => 0x8125_10ec,
                        0x18 if stored == u32::MAX => 0xffff_c004,
                        _ => stored,
                    }
                }
                0xf4200040 | 0xf2200040 => {
                    let (dbi, cfg) = if p >= 0xf4200000 {
                        (0x3c0000000, 0xf4000000)
                    } else {
                        (0x3c0400000, 0xf2000000)
                    };
                    assert_eq!(self.registers[&(dbi + 0x300004)], 1 << 31);
                    assert_eq!(self.registers[&(dbi + 4)], 2);
                    assert_eq!(self.registers[&(cfg + 4)], 2);
                    assert_eq!(self.registers[&(cfg + 0x18)], (p as u32 & !0xfff) | 4);
                    *self.registers.get(&p).unwrap_or(&0x6410_0000)
                }
                _ => *self.registers.get(&p).unwrap_or(&0),
            })
        }
        fn write32(&mut self, p: u64, v: u32) -> Result<(), ()> {
            if matches!(p, 0x3c0000004 | 0x3c0400004 | 0xf4000004 | 0xf2000004) {
                assert_eq!(v & 4, 0, "discovery must never write Bus Master Enable");
            }
            let masked = matches!(p,
                0xfdd20000..=0xfdd2ffff | 0xfdd00000..=0xfdd0ffff |
                0xfdc20000..=0xfdc2ffff | 0xfdc90000..=0xfdc9ffff |
                0xfdd60000..=0xfdd6ffff | 0xfdcb8000..=0xfdcbffff);
            if p == 0xfdd2046c && v & 0x40004000 == 0x40004000 {
                self.gen3_resets += 1;
            }
            let value = if masked {
                let mask = v >> 16;
                (*self.registers.get(&p).unwrap_or(&0) & !mask) | (v & mask)
            } else { v };
            if !(p == 0xf4000004 && v == 0 && self.reject_lan1_decode_clear
                && self.registers.get(&p) == Some(&2))
            {
                self.registers.insert(p, value);
            }
            Ok(())
        }
        fn delay_us(&mut self, _: u32) {}
        fn startup_phase(&mut self, phase: u32) { self.phases.push(phase); }
    }

    let mut board = Board { registers: BTreeMap::new(), failed: false, gen3_resets: 0, phases: Vec::new(), reject_lan1_decode_clear: false };
    board.registers.insert(0xfdd00080, 1);
    board.registers.insert(0xfdd00000, 100 | (6 << 12));
    board.registers.insert(0xfdd00004, 1 | (2 << 6) | (1 << 12) | (1 << 10));
    board.registers.insert(0xf4000018, 4);
    board.registers.insert(0xf2000018, 4);
    board.registers.insert(0x3c0700104, 1 << 31);
    board.registers.extend([(0xf4000008, 5), (0xf2000008, 5),
                            (0x3c0000900, u32::MAX), (0x3c0400900, u32::MAX)]);
    let mut platform = R5sPcie::prepare(board).unwrap();
    platform.detect(Lan::One).unwrap();
    platform.detect(Lan::Two).unwrap();
    assert_eq!(platform.io.gen3_resets, 1);
    assert_eq!(platform.identify_method(Lan::One), Ok((ring::Identity {
        pci_id: 0x8125_10ec, revision: 5, tx_config: 0x6410_0000,
    }, ring::Method::B51)));
    assert_eq!(platform.identify_method(Lan::Two), Ok((ring::Identity {
        pci_id: 0x8125_10ec, revision: 5, tx_config: 0x6410_0000,
    }, ring::Method::B51)));
    assert_eq!(platform.io.phases, [8, 9, 10, 8, 9, 10, 11, 11]);
    assert_eq!(platform.inbound_region0_control(Lan::One), Ok(0));
    assert_eq!(platform.inbound_region0_control(Lan::Two), Ok(1 << 31));
    assert_eq!(platform.io.registers[&0xf4000004] & 7, 0);
    assert_eq!(platform.io.registers[&0xf2000004] & 7, 0);
    assert_eq!(platform.revision(Lan::One), Ok(5));
    assert_eq!(platform.revision(Lan::Two), Ok(5));
    for (lan, dbi, physical, bus) in [(Lan::One, 0x3c0000000, 0x24200000, 0x80000000),
                                      (Lan::Two, 0x3c0400000, 0x24300000, 0x81000000)] {
        platform.map_inbound(lan, &InboundWindow { physical, bus, size: 0x100000 }).unwrap();
        assert_eq!(platform.io.registers[&(dbi + 0x300108)], bus as u32);
        assert_eq!(platform.io.registers[&(dbi + 0x300114)], physical as u32);
    }
    platform.io.registers.insert(0xf2200040, 0xdead_beef);
    assert_eq!(platform.identify_method(Lan::Two), Err(BringupError::UnsupportedMethod(ring::Identity {
        pci_id: 0x8125_10ec, revision: 5, tx_config: 0xdead_beef,
    })));
    assert_eq!(platform.io.registers[&0xf2000004] & 7, 0);
    assert!(matches!(platform.identify_method(Lan::One), Ok((_, ring::Method::B51))));
    platform.io.registers.insert(0xf2200040, 0x6410_0000);
    platform.io.registers.insert(0xf2000008, 6);
    assert_eq!(platform.identify_method(Lan::Two), Err(BringupError::UnsupportedMethod(ring::Identity {
        pci_id: 0x8125_10ec, revision: 6, tx_config: 0x6410_0000,
    })));
    platform.io.registers.insert(0xf2000008, 5);
    platform.detect(Lan::Two).unwrap();
    platform.io.reject_lan1_decode_clear = true;
    assert_eq!(platform.identify_method(Lan::One), Err(BringupError::Probe(pcie::ProbeError::InvalidLayout)));
    assert_eq!(platform.io.registers[&GPIO0] & ((1 << 14) | 1), 1);
    assert_eq!(platform.io.registers[&0xfdd20428] & 2, 2);
    assert!(matches!(platform.identify_method(Lan::Two), Ok((_, ring::Method::B51))));
    platform.io.failed = true;
    platform.io.phases.clear();
    assert_eq!(platform.detect(Lan::One), Err(BringupError::Link(pcie::LinkError::Timeout)));
    assert_eq!(platform.io.phases, [8, 9]);
    assert_eq!(platform.io.registers[&GPIO0] & ((1 << 14) | 1), 1);
    assert_eq!(platform.io.registers[&0xfdd20428] & 2, 2);
    assert_eq!(platform.io.registers[&0xfdd2042c] & 2, 0);
    assert_eq!(platform.io.gen3_resets, 1);
    assert_eq!(platform.io.registers[&0x3c0400004], 0);
}
