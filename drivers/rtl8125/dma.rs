//! RTL8125 packed tables with a verified coherent or Normal-NC DMA mapping.
use core::ptr::NonNull;

use crate::{mac::Registers, mmio::ordered, ring::{Dma, Descriptor, Error, BUFFER}};

const OWN: u32 = 1 << 31;
#[cfg(target_arch = "aarch64")]
const OUTER_SHAREABLE_DMA: bool = match option_env!("R5S_B51_DMA_BARRIERS") {
    Some(mode) => matches!(mode.as_bytes(), b"outer"),
    None => false,
};

#[inline]
fn publish_to_dma() {
    #[cfg(target_arch = "aarch64")]
    if OUTER_SHAREABLE_DMA {
        unsafe { core::arch::asm!("dsb oshst", options(nostack, preserves_flags)) };
        return;
    }
    ordered();
}

#[inline]
fn observe_dma() {
    #[cfg(target_arch = "aarch64")]
    if OUTER_SHAREABLE_DMA {
        unsafe { core::arch::asm!("dsb oshld", options(nostack, preserves_flags)) };
        return;
    }
    ordered();
}
const DESC: usize = 16;

fn v3_receive_status(status: u32, options: u32) -> u32 {
    let shape = (status & (3 << 24)) << 4;
    let error = u32::from(status & ((15 << 17) | (15 << 26)) != 0) << 21;
    let checksum = if options & ((3 << 30) | (1 << 26)) == 1 << 30 {
        match options & ((3 << 28) | (3 << 24)) {
            x if x == 1 << 28 => 1 << 17,
            x if x == 1 << 29 => 1 << 18,
            _ => 0,
        }
    } else { 0 };
    (status & 0x3fff) | shape | error | checksum
}

pub struct DmaMemory {
    pub cpu: NonNull<u8>,
    pub physical: u64,
    pub size: usize,
}

pub struct InboundWindow {
    pub physical: u64,
    pub bus: u64,
    pub size: u64,
}

impl InboundWindow {
    pub fn translate(&self, physical: u64, size: u64) -> Option<u64> {
        let last = self.size.checked_sub(1)?;
        self.physical.checked_add(last)?;
        self.bus.checked_add(last)?;
        let offset = physical.checked_sub(self.physical)?;
        if size == 0 || offset.checked_add(size)? > self.size {
            return None;
        }
        let bus = self.bus.checked_add(offset)?;
        bus.checked_add(size - 1)?;
        Some(bus)
    }
}

pub const TX_TAIL: u64 = 0x2800;
pub const TX_CLOSE: u64 = 0x2802;

const fn rx_descriptor_offset(slots: usize, extended_tx: bool) -> usize {
    if extended_tx { ((slots + 1) * 32 + 255) & !255 } else { slots * DESC }
}

pub const fn descriptor_bytes(slots: usize, extended_tx: bool) -> usize {
    queue_descriptor_bytes(slots, extended_tx, 1)
}

const fn rx_queue_stride(slots: usize, extended_tx: bool, queues: usize) -> usize {
    let stride = if queues > 1 { 32 } else { DESC };
    ((slots + extended_tx as usize) * stride + 255) & !255
}

pub const fn queue_descriptor_bytes(slots: usize, extended_tx: bool, queues: usize) -> usize {
    rx_descriptor_offset(slots, extended_tx) + queues * rx_queue_stride(slots, extended_tx, queues)
}

pub const fn tx_rx_descriptor_bytes(slots: usize, extended_tx: bool, rx_queues: usize, tx_queues: usize) -> usize {
    queue_descriptor_bytes(slots, extended_tx, rx_queues)
        + tx_queues.saturating_sub(1) * rx_descriptor_offset(slots, extended_tx)
}

#[derive(Clone, Copy, Default)]
struct TxCursor {
    no_close_tail: Option<u16>,
    tx_closed: u16,
    defer_tx: bool,
    tx_deferred: usize,
    tx_unpublished: usize,
}

pub struct PackedDma<I, const N: usize, const EXTENDED_TX: bool = false, const RX_QUEUES: usize = 1,
    const TX_QUEUES: usize = 1> {
    io: I,
    bar: u64,
    descriptors: DmaMemory,
    buffers: DmaMemory,
    descriptor_bus: u64,
    buffer_bus: u64,
    window: InboundWindow,
    rx_queue: usize,
    tx_queue: usize,
    tx: TxCursor,
    tx_saved: [TxCursor; TX_QUEUES],
}

