//! Ordered register access over already-established Microkit Device mappings.

use core::ptr::NonNull;

use super::pcie::ConfigIo;

#[derive(Clone, Copy)]
pub struct Mapping {
    pub physical: u64,
    pub virtual_base: NonNull<u8>,
    pub size: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessError {
    InvalidMapping,
    UnmappedRegister,
}

pub struct DeviceIo<'a, F> {
    maps: &'a [Mapping],
    delay: F,
}

impl<'a, F: FnMut(u32)> DeviceIo<'a, F> {
    /// # Safety
    /// Windows must be exclusive to this owner, mapped as Device memory, kept
    /// live for the lifetime of the backend and valid for aligned 32-bit access.
    /// MMIO faults are handled by the owning PD, not caught by this backend.
    pub unsafe fn new(maps: &'a [Mapping], delay: F) -> Result<Self, AccessError> {
        for (index, map) in maps.iter().enumerate() {
            let end = map.physical.checked_add(map.size as u64)
                .ok_or(AccessError::InvalidMapping)?;
            let va = map.virtual_base.as_ptr() as usize;
            let virtual_end = va.checked_add(map.size).ok_or(AccessError::InvalidMapping)?;
            if map.size < 4 || (map.size | va) & 3 != 0 || map.physical & 3 != 0 {
                return Err(AccessError::InvalidMapping);
            }
            for other in &maps[..index] {
                let other_va = other.virtual_base.as_ptr() as usize;
                let (Some(other_end), Some(other_virtual_end)) = (
                    other.physical.checked_add(other.size as u64),
                    other_va.checked_add(other.size),
                ) else {
                    return Err(AccessError::InvalidMapping);
                };
                if (map.physical < other_end && other.physical < end)
                    || (va < other_virtual_end && other_va < virtual_end)
                {
                    return Err(AccessError::InvalidMapping);
                }
            }
        }
        Ok(Self { maps, delay })
    }

    pub fn read8(&mut self, address: u64) -> Result<u8, AccessError> {
        let register = self.register(address, 1)?;
        ordered();
        let value = unsafe { register.read_volatile() };
        ordered();
        Ok(value)
    }

    pub fn read16(&mut self, address: u64) -> Result<u16, AccessError> {
        let register = self.register(address, 2)?.cast::<u16>();
        ordered();
        let value = unsafe { register.read_volatile() };
        ordered();
        Ok(u16::from_le(value))
    }

    pub fn write8(&mut self, address: u64, value: u8) -> Result<(), AccessError> {
        let register = self.register(address, 1)?;
        ordered();
        unsafe { register.write_volatile(value); }
        ordered();
        Ok(())
    }

    pub fn write16(&mut self, address: u64, value: u16) -> Result<(), AccessError> {
        let register = self.register(address, 2)?.cast::<u16>();
        ordered();
        unsafe { register.write_volatile(value.to_le()); }
        ordered();
        Ok(())
    }

    fn register(&self, physical: u64, width: usize) -> Result<*mut u8, AccessError> {
        if physical & (width as u64 - 1) != 0 {
            return Err(AccessError::UnmappedRegister);
        }
        for map in self.maps {
            if let Some(offset) = physical.checked_sub(map.physical) {
                if offset <= (map.size - width) as u64 {
                    return Ok(map.virtual_base.as_ptr().wrapping_add(offset as usize));
                }
            }
        }
        Err(AccessError::UnmappedRegister)
    }
}

pub fn ordered() {
    #[cfg(target_arch = "aarch64")]
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) }
}

impl<F: FnMut(u32)> ConfigIo for DeviceIo<'_, F> {
    type Error = AccessError;
    fn read32(&mut self, address: u64) -> Result<u32, Self::Error> {
        let register = self.register(address, 4)?.cast::<u32>();
        ordered();
        let value = unsafe { register.read_volatile() };
        ordered();
        Ok(u32::from_le(value))
    }
    fn write32(&mut self, address: u64, value: u32) -> Result<(), Self::Error> {
        let register = self.register(address, 4)?.cast::<u32>();
        ordered();
        unsafe { register.write_volatile(value.to_le()) };
        ordered();
        Ok(())
    }
    #[cfg(feature = "supervised")]
    fn startup_phase(&mut self, phase: u32) {
        crate::phase(phase);
    }

    #[cfg(feature = "tx-diagnostic")]
    fn inbound_step(&mut self, operation: u32, category: u32, address: u64, expected: u32, observed: u32) {
        let sample = match (operation, category) {
            (2, 4) => Some(45),
            (4, 6) => Some(46),
            (5, 7) => Some(47),
            _ => None,
        };
        if let Some(index) = sample {
            crate::status(index).store(observed, core::sync::atomic::Ordering::Relaxed);
        }
        for (index, value) in [operation, category, address as u32, (address >> 32) as u32, expected, observed].into_iter().enumerate() {
            crate::status(32 + index).store(value, core::sync::atomic::Ordering::Relaxed);
        }
    }

    fn delay_us(&mut self, microseconds: u32) {
        (self.delay)(microseconds)
    }
}

#[cfg(test)]
#[test]
fn bounds_and_aliases_fail_before_pointer_access() {
    let mut backing = [0u32; 4];
    let mapping = Mapping {
        physical: 0x3c0000000,
        virtual_base: NonNull::new(backing.as_mut_ptr().cast()).unwrap(),
        size: 16,
    };
    let duplicate = [mapping, Mapping { physical: 0x3c0400000, ..mapping }];
    assert!(matches!(unsafe { DeviceIo::new(&duplicate, |_| {}) }, Err(AccessError::InvalidMapping)));
    let maps = [mapping];
    let mut io = unsafe { DeviceIo::new(&maps, |_| {}) }.unwrap();
    assert_eq!(io.write32(mapping.physical + 12, 0x1234), Ok(()));
    assert_eq!(io.read32(mapping.physical + 12), Ok(0x1234));
    assert_eq!(io.read32(mapping.physical + 2), Err(AccessError::UnmappedRegister));
    assert_eq!(io.write32(mapping.physical + 16, 0), Err(AccessError::UnmappedRegister));
    assert_eq!(backing, [0, 0, 0, 0x1234]);
}