impl<I: Registers, const N: usize, const EXTENDED_TX: bool, const RX_QUEUES: usize, const TX_QUEUES: usize>
    PackedDma<I, N, EXTENDED_TX, RX_QUEUES, TX_QUEUES>
{
    const TX_STRIDE: usize = if EXTENDED_TX { 32 } else { DESC };
    // FriendlyARM reserves an extra entry after each ring. Keep the RX base 256-byte aligned.
    const RX_OFFSET: usize = TX_QUEUES * rx_descriptor_offset(N, EXTENDED_TX);
    pub const DESCRIPTOR_BYTES: usize = tx_rx_descriptor_bytes(N, EXTENDED_TX, RX_QUEUES, TX_QUEUES);

    /// # Safety
    /// Both ranges must be uniquely owned for the backend's lifetime, reserved
    /// outside kernel/loader/PD allocations, with no conflicting CPU aliases.
    /// They must use verified Normal non-cacheable memory, or Normal cacheable
    /// memory with verified DMA coherency for this root. The inbound hardware
    /// window and its address width must already have been verified. Bus mastering
    /// must remain off until the tables are initialized. Multiple RX queues require V3 mode.
    pub unsafe fn from_verified_memory(
        io: I, bar: u64, descriptors: DmaMemory, buffers: DmaMemory,
        inbound: &InboundWindow,
    ) -> Result<Self, Error> {
        if !(2..=1024).contains(&N) || !N.is_power_of_two() || bar.checked_add(0x91).is_none()
            || bar & 1 != 0 || !matches!(RX_QUEUES, 1 | 2 | 4)
            || !(TX_QUEUES == 1 || TX_QUEUES == 2 && EXTENDED_TX)
        {
            return Err(Error::Layout);
        }
        let valid = |memory: &DmaMemory, required: usize| {
            let cpu = memory.cpu.as_ptr() as usize;
            memory.size == required && (cpu | required) & 255 == 0
                && memory.physical & 255 == 0
                && cpu.checked_add(required).is_some()
                && memory.physical.checked_add(required as u64).is_some()
        };
        if !valid(&descriptors, Self::DESCRIPTOR_BYTES)
            || !valid(&buffers, N * BUFFER * (RX_QUEUES + TX_QUEUES))
        {
            return Err(Error::Layout);
        }
        fn overlap(a: u64, n: usize, b: u64, m: usize) -> bool {
            let (Some(a_end), Some(b_end)) = (a.checked_add(n as u64), b.checked_add(m as u64)) else {
                return true;
            };
            a < b_end && b < a_end
        }
        if overlap(descriptors.physical, descriptors.size, buffers.physical, buffers.size)
            || overlap(descriptors.cpu.as_ptr() as usize as u64, descriptors.size,
                       buffers.cpu.as_ptr() as usize as u64, buffers.size)
        {
            return Err(Error::Layout);
        }
        let descriptor_bus = inbound.translate(descriptors.physical, descriptors.size as u64)
            .ok_or(Error::Layout)?;
        let buffer_bus = inbound.translate(buffers.physical, buffers.size as u64)
            .ok_or(Error::Layout)?;
        if descriptor_bus & 255 != 0 || buffer_bus & 2047 != 0
            || buffers.physical & 2047 != 0 || buffers.cpu.as_ptr() as usize & 2047 != 0
            || (N * DESC) % 256 != 0
        {
            return Err(Error::Layout);
        }
        Ok(Self { io, bar, descriptors, buffers, descriptor_bus, buffer_bus,
                  window: InboundWindow { physical: inbound.physical, bus: inbound.bus, size: inbound.size },
                  rx_queue: 0, tx_queue: 0,
                  tx: TxCursor::default(), tx_saved: [TxCursor::default(); TX_QUEUES] })
    }

    /// # Safety
    /// The root must prevent new DMA, and the device and fabric must have
    /// drained all outstanding transactions from its previous generation.
    pub unsafe fn clear_after_stop(&mut self) {
        self.rx_queue = 0;
        self.tx_queue = 0;
        self.tx = TxCursor::default();
        self.tx_saved.fill(TxCursor::default());
        for i in 0..(self.descriptors.size / 4) {
            unsafe { self.descriptors.cpu.as_ptr().cast::<u32>().add(i).write_volatile(0); }
        }
        ordered();
    }

    pub fn enable_no_close(&mut self) -> Result<(), Error> {
        if self.tx.no_close_tail.is_some() { return Err(Error::Busy); }
        self.bar.checked_add(TX_CLOSE + self.tx_queue as u64 * 4 + 1).ok_or(Error::Layout)?;
        if self.io.read16(self.bar + TX_TAIL + self.tx_queue as u64 * 4).map_err(|_| Error::Backend)? != 0
            || self.io.read16(self.bar + TX_CLOSE + self.tx_queue as u64 * 4).map_err(|_| Error::Backend)? != 0
        {
            return Err(Error::Backend);
        }
        self.tx.no_close_tail = Some(0);
        self.tx.tx_unpublished = 0;
        Ok(())
    }

    pub fn tx_inflight(&self) -> usize {
        usize::from(self.tx.no_close_tail.unwrap_or(self.tx.tx_closed).wrapping_sub(self.tx.tx_closed))
    }

    pub fn oldest_tx_slot(&self) -> usize { usize::from(self.tx.tx_closed) % N }

    pub fn begin_tx_batch(&mut self) {
        self.tx.defer_tx = self.tx.no_close_tail.is_some();
    }

    pub fn end_tx_batch(&mut self) -> Result<(), Error> {
        self.tx.defer_tx = false;
        if self.tx.tx_deferred != 0 {
            publish_to_dma();
            let tail = self.tx.no_close_tail.ok_or(Error::Layout)?;
            self.io.write16(self.bar + TX_TAIL + self.tx_queue as u64 * 4, tail).map_err(|_| Error::Backend)?;
            self.tx.tx_deferred = 0;
        }
        Ok(())
    }

    pub fn reclaim_tx(&mut self) -> Result<usize, Error> {
        let pending = self.tx_inflight() - self.tx.tx_deferred;
        if pending == 0 { return Ok(0); }
        let close = self.io.read16(self.bar + TX_CLOSE + self.tx_queue as u64 * 4).map_err(|_| Error::Backend)?;
        let completed = usize::from(close.wrapping_sub(self.tx.tx_closed));
        if completed > pending { return Err(Error::Backend); }
        observe_dma();
        self.tx.tx_closed = close;
        Ok(completed)
    }

    fn tx_slot_pending(&self, slot: usize) -> bool {
        (slot + N - self.oldest_tx_slot()) % N < self.tx_inflight()
    }

    pub fn registers(&mut self) -> &mut I { &mut self.io }

    pub fn bus_addresses(&self) -> (u64, u64, u64) {
        (self.descriptor_bus, self.descriptor_bus + Self::RX_OFFSET as u64, self.buffer_bus)
    }

    pub fn rx_queue_addresses(&self, queue: usize) -> Option<(u64, u64)> {
        if queue >= RX_QUEUES { return None; }
        Some((self.descriptor_bus + (Self::RX_OFFSET
            + queue * rx_queue_stride(N, EXTENDED_TX, RX_QUEUES)) as u64,
            self.buffer_bus + ((queue + TX_QUEUES) * N * BUFFER) as u64))
    }

    pub fn tx_queue_address(&self, queue: usize) -> Option<u64> {
        (queue < TX_QUEUES)
            .then(|| self.descriptor_bus + (queue * rx_descriptor_offset(N, EXTENDED_TX)) as u64)
    }

    pub fn select_tx_queue(&mut self, queue: usize) -> Result<(), Error> {
        if queue >= TX_QUEUES { return Err(Error::Layout); }
        if queue != self.tx_queue {
            self.tx_saved[self.tx_queue] = self.tx;
            self.tx_queue = queue;
            self.tx = self.tx_saved[queue];
        }
        Ok(())
    }

    pub fn select_rx_queue(&mut self, queue: usize) -> Result<(), Error> {
        if queue >= RX_QUEUES { return Err(Error::Layout); }
        self.rx_queue = queue;
        Ok(())
    }

    /// Translates a CPU physical range inside the verified inbound window to
    /// its device-visible bus address for external buffer references.
    /// Inverse of `translate_external`: the CPU-physical address behind a
    /// device-visible buffer address inside the inbound window.
    pub fn external_physical(&self, bus: u64) -> Result<u64, Error> {
        let offset = bus.checked_sub(self.window.bus).ok_or(Error::Layout)?;
        if offset >= self.window.size {
            return Err(Error::Layout);
        }
        self.window.physical.checked_add(offset).ok_or(Error::Layout)
    }

    pub fn translate_external(&self, physical: u64, size: u64) -> Result<u64, Error> {
        self.window.translate(physical, size).ok_or(Error::Layout)
    }

    /// True when the bus address falls inside any private queue's buffer arena.
    pub fn is_private_buffer(&self, bus: u64) -> bool {
        bus >= self.buffer_bus && (bus - self.buffer_bus) < self.buffers.size as u64
    }

    pub fn tx_descriptor_words(&self, slot: usize) -> Result<[u32; 8], Error> {
        let pointer = self.descriptor(false, slot)?;
        let mut words = [0; 8];
        ordered();
        for (i, word) in words[..Self::TX_STRIDE / 4].iter_mut().enumerate() {
            *word = u32::from_le(unsafe { pointer.add(i).read_volatile() });
        }
        Ok(words)
    }

    fn index(&self, rx: bool, slot: usize) -> Result<usize, Error> {
        if slot >= N { return Err(Error::Layout); }
        Ok(slot + if rx { (self.rx_queue + TX_QUEUES) * N } else { self.tx_queue * N })
    }

    fn descriptor(&self, rx: bool, slot: usize) -> Result<*mut u32, Error> {
        self.index(rx, slot)?;
        let offset = if rx {
            let stride = if RX_QUEUES > 1 { 32 } else { DESC };
            Self::RX_OFFSET + self.rx_queue * rx_queue_stride(N, EXTENDED_TX, RX_QUEUES) + slot * stride
        } else { self.tx_queue * rx_descriptor_offset(N, EXTENDED_TX) + slot * Self::TX_STRIDE };
        Ok(self.descriptors.cpu.as_ptr().wrapping_add(offset).cast())
    }

    fn v3(&self, rx: bool) -> bool { rx && RX_QUEUES > 1 }

    fn write_descriptor(&self, receive: bool, d: *mut u32, value: Descriptor) {
        let (status, options, address) = if self.v3(receive) { (7, 6, 4) } else { (0, 1, 2) };
        unsafe {
            d.add(options).write_volatile(value.options.to_le());
            d.add(address).write_volatile((value.address as u32).to_le());
            d.add(address + 1).write_volatile(((value.address >> 32) as u32).to_le());
            publish_to_dma();
            d.add(status).write_volatile(value.flags.to_le());
        }
        publish_to_dma();
    }

    fn publish_external_descriptor<const CHECKSUM: bool>(
        &mut self, rx: bool, slot: usize, value: Descriptor,
    ) -> Result<(), Error> {
        let d = self.descriptor(rx, slot)?;
        // The copy path enforces the no-close slot discipline inside
        // `write_padded_payload`; the external path has no fill step, so the
        // same checks run here before the descriptor is written.
        if !rx {
            if let Some(tail) = self.tx.no_close_tail {
                if slot != usize::from(tail) % N { return Err(Error::Layout); }
                if self.tx_inflight() >= N - 1 { return Err(Error::Busy); }
            }
        }
        let end = if slot == N - 1 { 1 << 30 } else { 0 };
        let valid_flags = if rx {
            value.flags == OWN | end | BUFFER as u32
        } else {
            let length = value.flags & 0xffff;
            value.flags == end ||
                ((60..=1514).contains(&length) && value.flags == OWN | end | (3 << 28) | length)
        };
        use crate::ring::{TX_IPV4_CHECKSUM, TX_TCP_CHECKSUM, TX_UDP_CHECKSUM};
        let valid_options = if CHECKSUM { !rx && EXTENDED_TX
            && self.tx.no_close_tail.is_some() && value.flags & OWN != 0
            && matches!(value.options, TX_IPV4_CHECKSUM | TX_TCP_CHECKSUM | TX_UDP_CHECKSUM)
        } else { value.options == 0 };
        if !valid_flags || !valid_options || value.address & 2047 != 0
            || self.is_private_buffer(value.address) || self.status(rx, slot)? & OWN != 0
        {
            return Err(Error::Layout);
        }
        // The caller seeds checksums before publication when offload is requested.
        self.write_descriptor(rx, d, value);
        Ok(())
    }

}

impl<I: Registers, const N: usize, const EXTENDED_TX: bool, const RX_QUEUES: usize, const TX_QUEUES: usize> Dma
    for PackedDma<I, N, EXTENDED_TX, RX_QUEUES, TX_QUEUES>
{
    fn status(&mut self, rx: bool, slot: usize) -> Result<u32, Error> {
        let d = self.descriptor(rx, slot)?;
        let v3 = self.v3(rx);
        let flags = u32::from_le(unsafe { d.add(if v3 { 7 } else { 0 }).read_volatile() });
        observe_dma();
        if v3 && flags & OWN == 0 {
            let options = u32::from_le(unsafe { d.add(6).read_volatile() });
            return Ok(v3_receive_status(flags, options));
        }
        if !rx && self.tx.no_close_tail.is_some() {
            if self.tx_slot_pending(slot) { self.reclaim_tx()?; }
            return Ok(if self.tx_slot_pending(slot) { flags | OWN } else { flags & !OWN });
        }
        Ok(flags)
    }

    fn publish(&mut self, rx: bool, slot: usize, value: Descriptor) -> Result<(), Error> {
        let i = self.index(rx, slot)?;
        let d = self.descriptor(rx, slot)?;
        let end = if slot == N - 1 { 1 << 30 } else { 0 };
        let valid_flags = if rx {
            value.flags == OWN | end | BUFFER as u32
        } else {
            let length = value.flags & 0xffff;
            value.flags == end ||
                ((60..=1514).contains(&length) && value.flags == OWN | end | (3 << 28) | length)
        };
        use crate::ring::{TX_IPV4_CHECKSUM,TX_TCP_CHECKSUM,TX_UDP_CHECKSUM};
        let valid_options = value.options == 0 || (!rx && EXTENDED_TX
            && self.tx.no_close_tail.is_some() && value.flags & OWN != 0
            && matches!(value.options,TX_IPV4_CHECKSUM|TX_TCP_CHECKSUM|TX_UDP_CHECKSUM));
        if !valid_flags || !valid_options || value.address != self.buffer_bus + (i * BUFFER) as u64
            || self.status(rx, slot)? & OWN != 0
        {
            return Err(Error::Layout);
        }
        unsafe {
            if value.options & TX_IPV4_CHECKSUM != 0 {
                let packet = self.buffers.cpu.as_ptr().add(i * BUFFER);
                packet.add(24).write(0);
                packet.add(25).write(0);
                if matches!(value.options, TX_TCP_CHECKSUM|TX_UDP_CHECKSUM) {
                    let frame=core::slice::from_raw_parts_mut(packet,(value.flags&0xffff) as usize);
                    let length=u16::from_be_bytes([frame[16],frame[17]]);
                    let (protocol,offset,min)=if value.options==TX_TCP_CHECKSUM {(6,50,40)} else {(17,40,28)};
                    if length<min || usize::from(length)>frame.len()-14 || frame[23]!=protocol {
                        return Err(Error::Layout);
                    }
                    let mut sum=u32::from(length-20)+u32::from(protocol);
                    for pair in frame[26..34].chunks_exact(2) {
                        sum+=u32::from(u16::from_be_bytes([pair[0],pair[1]]));
                    }
                    sum=(sum&65535)+(sum>>16);
                    sum=(sum&65535)+(sum>>16);
                    frame[offset..offset+2].copy_from_slice(&(sum as u16).to_be_bytes());
                }
            }
        }
        self.write_descriptor(rx, d, value);
        Ok(())
    }

    fn publish_external(&mut self, rx: bool, slot: usize, value: Descriptor) -> Result<(), Error> {
        self.publish_external_descriptor::<false>(rx, slot, value)
    }

    fn publish_external_checksum(&mut self, slot: usize, value: Descriptor) -> Result<(), Error> {
        self.publish_external_descriptor::<true>(false, slot, value)
    }

    fn descriptor_address(&mut self, rx: bool, slot: usize) -> Result<u64, Error> {
        let d = self.descriptor(rx, slot)?;
        ordered();
        let address = if self.v3(rx) { 4 } else { 2 };
        let low = u32::from_le(unsafe { d.add(address).read_volatile() });
        let high = u32::from_le(unsafe { d.add(address + 1).read_volatile() });
        Ok((u64::from(high) << 32) | u64::from(low))
    }

    fn sync_receive_buffer(&mut self, slot: usize) -> Result<(), Error> {
        self.index(true, slot)?;
        ordered();
        Ok(())
    }

    fn read_payload(&mut self, slot: usize, output: &mut [u8]) -> Result<(), Error> {
        let i = self.index(true, slot)?;
        if output.len() > BUFFER || self.status(true, slot)? & OWN != 0 {
            return Err(Error::Layout);
        }
        let p = self.buffers.cpu.as_ptr().wrapping_add(i * BUFFER);
        unsafe { core::ptr::copy_nonoverlapping(p, output.as_mut_ptr(), output.len()); }
        Ok(())
    }

    fn write_padded_payload(&mut self, slot: usize, input: &[u8], padded: usize) -> Result<(), Error> {
        let i = self.index(false, slot)?;
        if let Some(tail) = self.tx.no_close_tail {
            if slot != usize::from(tail) % N { return Err(Error::Layout); }
            if self.tx_inflight() >= N - 1 { return Err(Error::Busy); }
        }
        if padded < input.len() || padded > BUFFER || self.status(false, slot)? & OWN != 0 {
            return Err(Error::Layout);
        }
        let p = self.buffers.cpu.as_ptr().wrapping_add(i * BUFFER);
        unsafe {
            core::ptr::copy_nonoverlapping(input.as_ptr(), p, input.len());
            p.add(input.len()).write_bytes(0, padded - input.len());
        }
        Ok(())
    }

    fn kick_tx(&mut self) -> Result<(), Error> {
        if let Some(tail) = self.tx.no_close_tail.as_mut() {
            *tail = tail.wrapping_add(1);
            if self.tx.defer_tx {
                self.tx.tx_deferred += 1;
                return Ok(());
            }
            publish_to_dma();
            return self.io.write16(self.bar + TX_TAIL + self.tx_queue as u64 * 4, *tail).map_err(|_| Error::Backend);
        }
        if self.tx_queue != 0 { return Err(Error::Layout); }
        publish_to_dma();
        self.io.write16(self.bar + 0x90, 1).map_err(|_| Error::Backend)
    }

    fn publish_tso(&mut self, slot: usize, value: Descriptor) -> Result<(), Error> {
        let d = self.descriptor(false, slot)?;
        if let Some(tail) = self.tx.no_close_tail {
            if slot != (usize::from(tail) + self.tx.tx_unpublished) % N {
                return Err(Error::Layout);
            }
            if self.tx_inflight() + self.tx.tx_unpublished >= N - 1 {
                return Err(Error::Busy);
            }
        }
        use crate::ring::{TSO_GIANT_SEND_V4, TSO_GIANT_SEND_V6};
        let giant = value.flags & (TSO_GIANT_SEND_V4 | TSO_GIANT_SEND_V6);
        if value.flags & OWN == 0
            || !matches!(giant, TSO_GIANT_SEND_V4 | TSO_GIANT_SEND_V6)
            || value.flags & 0xffff == 0
            || self.status(false, slot)? & OWN != 0
        {
            return Err(Error::Layout);
        }
        // No payload byte is touched: TSO fragments live outside this arena.
        self.write_descriptor(false, d, value);
        self.tx.tx_unpublished += 1;
        Ok(())
    }

    fn kick_tx_count(&mut self, count: usize) -> Result<(), Error> {
        publish_to_dma();
        if let Some(tail) = self.tx.no_close_tail.as_mut() {
            *tail = tail.wrapping_add(count as u16);
            self.tx.tx_unpublished = self.tx.tx_unpublished.saturating_sub(count);
            self.tx.tx_deferred = 0;
            return self.io.write16(self.bar + TX_TAIL + self.tx_queue as u64 * 4, *tail).map_err(|_| Error::Backend);
        }
        if self.tx_queue != 0 { return Err(Error::Layout); }
        self.tx.tx_unpublished = 0;
        self.io.write16(self.bar + 0x90, 1).map_err(|_| Error::Backend)
    }

    fn tx_slots_free(&self) -> usize {
        if let Some(tail) = self.tx.no_close_tail {
            return (N - 1).saturating_sub(usize::from(tail.wrapping_sub(self.tx.tx_closed)));
        }
        (0..N)
            .filter(|&slot| {
                self.descriptor(false, slot).map_or(false, |d| unsafe {
                    u32::from_le(d.read_volatile()) & OWN == 0
                })
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::{mac::tests::Model, ring::{Ring, Rx, TsoFrag, TsoRequest}};

    #[repr(align(2048))]
    struct Aligned<const M: usize>([u8; M]);

    #[test]
    fn rss_v3_queues_use_independent_descriptors_buffers_and_rx_cursors() {
        const N: usize = 16;
        const QUEUES: usize = 4;
        const TABLE: usize = queue_descriptor_bytes(N, true, QUEUES);
        const PAYLOADS: usize = (QUEUES + 1) * N * BUFFER;
        let mut table = Aligned([0; TABLE]);
        let mut payloads = Aligned([0; PAYLOADS]);
        let descriptors = DmaMemory {
            cpu: NonNull::new(table.0.as_mut_ptr()).unwrap(), physical: 0x400000, size: TABLE,
        };
        let buffers = DmaMemory {
            cpu: NonNull::new(payloads.0.as_mut_ptr()).unwrap(), physical: 0x404000, size: PAYLOADS,
        };
        let window = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 0x100000 };
        let mut dma = unsafe {
            PackedDma::<_, N, true, QUEUES>::from_verified_memory(Model::default(), 0, descriptors, buffers, &window)
        }.unwrap();
        let (tx, rx, data) = dma.bus_addresses();
        let mut rings = core::array::from_fn::<_, QUEUES, _>(|queue| {
            dma.select_rx_queue(queue).unwrap();
            let (base, payload) = dma.rx_queue_addresses(queue).unwrap();
            assert_eq!(base, rx + (queue * 768) as u64);
            assert_eq!(payload, data + ((queue + 1) * N * BUFFER) as u64);
            assert_eq!(dma.bus_addresses().0, tx);
            let mut ring = Ring::<N>::new(data, PAYLOADS as u64).unwrap()
                .with_private_rx(payload).unwrap();
            if queue == 0 { ring.initialize(&mut dma).unwrap(); }
            else { ring.initialize_rx(&mut dma).unwrap(); }
            assert_eq!(dma.descriptor_address(true, 0), Ok(payload));
            let last = dma.descriptor(true, N - 1).unwrap();
            assert_eq!(u32::from_le(unsafe { last.add(7).read_volatile() }), OWN | (1 << 30) | 2048);
            ring
        });
        assert_eq!(dma.select_rx_queue(QUEUES), Err(Error::Layout));
        for (queue, outcome) in [(2, Rx::Frame(60)), (0, Rx::Rejected), (3, Rx::Frame(60))] {
            dma.select_rx_queue(queue).unwrap();
            assert!(!rings[queue].rx_ready(&mut dma).unwrap());
            let desc = dma.descriptor(true, 0).unwrap();
            let status = (3u32 << 24) | 64 | if queue == 0 { 1 << 17 } else { 0 };
            let options = (1u32 << 30) | (1 << 28) | if queue == 3 { 1 << 24 } else { 0 };
            unsafe {
                desc.add(6).write_volatile(options.to_le());
                desc.add(7).write_volatile(status.to_le());
                dma.buffers.cpu.as_ptr().add((queue + 1) * N * BUFFER).write(queue as u8);
            }
            let mut frame = [0; 60];
            assert!(rings[queue].rx_ready(&mut dma).unwrap());
            assert_eq!(rings[queue].receive(&mut dma, &mut frame, 0xf << 19), Ok(outcome));
            assert_eq!(frame[0], queue as u8);
            assert_eq!(rings[queue].rx_transport_checksum(), if queue == 2 { 6 } else { 0 });
            assert!(!rings[queue].rx_ready(&mut dma).unwrap());
            assert_eq!(u32::from_le(unsafe { desc.add(7).read_volatile() }), OWN | 2048);
        }
        dma.select_rx_queue(1).unwrap();
        assert!(!rings[1].rx_ready(&mut dma).unwrap());
        let external = 0x800f0000;
        let desc = dma.descriptor(true, 0).unwrap();
        unsafe { desc.add(7).write_volatile(0); }
        rings[1].arm_rx_external(&mut dma, 0, Some(external)).unwrap();
        assert_eq!(dma.descriptor_address(true, 0), Ok(external));
        unsafe { desc.add(7).write_volatile((3u32 << 24 | 64).to_le()); }
        assert_eq!(rings[1].receive_external(&mut dma, 0xf << 19), Ok(Rx::External(0, external, 60)));
        rings[1].arm_rx_external(&mut dma, 0, None).unwrap();
        assert_eq!(dma.descriptor_address(true, 0), Ok(data + (2 * N * BUFFER) as u64));
        dma.enable_no_close().unwrap();
        rings[0].send(&mut dma, &[0; 60]).unwrap();
        dma.io.tx_close = dma.io.tx_tail;
        assert_eq!(dma.reclaim_tx(), Ok(1));
    }

    #[test]
    fn b51_tx_queues_use_separate_doorbells_and_completion_cursors() {
        const N: usize = 16;
        const TABLE: usize = tx_rx_descriptor_bytes(N, true, 1, 2);
        const PAYLOADS: usize = 3 * N * BUFFER;
        let mut table = Aligned([0; TABLE]);
        let mut payloads = Aligned([0; PAYLOADS]);
        let descriptors = DmaMemory {
            cpu: NonNull::new(table.0.as_mut_ptr()).unwrap(), physical: 0x400000, size: TABLE,
        };
        let buffers = DmaMemory {
            cpu: NonNull::new(payloads.0.as_mut_ptr()).unwrap(), physical: 0x404000, size: PAYLOADS,
        };
        let window = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 0x100000 };
        let mut dma = unsafe {
            PackedDma::<_, N, true, 1, 2>::from_verified_memory(Model::default(), 0, descriptors, buffers, &window)
        }.unwrap();
        assert_eq!(dma.tx_queue_address(1), Some(0x80000300));
        assert_eq!(dma.rx_queue_addresses(0), Some((0x80000600, 0x80014000)));
        for queue in 0..2 {
            dma.select_tx_queue(queue).unwrap();
            dma.enable_no_close().unwrap();
            let mut ring = Ring::<N>::new(dma.buffer_bus + (queue * N * BUFFER) as u64,
                                         ((3 - queue) * N * BUFFER) as u64).unwrap();
            ring.initialize_tx(&mut dma).unwrap();
            for _ in 0..=queue {
                ring.send(&mut dma, &[queue as u8; 60]).unwrap();
            }
            assert_eq!(dma.tx_inflight(), queue + 1);
        }
        assert_eq!((dma.io.tx_tail, dma.io.tx_tail_q1), (1, 2));
        assert_eq!(payloads.0[N * BUFFER], 1);
        dma.io.tx_close_q1 = 2;
        assert_eq!(dma.reclaim_tx(), Ok(2));
        dma.select_tx_queue(0).unwrap();
        assert_eq!(dma.reclaim_tx(), Ok(0));
        assert_eq!(dma.tx_inflight(), 1);
        dma.io.tx_close = 1;
        assert_eq!(dma.reclaim_tx(), Ok(1));
        assert_eq!(dma.select_tx_queue(2), Err(Error::Layout));
    }

    #[test]
    fn wide_b51_rings_wrap_without_overlapping_the_shared_pool() {
        fn run<const N: usize>() {
            const REGION: usize = 0x300000;
            let mut data = std::vec![0; REGION + 2047];
            let cpu = data.as_mut_ptr();
            let aligned = unsafe { cpu.add(cpu.align_offset(2048)) };
            let descriptor_size = descriptor_bytes(N, true);
            let payload_offset = descriptor_size.next_multiple_of(4096);
            for (window, private, shared_bus) in [
                (InboundWindow { physical: 0x24200000, bus: 0x80000000, size: 0x400000 },
                 0x24200000, 0x80300000),
                (InboundWindow { physical: 0x24500000, bus: 0x81000000, size: 0x400000 },
                 0x24600000, 0x81000000),
            ] {
                let descriptors = DmaMemory {
                    cpu: NonNull::new(aligned).unwrap(), physical: private, size: descriptor_size,
                };
                let buffers = DmaMemory {
                    cpu: NonNull::new(unsafe { aligned.add(payload_offset) }).unwrap(),
                    physical: private + payload_offset as u64, size: N * BUFFER * 2,
                };
                let mut dma = unsafe {
                    PackedDma::<_, N, true>::from_verified_memory(Model::default(), 0, descriptors, buffers, &window)
                }.unwrap();
                let (_, _, payload_bus) = dma.bus_addresses();
                assert_eq!(dma.translate_external(0x24500000, BUFFER as u64), Ok(shared_bus));
                let other_private = if private == 0x24200000 { 0x24600000 } else { 0x24200000 };
                assert_eq!(dma.translate_external(other_private, BUFFER as u64), Err(Error::Layout));
                let mut ring = Ring::<N>::new(payload_bus, (N * BUFFER * 2) as u64).unwrap();
                unsafe { dma.clear_after_stop() };
                ring.initialize(&mut dma).unwrap();
                dma.enable_no_close().unwrap();
                for sequence in 0..=N {
                    let slot = sequence % N;
                    let end = if slot == N - 1 { 1 << 30 } else { 0 };
                    let rx = dma.descriptor(true, slot).unwrap();
                    unsafe {
                        assert_eq!(u32::from_le(rx.read_volatile()) & ((1 << 30) | OWN), end | OWN);
                        rx.write_volatile((end | (3 << 28) | 64).to_le());
                    }
                    assert_eq!(ring.receive(&mut dma, &mut [0; 1514], 1 << 21), Ok(Rx::Frame(60)));
                    ring.send(&mut dma, &[0; 60]).unwrap();
                    assert_eq!(dma.tx_descriptor_words(slot).unwrap()[0] & (1 << 30), end);
                    dma.io.tx_close = dma.io.tx_tail;
                    assert_eq!(dma.reclaim_tx(), Ok(1));
                }
            }
        }
        run::<256>();
        run::<512>();
    }

    #[test]
    fn ipv4_checksum_request_zeroes_only_private_header_and_sets_opts2() {
        const N:usize=16;
        const BYTES:usize=PackedDma::<Model,N,true>::DESCRIPTOR_BYTES;
        let mut desc=Aligned([0;BYTES]);
        let mut data=Aligned([0;N*BUFFER*2]);
        let window=InboundWindow{physical:0x400000,bus:0x80000000,size:0x20000};
        let desc=DmaMemory{cpu:NonNull::new(desc.0.as_mut_ptr()).unwrap(),physical:0x400000,size:BYTES};
        let buffers=DmaMemory{cpu:NonNull::new(data.0.as_mut_ptr()).unwrap(),physical:0x402000,size:N*BUFFER*2};
        let mut dma=unsafe {PackedDma::<_,N,true>::from_verified_memory(Model::default(),0,desc,buffers,&window)}.unwrap();
        let (_,_,bus)=dma.bus_addresses();
        let mut ring=Ring::<N>::new(bus,(N*BUFFER*2) as u64).unwrap();
        ring.initialize(&mut dma).unwrap();
        dma.enable_no_close().unwrap();
        let mut frame=[0x5a;60];
        frame[12..15].copy_from_slice(&[8,0,0x45]);
        ring.send_ipv4_checksum(&mut dma,&frame).unwrap();
        assert_eq!(dma.tx_descriptor_words(0).unwrap()[1], crate::ring::TX_IPV4_CHECKSUM);
        assert_eq!(&data.0[23..27],&[0x5a,0,0,0x5a]);
        assert_eq!(&frame[23..27],&[0x5a;4]);
        ring.send(&mut dma,&frame).unwrap();
        assert_eq!(dma.tx_descriptor_words(1).unwrap()[1],0);
        assert_eq!(&data.0[BUFFER+24..BUFFER+26],&[0x5a;2]);
        frame[16..18].copy_from_slice(&28u16.to_be_bytes());
        frame[23]=17;
        frame[26..34].fill(0);
        ring.send_checksum(&mut dma,&frame,crate::ring::TX_UDP_CHECKSUM).unwrap();
        assert_eq!(dma.tx_descriptor_words(2).unwrap()[1], crate::ring::TX_UDP_CHECKSUM);
        assert_eq!(&data.0[2*BUFFER+40..2*BUFFER+42],&25u16.to_be_bytes());
        for (slot, flags, expected) in [(0,1<<17,6),(1,1<<18,17),(2,(1<<18)|(1<<15),0),
                                        (3,(1<<17)|(1<<16),0),(4,3<<17,0)] {
            let d=dma.descriptor(true,slot).unwrap();
            unsafe { d.write_volatile((3<<28) | 64 | flags) };
            assert_eq!(ring.receive(&mut dma,&mut [0;1514],1<<21),Ok(Rx::Frame(60)));
            assert_eq!(ring.rx_transport_checksum(),expected);
        }
    }

    #[test]
    fn packed_ring_uses_distinct_words_and_device_bus_addresses() {
        const N: usize = 16;
        let mut desc = Aligned([0; 512]);
        let mut data = Aligned([0; N * BUFFER * 2]);
        let descriptors = DmaMemory {
            cpu: NonNull::new(desc.0.as_mut_ptr()).unwrap(), physical: 0x400000, size: 512,
        };
        let buffers = DmaMemory {
            cpu: NonNull::new(data.0.as_mut_ptr()).unwrap(), physical: 0x402000, size: data.0.len(),
        };
        let inbound = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 0x20000 };
        let mut backend = unsafe {
            PackedDma::<_, N>::from_verified_memory(Model::default(), 0, descriptors, buffers, &inbound)
        }.unwrap();
        let (tx, rx, payload_bus) = backend.bus_addresses();
        assert_eq!((tx,rx,payload_bus), (0x80000000,0x80000100,0x80002000));
        assert_eq!(backend.publish(false, 0, Descriptor {
            flags: OWN | (3 << 28) | 2049, options: 0, address: payload_bus,
        }), Err(Error::Layout));
        assert_eq!(backend.status(false, 0), Ok(0));
        let mut ring = Ring::<N>::new(payload_bus, (N * BUFFER * 2) as u64).unwrap();
        ring.initialize(&mut backend).unwrap();
        assert_eq!(backend.status(true, 0), Ok(OWN | 2048));
        for _ in 0..2 {
            for slot in 0..N {
                ring.send(&mut backend, &[0x8a; 14]).unwrap();
                let descriptor = backend.descriptor(false, slot).unwrap();
                assert_ne!(unsafe { descriptor.read_volatile() } & OWN, 0);
                let adjacent = (slot + 1) % N;
                assert_eq!(backend.status(false, adjacent).unwrap() & OWN, 0);
                let buffer = backend.buffers.cpu.as_ptr().wrapping_add(slot * BUFFER);
                assert_eq!(unsafe { buffer.read() }, 0x8a);
                assert_eq!(unsafe { buffer.add(14).read() }, 0);
                unsafe { descriptor.write_volatile(0); }
            }
        }
        assert_eq!(backend.io.doorbells, 32);
        let slot0 = backend.descriptor(true, 0).unwrap();
        unsafe { slot0.write_volatile(0x30000000 | 64); }
        assert_eq!(ring.receive(&mut backend, &mut [0;1514], 1 << 21), Ok(Rx::Frame(60)));
        assert_eq!(backend.status(true, 0), Ok(OWN | 2048));
        // The simulated device is stopped and no transactions remain.
        unsafe { backend.clear_after_stop(); }
        ring.initialize(&mut backend).unwrap();
        assert_eq!(backend.status(true, 0), Ok(OWN | 2048));

        backend.enable_no_close().unwrap();
        for turn in 0..=4096 {
            let previous_tail = backend.io.tx_tail;
            backend.begin_tx_batch();
            for slot in 0..N - 1 {
                ring.send(&mut backend, &[slot as u8; 60]).unwrap();
            }
            assert_eq!(backend.tx_inflight(), N - 1);
            assert_eq!(ring.send(&mut backend, &[0xaa; 60]), Err(Error::Busy));
            assert_eq!(backend.reclaim_tx(), Ok(0));
            assert_eq!(backend.io.tx_tail, previous_tail);
            backend.end_tx_batch().unwrap();
            assert_eq!(backend.io.tx_tail, previous_tail.wrapping_add((N - 1) as u16));
            let oldest = backend.oldest_tx_slot();
            let payload = backend.buffers.cpu.as_ptr().wrapping_add(oldest * BUFFER);
            assert_eq!(unsafe { payload.read() }, 0);
            assert!(backend.write_padded_payload(oldest, &[0xbb; 60], 60).is_err());
            assert_eq!(unsafe { payload.read() }, 0);
            backend.io.tx_close = backend.tx.tx_closed.wrapping_add(2);
            assert_eq!(backend.reclaim_tx(), Ok(2));
            assert!(!backend.tx_slot_pending(oldest));
            assert!(backend.tx_slot_pending((oldest + 2) % N));
            ring.send(&mut backend, &[0xcc; 60]).unwrap();
            backend.io.tx_close = backend.io.tx_tail;
            assert_eq!(backend.reclaim_tx(), Ok(N - 2));
            assert_eq!(backend.tx_inflight(), 0);
            if turn == 4095 { assert_eq!(backend.io.tx_tail, 0); }
        }
        ring.send(&mut backend, &[0x99; 14]).unwrap();
        let first = backend.oldest_tx_slot();
        assert_eq!(backend.status(false, first).unwrap() & OWN, OWN);
        backend.io.tx_close = backend.io.tx_tail.wrapping_add(1);
        assert_eq!(backend.reclaim_tx(), Err(Error::Backend));
        assert_eq!(backend.tx_inflight(), 1);
        assert_eq!(backend.enable_no_close(), Err(Error::Busy));
        unsafe { backend.clear_after_stop(); }
        backend.io.tx_tail = 0;
        backend.io.tx_close = 0;
        ring.initialize(&mut backend).unwrap();
        backend.enable_no_close().unwrap();
        ring.send(&mut backend, &[0x99; 14]).unwrap();
        assert_eq!(backend.io.tx_tail, 1);
        assert_eq!(backend.io.doorbells, 32);
    }

    #[test]
    fn inbound_translation_rejects_crossing_or_overflowing_ranges() {
        let inbound = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 4096 };
        assert_eq!(inbound.translate(0x400ff0,16), Some(0x80000ff0));
        assert_eq!(inbound.translate(0x400ff0,17), None);
        assert_eq!(inbound.translate(0x3fffff,1), None);
        assert_eq!(InboundWindow { bus: u64::MAX, ..inbound }.translate(0x400000, 2), None);
        assert_eq!(InboundWindow { bus: u64::MAX, ..inbound }.translate(0x400000, 1), None);
        assert_eq!(InboundWindow { physical: u64::MAX - 4094, ..inbound }
            .translate(u64::MAX - 4094, 1), None);
        assert_eq!(InboundWindow { physical: u64::MAX - 4095, ..inbound }
            .translate(u64::MAX, 1), Some(0x80000fff));
    }

    #[test]
    fn send_external_publishes_external_address_and_length_bounds() {
        const N: usize = 16;
        const BYTES: usize = PackedDma::<Model, N, true>::DESCRIPTOR_BYTES;
        let mut desc = Aligned([0; BYTES]);
        let mut data = Aligned([0; N * BUFFER * 2]);
        let window = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 0x20000 };
        let descriptors = DmaMemory {
            cpu: NonNull::new(desc.0.as_mut_ptr()).unwrap(), physical: 0x400000, size: BYTES,
        };
        let buffers = DmaMemory {
            cpu: NonNull::new(data.0.as_mut_ptr()).unwrap(), physical: 0x402000, size: N * BUFFER * 2,
        };
        let mut dma = unsafe {
            PackedDma::<_, N, true>::from_verified_memory(Model::default(), 0, descriptors, buffers, &window)
        }.unwrap();
        let (_, _, payload_bus) = dma.bus_addresses();
        let mut ring = Ring::<N>::new(payload_bus, (N * BUFFER * 2) as u64).unwrap();
        ring.initialize(&mut dma).unwrap();
        dma.enable_no_close().unwrap();
        // Both halves of the external address land in the descriptor.
        let bus = 0x8123_4567_89AB_C800u64;
        assert_eq!(ring.send_external(&mut dma, bus, 14, 0), Ok(()));
        let words = dma.tx_descriptor_words(0).unwrap();
        assert_eq!(words[0] & 0xffff, 60);
        assert_eq!(words[0] & (3 << 28), 3 << 28);
        assert_eq!(words[0] & (1 << 31), 1 << 31);
        assert_eq!(words[0] & (1 << 30), 0);
        assert_eq!(words[1], 0);
        assert_eq!((u64::from(words[3]) << 32) | u64::from(words[2]), bus);
        assert_eq!(ring.tx_external_address(&mut dma, 0), Ok(bus));
        // The copy path's length bounds apply; a full 1514-byte frame pads to
        // itself and the ring stops on a Layout fault only.
        assert_eq!(ring.send_external(&mut dma, bus, 1514, 0), Ok(()));
        assert_eq!(dma.tx_descriptor_words(1).unwrap()[0] & 0xffff, 1514);
        assert_eq!(ring.send_external(&mut dma, bus, 13, 0), Err(Error::Length));
        assert_eq!(ring.send_external(&mut dma, bus, 1515, 0), Err(Error::Length));
        assert_eq!(ring.send_external(&mut dma, bus, 60, crate::ring::TX_UDP_CHECKSUM), Err(Error::Layout));
        assert_eq!(ring.send_external(&mut dma, bus, 60, 0), Err(Error::Stopped));
    }

    #[test]
    fn send_external_rejects_out_of_no_close_order() {
        const N: usize = 16;
        const BYTES: usize = PackedDma::<Model, N, true>::DESCRIPTOR_BYTES;
        let mut desc = Aligned([0; BYTES]);
        let mut data = Aligned([0; N * BUFFER * 2]);
        let window = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 0x20000 };
        let descriptors = DmaMemory {
            cpu: NonNull::new(desc.0.as_mut_ptr()).unwrap(), physical: 0x400000, size: BYTES,
        };
        let buffers = DmaMemory {
            cpu: NonNull::new(data.0.as_mut_ptr()).unwrap(), physical: 0x402000, size: N * BUFFER * 2,
        };
        let mut dma = unsafe {
            PackedDma::<_, N, true>::from_verified_memory(Model::default(), 0, descriptors, buffers, &window)
        }.unwrap();
        let (_, _, payload_bus) = dma.bus_addresses();
        let mut ring = Ring::<N>::new(payload_bus, (N * BUFFER * 2) as u64).unwrap();
        ring.initialize(&mut dma).unwrap();
        dma.enable_no_close().unwrap();
        let bus = 0x8200_0800u64;
        assert_eq!(ring.send_external(&mut dma, bus, 60, 0), Ok(()));
        // The backend tail advances past the ring cursor and both entries
        // complete: slot 1 is free but is no longer the free tail.
        dma.kick_tx().unwrap();
        dma.io.tx_close = 2;
        assert_eq!(dma.reclaim_tx(), Ok(2));
        assert_eq!(ring.send_external(&mut dma, bus, 60, 0), Err(Error::Layout));
        assert_eq!(ring.send_external(&mut dma, bus, 60, 0), Err(Error::Stopped));
    }

    #[test]
    fn send_external_busy_when_no_close_ring_full() {
        const N: usize = 16;
        const BYTES: usize = PackedDma::<Model, N, true>::DESCRIPTOR_BYTES;
        let mut desc = Aligned([0; BYTES]);
        let mut data = Aligned([0; N * BUFFER * 2]);
        let window = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 0x20000 };
        let descriptors = DmaMemory {
            cpu: NonNull::new(desc.0.as_mut_ptr()).unwrap(), physical: 0x400000, size: BYTES,
        };
        let buffers = DmaMemory {
            cpu: NonNull::new(data.0.as_mut_ptr()).unwrap(), physical: 0x402000, size: N * BUFFER * 2,
        };
        let mut dma = unsafe {
            PackedDma::<_, N, true>::from_verified_memory(Model::default(), 0, descriptors, buffers, &window)
        }.unwrap();
        let (_, _, payload_bus) = dma.bus_addresses();
        let mut ring = Ring::<N>::new(payload_bus, (N * BUFFER * 2) as u64).unwrap();
        ring.initialize(&mut dma).unwrap();
        dma.enable_no_close().unwrap();
        let bus = 0x8200_0800u64;
        for slot in 0..N - 1 {
            ring.send_external(&mut dma, bus + (slot * BUFFER) as u64, 60, 0).unwrap();
        }
        assert_eq!(dma.tx_inflight(), N - 1);
        assert_eq!(ring.send_external(&mut dma, bus, 60, 0), Err(Error::Busy));
        // Busy leaves the ring usable; one reclaimed slot admits a send again.
        dma.io.tx_close = dma.tx.tx_closed.wrapping_add(1);
        assert_eq!(dma.reclaim_tx(), Ok(1));
        assert_eq!(ring.send_external(&mut dma, bus, 60, 0), Ok(()));
        assert_eq!(dma.tx_inflight(), N - 1);
    }

    #[test]
    fn publish_external_validates_address_options_and_ownership() {
        const N: usize = 16;
        const BYTES: usize = PackedDma::<Model, N, true>::DESCRIPTOR_BYTES;
        let mut desc = Aligned([0; BYTES]);
        let mut data = Aligned([0; N * BUFFER * 2]);
        let window = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 0x20000 };
        let descriptors = DmaMemory {
            cpu: NonNull::new(desc.0.as_mut_ptr()).unwrap(), physical: 0x400000, size: BYTES,
        };
        let buffers = DmaMemory {
            cpu: NonNull::new(data.0.as_mut_ptr()).unwrap(), physical: 0x402000, size: N * BUFFER * 2,
        };
        let mut dma = unsafe {
            PackedDma::<_, N, true>::from_verified_memory(Model::default(), 0, descriptors, buffers, &window)
        }.unwrap();
        let (_, _, payload_bus) = dma.bus_addresses();
        let mut ring = Ring::<N>::new(payload_bus, (N * BUFFER * 2) as u64).unwrap();
        ring.initialize(&mut dma).unwrap();
        let external = 0x8200_0800u64;
        let armed = Descriptor { flags: OWN | BUFFER as u32, options: 0, address: external };
        // Every RX slot is device-owned after initialize and address zero is
        // reserved as the private-slot marker.
        assert_eq!(dma.publish_external(true, 0, armed), Err(Error::Layout));
        assert_eq!(ring.arm_rx_external(&mut dma, 0, Some(external)), Err(Error::Layout));
        assert_eq!(ring.arm_rx_external(&mut dma, 0, Some(0)), Err(Error::Layout));
        // The simulated device is stopped and no transactions remain.
        unsafe { dma.clear_after_stop(); }
        // RX descriptors never accept TX checksum options.
        assert_eq!(dma.publish_external(true, 0, Descriptor {
            flags: OWN | BUFFER as u32, options: crate::ring::TX_TCP_CHECKSUM, address: external,
        }), Err(Error::Layout));
        assert_eq!(dma.publish_external(true, 0, Descriptor {
            flags: OWN | BUFFER as u32, options: 0, address: external + 1,
        }), Err(Error::Layout));
        assert_eq!(dma.publish_external(true, 0, Descriptor {
            flags: OWN | BUFFER as u32, options: 0, address: payload_bus + BUFFER as u64,
        }), Err(Error::Layout));
        assert_eq!(dma.publish_external(true, 0, armed), Ok(()));
        assert_eq!(dma.descriptor_address(true, 0), Ok(external));
        assert_eq!(&data.0[..BUFFER], &[0; BUFFER][..]);
        // TX publishes keep the copy path's flags shape.
        assert_eq!(dma.publish_external(false, 0, Descriptor {
            flags: OWN | (3 << 28) | 2049, options: 0, address: external,
        }), Err(Error::Layout));
        assert_eq!(dma.publish_external(false, 0, Descriptor {
            flags: OWN | (3 << 28) | 60, options: 0, address: external,
        }), Ok(()));
        assert_eq!(dma.tx_descriptor_words(0).unwrap()[0] & 0xffff, 60);
        unsafe { dma.clear_after_stop(); }
        ring.initialize(&mut dma).unwrap();
        let tcp = crate::ring::TX_TCP_CHECKSUM;
        assert_eq!(dma.publish_external(false, 0, Descriptor {
            flags: OWN | (3 << 28) | 60, options: tcp, address: external,
        }), Err(Error::Layout));
        dma.enable_no_close().unwrap();
        ring.send_external_checksum(&mut dma, external, 60, tcp).unwrap();
        assert_eq!(dma.tx_descriptor_words(0).unwrap()[1], tcp);
        assert_eq!(&data.0[..BUFFER], &[0; BUFFER]);
    }

    #[test]
    fn descriptor_address_reads_back_both_address_halves() {
        const N: usize = 16;
        const BYTES: usize = PackedDma::<Model, N>::DESCRIPTOR_BYTES;
        let mut desc = Aligned([0; BYTES]);
        let mut data = Aligned([0; N * BUFFER * 2]);
        let window = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 0x20000 };
        let descriptors = DmaMemory {
            cpu: NonNull::new(desc.0.as_mut_ptr()).unwrap(), physical: 0x400000, size: BYTES,
        };
        let buffers = DmaMemory {
            cpu: NonNull::new(data.0.as_mut_ptr()).unwrap(), physical: 0x402000, size: N * BUFFER * 2,
        };
        let mut dma = unsafe {
            PackedDma::<_, N>::from_verified_memory(Model::default(), 0, descriptors, buffers, &window)
        }.unwrap();
        let (_, _, payload_bus) = dma.bus_addresses();
        let mut ring = Ring::<N>::new(payload_bus, (N * BUFFER * 2) as u64).unwrap();
        ring.initialize(&mut dma).unwrap();
        let address = 0x8123_4567_89AB_C800u64;
        let d = dma.descriptor(false, 5).unwrap();
        unsafe {
            d.add(2).write_volatile((address as u32).to_le());
            d.add(3).write_volatile(((address >> 32) as u32).to_le());
        }
        assert_eq!(dma.descriptor_address(false, 5), Ok(address));
        assert_eq!(ring.tx_external_address(&mut dma, 5), Ok(address));
        let r = dma.descriptor(true, 5).unwrap();
        unsafe {
            r.add(2).write_volatile(0x00AA_BBCCu32.to_le());
            r.add(3).write_volatile(0xDDu32.to_le());
        }
        assert_eq!(dma.descriptor_address(true, 5), Ok(0x0000_00DD_00AA_BBCC));
    }

    #[test]
    fn translate_external_matches_window_and_private_arena_edges() {
        const N: usize = 16;
        const BYTES: usize = PackedDma::<Model, N>::DESCRIPTOR_BYTES;
        let mut desc = Aligned([0; BYTES]);
        let mut data = Aligned([0; N * BUFFER * 2]);
        let window = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 0x20000 };
        let descriptors = DmaMemory {
            cpu: NonNull::new(desc.0.as_mut_ptr()).unwrap(), physical: 0x400000, size: BYTES,
        };
        let buffers = DmaMemory {
            cpu: NonNull::new(data.0.as_mut_ptr()).unwrap(), physical: 0x402000, size: N * BUFFER * 2,
        };
        let dma = unsafe {
            PackedDma::<_, N>::from_verified_memory(Model::default(), 0, descriptors, buffers, &window)
        }.unwrap();
        let (_, _, payload_bus) = dma.bus_addresses();
        assert_eq!(payload_bus, 0x80002000);
        // In-window offsets translate exactly like InboundWindow::translate.
        for (physical, size) in [(0x400000, 1), (0x412340, 0x1000), (0x41fff0, 16), (0x41f000, 0x1000)] {
            assert_eq!(dma.translate_external(physical, size),
                       window.translate(physical, size).ok_or(Error::Layout));
        }
        // Out-of-window, crossing and empty ranges are refused.
        for (physical, size) in [(0x3fffff, 1), (0x420000, 1), (0x41fff0, 17), (0x400000, 0)] {
            assert_eq!(dma.translate_external(physical, size), Err(Error::Layout));
        }
        // The private arena edges are exact.
        let arena = (N * BUFFER * 2) as u64;
        assert!(dma.is_private_buffer(payload_bus));
        assert!(dma.is_private_buffer(payload_bus + arena - 1));
        assert!(!dma.is_private_buffer(payload_bus + arena));
        assert!(!dma.is_private_buffer(payload_bus - 1));
        assert!(!dma.is_private_buffer(0x8200_0800));
    }

    fn tso_request<'a>(frags: &'a [TsoFrag], head_len: u32, tso_id: u32) -> TsoRequest<'a> {
        let sum: usize = frags.iter().map(|f| f.length).sum();
        TsoRequest {
            ip_version: 4,
            protocol: 6,
            ip_hdr_len: 20,
            tcp_hdr_len: 20,
            mss: 1460,
            flags: 0,
            head_len,
            payload_len: (sum - head_len as usize) as u32,
            tso_id,
            frags,
        }
    }

    #[test]
    fn tso_publishes_exact_opts1_opts2_per_fragment() {
        const N: usize = 16;
        const BYTES: usize = PackedDma::<Model, N, true>::DESCRIPTOR_BYTES;
        let mut desc = Aligned([0; BYTES]);
        let mut data = Aligned([0; N * BUFFER * 2]);
        let window = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 0x20000 };
        let descriptors = DmaMemory {
            cpu: NonNull::new(desc.0.as_mut_ptr()).unwrap(), physical: 0x400000, size: BYTES,
        };
        let buffers = DmaMemory {
            cpu: NonNull::new(data.0.as_mut_ptr()).unwrap(), physical: 0x402000, size: N * BUFFER * 2,
        };
        let mut dma = unsafe {
            PackedDma::<_, N, true>::from_verified_memory(Model::default(), 0, descriptors, buffers, &window)
        }.unwrap();
        let (_, _, payload_bus) = dma.bus_addresses();
        let mut ring = Ring::<N>::new(payload_bus, (N * BUFFER * 2) as u64).unwrap();
        ring.initialize(&mut dma).unwrap();
        dma.enable_no_close().unwrap();
        let opts2 = (1460 & 0x7ff) << 18;
        let two = [
            TsoFrag { bus: 0x8200_0800, length: 1514 },
            TsoFrag { bus: 0x8200_1000, length: 1540 },
        ];
        assert_eq!(ring.send_tso(&mut dma, &tso_request(&two, 54, 0xAAAA_5555)), Ok(0));
        assert_eq!(dma.tx_descriptor_words(0).unwrap(), [
            (1 << 31) | (1 << 29) | (1 << 26) | (34 << 18) | 1514,
            opts2,
            0x8200_0800,
            0,
            0, 0, 0, 0,
        ]);
        assert_eq!(dma.tx_descriptor_words(1).unwrap(), [
            (1 << 31) | (1 << 28) | (1 << 26) | (34 << 18) | 1540,
            opts2,
            0x8200_1000,
            0,
            0, 0, 0, 0,
        ]);
        let three = [
            TsoFrag { bus: 0x8200_1800, length: 1054 },
            TsoFrag { bus: 0x8200_2000, length: 1500 },
            TsoFrag { bus: 0x8200_2800, length: 1500 },
        ];
        assert_eq!(ring.send_tso(&mut dma, &tso_request(&three, 54, 7)), Ok(2));
        assert_eq!(dma.tx_descriptor_words(2).unwrap()[0],
                   (1 << 31) | (1 << 29) | (1 << 26) | (34 << 18) | 1054);
        assert_eq!(dma.tx_descriptor_words(3).unwrap()[0],
                   (1 << 31) | (1 << 26) | (34 << 18) | 1500);
        assert_eq!(dma.tx_descriptor_words(4).unwrap()[0],
                   (1 << 31) | (1 << 28) | (1 << 26) | (34 << 18) | 1500);
        assert_eq!(dma.tx_descriptor_words(3).unwrap()[1], opts2);
    }

    #[test]
    fn tso_ipv6_encodes_giant_send_v6_with_gttcpho_54() {
        const N: usize = 16;
        const BYTES: usize = PackedDma::<Model, N, true>::DESCRIPTOR_BYTES;
        let mut desc = Aligned([0; BYTES]);
        let mut data = Aligned([0; N * BUFFER * 2]);
        let window = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 0x20000 };
        let descriptors = DmaMemory {
            cpu: NonNull::new(desc.0.as_mut_ptr()).unwrap(), physical: 0x400000, size: BYTES,
        };
        let buffers = DmaMemory {
            cpu: NonNull::new(data.0.as_mut_ptr()).unwrap(), physical: 0x402000, size: N * BUFFER * 2,
        };
        let mut dma = unsafe {
            PackedDma::<_, N, true>::from_verified_memory(Model::default(), 0, descriptors, buffers, &window)
        }.unwrap();
        let (_, _, payload_bus) = dma.bus_addresses();
        let mut ring = Ring::<N>::new(payload_bus, (N * BUFFER * 2) as u64).unwrap();
        ring.initialize(&mut dma).unwrap();
        dma.enable_no_close().unwrap();
        let v6_frags = [
            TsoFrag { bus: 0x8200_0800, length: 1514 },
            TsoFrag { bus: 0x8200_1000, length: 560 },
        ];
        let v6 = TsoRequest {
            ip_version: 6,
            ip_hdr_len: 40,
            head_len: 74,
            ..tso_request(&v6_frags, 74, 0x6666_6666)
        };
        assert_eq!(ring.send_tso(&mut dma, &v6), Ok(0));
        let first = dma.tx_descriptor_words(0).unwrap();
        assert_eq!(first[0], (1 << 31) | (1 << 29) | (1 << 25) | (54 << 18) | 1514);
        assert_eq!(first[0] & (1 << 26), 0);
        assert_eq!((first[0] >> 18) & 0x7f, 54);
        assert_eq!(first[1], (1460 & 0x7ff) << 18);
        let last = dma.tx_descriptor_words(1).unwrap();
        assert_eq!(last[0], (1 << 31) | (1 << 28) | (1 << 25) | (54 << 18) | 560);
        assert_eq!(last[0] & (1 << 26), 0);
    }

    #[test]
    fn kick_tx_count_advances_tail_by_fragment_count() {
        {
            const N: usize = 16;
            const BYTES: usize = PackedDma::<Model, N, true>::DESCRIPTOR_BYTES;
            let mut desc = Aligned([0; BYTES]);
            let mut data = Aligned([0; N * BUFFER * 2]);
            let window = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 0x20000 };
            let descriptors = DmaMemory {
                cpu: NonNull::new(desc.0.as_mut_ptr()).unwrap(), physical: 0x400000, size: BYTES,
            };
            let buffers = DmaMemory {
                cpu: NonNull::new(data.0.as_mut_ptr()).unwrap(), physical: 0x402000, size: N * BUFFER * 2,
            };
            let mut dma = unsafe {
                PackedDma::<_, N, true>::from_verified_memory(Model::default(), 0, descriptors, buffers, &window)
            }.unwrap();
            let (_, _, payload_bus) = dma.bus_addresses();
            let mut ring = Ring::<N>::new(payload_bus, (N * BUFFER * 2) as u64).unwrap();
            ring.initialize(&mut dma).unwrap();
            dma.enable_no_close().unwrap();
            let two = [
                TsoFrag { bus: 0x8200_0800, length: 1514 },
                TsoFrag { bus: 0x8200_1000, length: 1540 },
            ];
            let three = [
                TsoFrag { bus: 0x8200_1800, length: 1054 },
                TsoFrag { bus: 0x8200_2000, length: 1500 },
                TsoFrag { bus: 0x8200_2800, length: 1500 },
            ];
            assert_eq!(ring.send_tso(&mut dma, &tso_request(&two, 54, 1)), Ok(0));
            assert_eq!(dma.io.tx_tail, 2);
            assert_eq!(dma.io.doorbells, 0);
            assert_eq!(ring.send_tso(&mut dma, &tso_request(&three, 54, 2)), Ok(2));
            assert_eq!(dma.io.tx_tail, 5);
            assert_eq!(dma.io.doorbells, 0);
        }
        {
            const N: usize = 16;
            const BYTES: usize = PackedDma::<Model, N, true>::DESCRIPTOR_BYTES;
            let mut desc = Aligned([0; BYTES]);
            let mut data = Aligned([0; N * BUFFER * 2]);
            let window = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 0x20000 };
            let descriptors = DmaMemory {
                cpu: NonNull::new(desc.0.as_mut_ptr()).unwrap(), physical: 0x400000, size: BYTES,
            };
            let buffers = DmaMemory {
                cpu: NonNull::new(data.0.as_mut_ptr()).unwrap(), physical: 0x402000, size: N * BUFFER * 2,
            };
            let mut dma = unsafe {
                PackedDma::<_, N, true>::from_verified_memory(Model::default(), 0, descriptors, buffers, &window)
            }.unwrap();
            let (_, _, payload_bus) = dma.bus_addresses();
            let mut ring = Ring::<N>::new(payload_bus, (N * BUFFER * 2) as u64).unwrap();
            ring.initialize(&mut dma).unwrap();
            let two = [
                TsoFrag { bus: 0x8200_0800, length: 1514 },
                TsoFrag { bus: 0x8200_1000, length: 1540 },
            ];
            // Classic mode: one 0x90 doorbell for the whole request.
            assert_eq!(ring.send_tso(&mut dma, &tso_request(&two, 54, 3)), Ok(0));
            assert_eq!(dma.io.doorbells, 1);
            assert_eq!(dma.io.tx_tail, 0);
        }
    }

    #[test]
    fn tso_saturates_no_close_ring_at_63_outstanding() {
        const N: usize = 64;
        const BYTES: usize = PackedDma::<Model, N, true>::DESCRIPTOR_BYTES;
        let mut desc = Aligned([0; BYTES]);
        let mut data = Aligned([0; N * BUFFER * 2]);
        let window = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 0x80000 };
        let descriptors = DmaMemory {
            cpu: NonNull::new(desc.0.as_mut_ptr()).unwrap(), physical: 0x400000, size: BYTES,
        };
        let buffers = DmaMemory {
            cpu: NonNull::new(data.0.as_mut_ptr()).unwrap(), physical: 0x402000, size: N * BUFFER * 2,
        };
        let mut dma = unsafe {
            PackedDma::<_, N, true>::from_verified_memory(Model::default(), 0, descriptors, buffers, &window)
        }.unwrap();
        let (_, _, payload_bus) = dma.bus_addresses();
        let mut ring = Ring::<N>::new(payload_bus, (N * BUFFER * 2) as u64).unwrap();
        ring.initialize(&mut dma).unwrap();
        dma.enable_no_close().unwrap();
        for slot in 0..N - 1 {
            ring.send_external(&mut dma, 0x8200_0800 + (slot * BUFFER) as u64, 60, 0).unwrap();
        }
        assert_eq!(dma.tx_inflight(), N - 1);
        assert_eq!(dma.tx_slots_free(), 0);
        let one = [TsoFrag { bus: 0x8200_8000, length: 3054 }];
        assert_eq!(ring.send_tso(&mut dma, &tso_request(&one, 54, 5)), Err(Error::Busy));
        assert_eq!(dma.io.tx_tail, (N - 1) as u16);
        assert_eq!(dma.tx_descriptor_words(N - 1).unwrap()[0], 1 << 30);
        dma.io.tx_close = 3;
        assert_eq!(dma.reclaim_tx(), Ok(3));
        assert_eq!(dma.tx_slots_free(), 3);
        let three = [
            TsoFrag { bus: 0x8200_8800, length: 1054 },
            TsoFrag { bus: 0x8200_9000, length: 1500 },
            TsoFrag { bus: 0x8200_9800, length: 1500 },
        ];
        assert_eq!(ring.send_tso(&mut dma, &tso_request(&three, 54, 6)), Ok(N - 1));
        assert_eq!(dma.io.tx_tail, (N + 2) as u16);
        assert_eq!(dma.tx_slots_free(), 0);
        assert_eq!(ring.send_tso(&mut dma, &tso_request(&one, 54, 7)), Err(Error::Busy));
    }

    #[test]
    fn tso_request_spans_ring_wrap_with_end_on_last_slot() {
        const N: usize = 16;
        const BYTES: usize = PackedDma::<Model, N, true>::DESCRIPTOR_BYTES;
        let mut desc = Aligned([0; BYTES]);
        let mut data = Aligned([0; N * BUFFER * 2]);
        let window = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 0x20000 };
        let descriptors = DmaMemory {
            cpu: NonNull::new(desc.0.as_mut_ptr()).unwrap(), physical: 0x400000, size: BYTES,
        };
        let buffers = DmaMemory {
            cpu: NonNull::new(data.0.as_mut_ptr()).unwrap(), physical: 0x402000, size: N * BUFFER * 2,
        };
        let mut dma = unsafe {
            PackedDma::<_, N, true>::from_verified_memory(Model::default(), 0, descriptors, buffers, &window)
        }.unwrap();
        let (_, _, payload_bus) = dma.bus_addresses();
        let mut ring = Ring::<N>::new(payload_bus, (N * BUFFER * 2) as u64).unwrap();
        ring.initialize(&mut dma).unwrap();
        dma.enable_no_close().unwrap();
        for slot in 0..N - 1 {
            ring.send_external(&mut dma, 0x8200_0800 + (slot * BUFFER) as u64, 60, 0).unwrap();
        }
        dma.io.tx_close = 2;
        assert_eq!(dma.reclaim_tx(), Ok(2));
        let two = [
            TsoFrag { bus: 0x8200_2000, length: 1514 },
            TsoFrag { bus: 0x8200_2800, length: 1540 },
        ];
        assert_eq!(ring.send_tso(&mut dma, &tso_request(&two, 54, 9)), Ok(N - 1));
        assert_eq!(dma.tx_descriptor_words(N - 1).unwrap()[0],
                   (1 << 31) | (1 << 29) | (1 << 30) | (1 << 26) | (34 << 18) | 1514);
        assert_eq!(dma.tx_descriptor_words(0).unwrap()[0],
                   (1 << 31) | (1 << 28) | (1 << 26) | (34 << 18) | 1540);
        assert_eq!(dma.io.tx_tail, (N + 1) as u16);
    }

    #[test]
    fn retired_tso_reports_only_once_the_window_covers_the_last_slot() {
        const N: usize = 16;
        const BYTES: usize = PackedDma::<Model, N, true>::DESCRIPTOR_BYTES;
        let mut desc = Aligned([0; BYTES]);
        let mut data = Aligned([0; N * BUFFER * 2]);
        let window = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 0x20000 };
        let descriptors = DmaMemory {
            cpu: NonNull::new(desc.0.as_mut_ptr()).unwrap(), physical: 0x400000, size: BYTES,
        };
        let buffers = DmaMemory {
            cpu: NonNull::new(data.0.as_mut_ptr()).unwrap(), physical: 0x402000, size: N * BUFFER * 2,
        };
        let mut dma = unsafe {
            PackedDma::<_, N, true>::from_verified_memory(Model::default(), 0, descriptors, buffers, &window)
        }.unwrap();
        let (_, _, payload_bus) = dma.bus_addresses();
        let mut ring = Ring::<N>::new(payload_bus, (N * BUFFER * 2) as u64).unwrap();
        ring.initialize(&mut dma).unwrap();
        dma.enable_no_close().unwrap();
        let three = [
            TsoFrag { bus: 0x8200_0800, length: 1054 },
            TsoFrag { bus: 0x8200_1000, length: 1500 },
            TsoFrag { bus: 0x8200_1800, length: 1500 },
        ];
        assert_eq!(ring.send_tso(&mut dma, &tso_request(&three, 54, 77)), Ok(0));
        ring.send_external(&mut dma, 0x8200_3000, 60, 0).unwrap();
        let mut out = [0u32; 4];
        // The first two descriptors retire but the request stays silent.
        let from = dma.oldest_tx_slot();
        dma.io.tx_close = 2;
        assert_eq!(dma.reclaim_tx(), Ok(2));
        assert_eq!(ring.retired_tso(from, 2, &mut out), 0);
        // The final descriptor retires: the id is reported exactly once.
        let from = dma.oldest_tx_slot();
        dma.io.tx_close = 3;
        assert_eq!(dma.reclaim_tx(), Ok(1));
        assert_eq!(ring.retired_tso(from, 1, &mut out), 1);
        assert_eq!(out[0], 77);
        assert_eq!(ring.retired_tso(from, 1, &mut out), 0);
        // An ordinary frame completion reports nothing.
        let from = dma.oldest_tx_slot();
        dma.io.tx_close = 4;
        assert_eq!(dma.reclaim_tx(), Ok(1));
        assert_eq!(ring.retired_tso(from, 1, &mut out), 0);
        // Two requests in one window report in slot order.
        let one = [TsoFrag { bus: 0x8200_3800, length: 3054 }];
        assert_eq!(ring.send_tso(&mut dma, &tso_request(&one, 54, 11)), Ok(4));
        assert_eq!(ring.send_tso(&mut dma, &tso_request(&one, 54, 22)), Ok(5));
        let from = dma.oldest_tx_slot();
        dma.io.tx_close = 6;
        assert_eq!(dma.reclaim_tx(), Ok(2));
        assert_eq!(ring.retired_tso(from, 2, &mut out), 2);
        assert_eq!(&out[..2], &[11, 22]);
    }

    #[test]
    fn tso_and_external_frames_interleave_and_reclaim() {
        const N: usize = 16;
        const BYTES: usize = PackedDma::<Model, N, true>::DESCRIPTOR_BYTES;
        let mut desc = Aligned([0; BYTES]);
        let mut data = Aligned([0; N * BUFFER * 2]);
        let window = InboundWindow { physical: 0x400000, bus: 0x80000000, size: 0x20000 };
        let descriptors = DmaMemory {
            cpu: NonNull::new(desc.0.as_mut_ptr()).unwrap(), physical: 0x400000, size: BYTES,
        };
        let buffers = DmaMemory {
            cpu: NonNull::new(data.0.as_mut_ptr()).unwrap(), physical: 0x402000, size: N * BUFFER * 2,
        };
        let mut dma = unsafe {
            PackedDma::<_, N, true>::from_verified_memory(Model::default(), 0, descriptors, buffers, &window)
        }.unwrap();
        let (_, _, payload_bus) = dma.bus_addresses();
        let mut ring = Ring::<N>::new(payload_bus, (N * BUFFER * 2) as u64).unwrap();
        ring.initialize(&mut dma).unwrap();
        dma.enable_no_close().unwrap();
        ring.send_external(&mut dma, 0x8200_0800, 60, 0).unwrap();
        let two = [
            TsoFrag { bus: 0x8200_2000, length: 1514 },
            TsoFrag { bus: 0x8200_2800, length: 1540 },
        ];
        assert_eq!(ring.send_tso(&mut dma, &tso_request(&two, 54, 42)), Ok(1));
        ring.send_external(&mut dma, 0x8200_3000, 60, 0).unwrap();
        assert_eq!(dma.io.tx_tail, 4);
        let mut out = [0u32; 4];
        let from = dma.oldest_tx_slot();
        dma.io.tx_close = 4;
        assert_eq!(dma.reclaim_tx(), Ok(4));
        assert_eq!(ring.retired_tso(from, 4, &mut out), 1);
        assert_eq!(out[0], 42);
        assert_eq!(dma.tx_inflight(), 0);
    }
}
