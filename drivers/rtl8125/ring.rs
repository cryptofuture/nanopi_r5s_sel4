//! Type-1 RTL8125 descriptors, as used by the pinned r8169 methods 48–51.
//! The device fetches packed 16-byte descriptors. The descriptor mapping must
//! be DMA-coherent or Normal non-cacheable, with no cached aliases.

pub const TX_IPV4_CHECKSUM: u32 = 1 << 29;
pub const TX_TCP_CHECKSUM: u32 = TX_IPV4_CHECKSUM | 1 << 30;
pub const TX_UDP_CHECKSUM: u32 = TX_IPV4_CHECKSUM | 1 << 31;
const OWN: u32 = 1 << 31;
const END: u32 = 1 << 30;
const FIRST: u32 = 1 << 29;
const LAST: u32 = 1 << 28;
pub const BUFFER: usize = 2048;
pub const TSO_MAX_MSS: u16 = 2047;
pub const TSO_MIN_MSS: u16 = 64;
pub const TSO_MAX_PAYLOAD: u32 = 64000;
pub const TSO_MAX_FRAGS: usize = 8;
pub const TSO_MAX_HDR: u32 = 1023;
pub const TSO_GIANT_SEND_V4: u32 = 1 << 26;
pub const TSO_GIANT_SEND_V6: u32 = 1 << 25;
pub const TSO_GTTCPHO_SHIFT: u32 = 18;
pub const TSO_MSS_SHIFT: u32 = 18;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    A48,
    A49,
    B50,
    B51,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub pci_id: u32,
    pub revision: u8,
    pub tx_config: u32,
}

impl Identity {
    pub fn classify(self) -> Option<Method> {
        method(self.pci_id, self.tx_config)
    }

    pub fn supported_method(self) -> Option<Method> {
        self.classify().filter(|method| {
            self.pci_id == 0x8125_10ec && self.revision == 0x05 && *method == Method::B51
        })
    }
}

pub fn method(pci_id: u32, tx_config: u32) -> Option<Method> {
    if pci_id != 0x8125_10ec {
        return None;
    }
    match (tx_config & 0x7c800000, tx_config & 0x00700000) {
        (0x60800000, 0) => Some(Method::A48),
        (0x60800000, 0x100000) => Some(Method::A49),
        (0x64000000, 0) => Some(Method::B50),
        (0x64000000, 0x100000) => Some(Method::B51),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Descriptor {
    pub flags: u32,
    pub options: u32,
    pub address: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Layout,
    Busy,
    Length,
    Backend,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TsoFrag {
    pub bus: u64,
    pub length: usize,
}

pub struct TsoRequest<'a> {
    pub ip_version: u8,
    pub protocol: u8,
    pub ip_hdr_len: u8,
    pub tcp_hdr_len: u8,
    pub mss: u16,
    pub flags: u16,
    pub head_len: u32,
    pub payload_len: u32,
    pub tso_id: u32,
    pub frags: &'a [TsoFrag],
}

impl TsoRequest<'_> {
    pub fn validate(&self) -> Result<(), Error> {
        if self.protocol != 6 || !matches!(self.ip_version, 4 | 6) {
            return Err(Error::Length);
        }
        if self.ip_hdr_len != if self.ip_version == 4 { 20 } else { 40 } {
            return Err(Error::Length);
        }
        if !(20..=60).contains(&self.tcp_hdr_len) || self.tcp_hdr_len % 4 != 0 {
            return Err(Error::Length);
        }
        if self.head_len != 14 + u32::from(self.ip_hdr_len) + u32::from(self.tcp_hdr_len) {
            return Err(Error::Length);
        }
        if self.mss < TSO_MIN_MSS || self.mss > TSO_MAX_MSS {
            return Err(Error::Length);
        }
        if self.head_len > TSO_MAX_HDR {
            return Err(Error::Length);
        }
        if self.payload_len < 1 {
            return Err(Error::Length);
        }
        let total = u64::from(self.head_len) + u64::from(self.payload_len);
        if total > u64::from(TSO_MAX_PAYLOAD) {
            return Err(Error::Length);
        }
        let l4_offset = 14 + u32::from(self.ip_hdr_len);
        if l4_offset & 1 != 0 || l4_offset > 0x70 {
            return Err(Error::Length);
        }
        if self.frags.is_empty() || self.frags.len() > TSO_MAX_FRAGS {
            return Err(Error::Length);
        }
        if self.tso_id == 0 {
            return Err(Error::Length);
        }
        let mut sum = 0u64;
        for frag in self.frags {
            if frag.length == 0 || frag.length > 65535 || frag.bus & 2047 != 0 {
                return Err(Error::Length);
            }
            sum += frag.length as u64;
        }
        if sum != total || (self.frags[0].length as u64) < u64::from(self.head_len) {
            return Err(Error::Length);
        }
        Ok(())
    }
}

/// The backend owns both descriptor tables and all buffers. Publish writes
/// address/options before OWN with a device barrier. Status loads acquire DMA
/// completion before reading payload. Synchronization of cached buffers is
/// per slot. Resetting software indices requires confirmed DMA quiescence.
#[derive(Debug, PartialEq, Eq)]
pub enum Rx {
    Pending,
    Rejected,
    Frame(usize),
    /// A completed external buffer: (slot, device-visible buffer address,
    /// payload length). The payload stays in the external buffer and the slot
    /// is not re-armed; the caller re-arms it with `Ring::arm_rx_external`.
    External(usize, u64, usize),
}

pub trait Dma {
    fn status(&mut self, receive: bool, slot: usize) -> Result<u32, Error>;
    fn publish(&mut self, receive: bool, slot: usize, value: Descriptor) -> Result<(), Error>;
    /// Publishes a descriptor whose `address` is a device-visible external
    /// buffer: 2048-aligned and outside the private buffer arena. The flags
    /// shape must match what `publish` accepts (RX: `OWN | end | BUFFER as
    /// u32`; TX: `end` or `OWN | end | (3 << 28) | length` with length
    /// 60..=1514). The slot must be CPU-owned and options must be zero.
    fn publish_external(&mut self, receive: bool, slot: usize, value: Descriptor) -> Result<(), Error>;
    /// Publish an external TX buffer whose checksum fields were prepared by
    /// the caller for the requested offload.
    fn publish_external_checksum(&mut self, _slot: usize, _value: Descriptor) -> Result<(), Error> {
        Err(Error::Layout)
    }
    /// Reads back the 64-bit buffer address from the descriptor's two 32-bit
    /// address words, with a device barrier before the loads.
    fn descriptor_address(&mut self, receive: bool, slot: usize) -> Result<u64, Error>;
    fn sync_receive_buffer(&mut self, slot: usize) -> Result<(), Error>;
    fn read_payload(&mut self, slot: usize, output: &mut [u8]) -> Result<(), Error>;
    fn write_padded_payload(&mut self, slot: usize, payload: &[u8], padded: usize)
        -> Result<(), Error>;
    fn kick_tx(&mut self) -> Result<(), Error>;
    /// Publishes one descriptor of a TSO request: `options` holds the MSS
    /// field, `flags` carries GiantSendv4/6 with GTTCPHO and the fragment
    /// length. Default backends do not support TSO.
    fn publish_tso(&mut self, slot: usize, descriptor: Descriptor) -> Result<(), Error> {
        let _ = (slot, descriptor);
        Err(Error::Layout)
    }
    /// Single doorbell for `count` consecutively published TX descriptors.
    fn kick_tx_count(&mut self, count: usize) -> Result<(), Error> {
        for _ in 0..count {
            self.kick_tx()?;
        }
        Ok(())
    }
    /// TX slots free for new publications; the default reports unbounded.
    fn tx_slots_free(&self) -> usize {
        usize::MAX
    }
}

pub struct Ring<const N: usize> {
    tx: usize,
    rx: usize,
    tx_buffers: u64,
    rx_buffers: u64,
    rx_completion: u32,
    ready: bool,
    // External buffer address per RX slot; 0 marks a private buffer slot.
    rx_external: [u64; N],
    // When set, the next consumed private RX slot re-arms at this external
    // buffer instead of its private one (slot migration onto the shared pool).
    rearm_next: Option<u64>,
    // TSO request id whose LAST descriptor landed in the slot; 0 = none.
    tso_last: [u32; N],
}

impl<const N: usize> Ring<N> {
    pub fn new(base: u64, size: u64) -> Result<Self, Error> {
        if !(2..=1024).contains(&N) || !N.is_power_of_two() || base & 2047 != 0 {
            return Err(Error::Layout);
        }
        let half = (N * BUFFER) as u64;
        if size < half * 2 || base.checked_add(half * 2).is_none() {
            return Err(Error::Layout);
        }
        Ok(Self { tx: 0, rx: 0, tx_buffers: base, rx_buffers: base + half, rx_completion:0,
                  ready: false, rx_external: [0; N], rearm_next: None, tso_last: [0; N] })
    }

    pub fn with_private_rx(mut self, base: u64) -> Result<Self, Error> {
        if base & 2047 != 0 || base.checked_add((N * BUFFER) as u64).is_none() {
            return Err(Error::Layout);
        }
        self.rx_buffers = base;
        Ok(self)
    }

    fn desc(&self, receive: bool, slot: usize, flags: u32) -> Descriptor {
        let base = if receive { self.rx_buffers } else { self.tx_buffers };
        Descriptor {
            flags: flags | if slot == N - 1 { END } else { 0 },
            options: 0,
            address: base + (slot * BUFFER) as u64,
        }
    }

    /// Requires a stopped controller. A backend error leaves the ring unusable
    /// until this operation completes after a confirmed stop.
    pub fn initialize(&mut self, io: &mut impl Dma) -> Result<(), Error> {
        self.initialize_tx(io)?;
        self.initialize_rx(io)
    }

    pub fn initialize_tx(&mut self, io: &mut impl Dma) -> Result<(), Error> {
        self.ready = false;
        for slot in 0..N {
            io.publish(false, slot, self.desc(false, slot, 0))?;
        }
        self.tx = 0;
        self.tso_last = [0; N];
        self.ready = true;
        Ok(())
    }

    /// Initialize only the selected RX ring. The RX engine must be stopped.
    pub fn initialize_rx(&mut self, io: &mut impl Dma) -> Result<(), Error> {
        self.ready = false;
        for slot in 0..N {
            io.sync_receive_buffer(slot)?;
            io.publish(true, slot, self.desc(true, slot, OWN | BUFFER as u32))?;
        }
        self.rx = 0;
        self.rx_external = [0; N];
        self.rearm_next = None;
        self.ready = true;
        Ok(())
    }

    pub fn current_rx_external(&self) -> bool { self.rx_external[self.rx] != 0 }

    pub fn rx_ready(&self, io: &mut impl Dma) -> Result<bool, Error> {
        if !self.ready { return Err(Error::Stopped); }
        Ok(io.status(true, self.rx)? & OWN == 0)
    }

    /// Offer an external buffer for the re-arm of the next consumed private
    /// RX slot. Cleared by that re-arm; `None` keeps the private fallback.
    pub fn migrate_next_rx(&mut self, bus: Option<u64>) {
        self.rearm_next = bus;
    }

    pub fn send(&mut self, io: &mut impl Dma, payload: &[u8]) -> Result<(), Error> {
        self.send_observed(io, payload, |_, _| Ok(()))
    }

    pub fn send_ipv4_checksum(&mut self, io: &mut impl Dma, payload: &[u8]) -> Result<(), Error> {
        self.send_checksum(io,payload,TX_IPV4_CHECKSUM)
    }

    pub fn send_checksum(&mut self, io: &mut impl Dma, payload: &[u8], options:u32) -> Result<(),Error> {
        if !self.ready { return Err(Error::Stopped); }
        let result = self.send_frame(io, payload, options, |_, _| Ok(()));
        if matches!(result, Err(Error::Backend | Error::Layout)) { self.ready = false; }
        result
    }

    pub fn send_observed<I: Dma>(&mut self, io: &mut I, payload: &[u8],
        before_kick: impl FnOnce(&I, usize) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if !self.ready {
            return Err(Error::Stopped);
        }
        let result = self.send_frame(io, payload, 0, before_kick);
        if matches!(result, Err(Error::Backend | Error::Layout)) {
            self.ready = false;
        }
        result
    }

    fn send_frame<I: Dma>(&mut self, io: &mut I, payload: &[u8], options: u32, before_kick: impl FnOnce(&I, usize) -> Result<(), Error>) -> Result<(), Error> {
        if !(14..=1514).contains(&payload.len()) {
            return Err(Error::Length);
        }
        let slot = self.tx;
        if io.status(false, slot)? & OWN != 0 {
            return Err(Error::Busy);
        }
        let padded = payload.len().max(60);
        io.write_padded_payload(slot, payload, padded)?;
        let mut descriptor = self.desc(false, slot, OWN | FIRST | LAST | padded as u32);
        descriptor.options = options;
        io.publish(false, slot, descriptor)?;
        before_kick(io, slot)?;
        self.tx = (slot + 1) % N;
        io.kick_tx()
    }

    /// Sends from a device-visible external buffer at `bus` (2048-aligned,
    /// outside the private arena). The caller guarantees the buffer is
    /// zero-filled from `length` to the padded size. It carries complete
    /// checksums or the seed for the requested offload. The slot discipline is
    /// identical to `send_frame`; the no-close in-order and capacity checks
    /// that `write_padded_payload` performs on the copy path are performed by
    /// `publish_external` because the external path has no fill step.
    pub fn send_external(&mut self, io: &mut impl Dma, bus: u64, length: usize, options: u32) -> Result<(), Error> {
        self.send_external_frame::<false>(io, bus, length, options)
    }

    pub fn send_external_checksum(
        &mut self, io: &mut impl Dma, bus: u64, length: usize, options: u32,
    ) -> Result<(), Error> {
        self.send_external_frame::<true>(io, bus, length, options)
    }

    fn send_external_frame<const CHECKSUM: bool>(
        &mut self, io: &mut impl Dma, bus: u64, length: usize, options: u32,
    ) -> Result<(), Error> {
        if !self.ready {
            return Err(Error::Stopped);
        }
        let result = self.publish_external_frame::<CHECKSUM>(io, bus, length, options);
        if matches!(result, Err(Error::Backend | Error::Layout)) {
            self.ready = false;
        }
        result
    }

    fn publish_external_frame<const CHECKSUM: bool>(
        &mut self, io: &mut impl Dma, bus: u64, length: usize, options: u32,
    ) -> Result<(), Error> {
        if !(14..=1514).contains(&length) {
            return Err(Error::Length);
        }
        let slot = self.tx;
        if io.status(false, slot)? & OWN != 0 {
            return Err(Error::Busy);
        }
        let padded = length.max(60);
        let descriptor = Descriptor {
            flags: OWN | FIRST | LAST | padded as u32 | if slot == N - 1 { END } else { 0 },
            options,
            address: bus,
        };
        if CHECKSUM { io.publish_external_checksum(slot, descriptor)?; }
        else { io.publish_external(false, slot, descriptor)?; }
        self.tx = (slot + 1) % N;
        io.kick_tx()
    }

    /// Sends one hardware TSO request as one descriptor per fragment,
    /// published consecutively from the TX cursor with a single
    /// `kick_tx_count` doorbell. All `nfrags` slots are checked free before
    /// the first publish. Returns the first slot used; the request id is
    /// recorded on the final descriptor's slot for `retired_tso`.
    pub fn send_tso(&mut self, io: &mut impl Dma, request: &TsoRequest<'_>) -> Result<usize, Error> {
        if !self.ready {
            return Err(Error::Stopped);
        }
        let result = self.send_tso_frame(io, request);
        if matches!(result, Err(Error::Backend | Error::Layout)) {
            self.ready = false;
        }
        result
    }

    fn send_tso_frame(&mut self, io: &mut impl Dma, request: &TsoRequest<'_>) -> Result<usize, Error> {
        request.validate()?;
        let nfrags = request.frags.len();
        if io.tx_slots_free() < nfrags {
            return Err(Error::Busy);
        }
        let first = self.tx;
        let giant = if request.ip_version == 4 { TSO_GIANT_SEND_V4 } else { TSO_GIANT_SEND_V6 };
        let l4_offset = 14 + u32::from(request.ip_hdr_len);
        for (i, frag) in request.frags.iter().enumerate() {
            let slot = (first + i) % N;
            let mut flags = OWN | giant | (l4_offset << TSO_GTTCPHO_SHIFT) | frag.length as u32;
            if i == 0 {
                flags |= FIRST;
            }
            if i + 1 == nfrags {
                flags |= LAST;
            }
            if slot == N - 1 {
                flags |= END;
            }
            io.publish_tso(slot, Descriptor {
                flags,
                options: (u32::from(request.mss) & 0x7ff) << TSO_MSS_SHIFT,
                address: frag.bus,
            })?;
        }
        self.tso_last[(first + nfrags - 1) % N] = request.tso_id;
        self.tx = (first + nfrags) % N;
        io.kick_tx_count(nfrags)?;
        Ok(first)
    }

    /// Reports request ids whose LAST descriptor falls in the reclaimed
    /// window `[from_slot, from_slot + count)` in slot order, zeroing the
    /// records. Callers sample `oldest_tx_slot()` before `reclaim_tx()`.
    pub fn retired_tso(&mut self, from_slot: usize, count: usize, out: &mut [u32]) -> usize {
        let mut n = 0;
        for k in 0..count {
            let slot = (from_slot + k) % N;
            let id = self.tso_last[slot];
            if id != 0 && n < out.len() {
                out[n] = id;
                self.tso_last[slot] = 0;
                n += 1;
            }
        }
        n
    }

    /// The backend supplies the selected method's RX error-summary mask.
    pub fn rx_transport_checksum(&self) -> u32 {
        match self.rx_completion & ((3<<17)|(1<<16)|(1<<15)|(1<<14)) {
            value if value == (1<<17) => 6,
            value if value == (1<<18) => 17,
            _ => 0,
        }
    }

    /// Copy-path receive. Consumes only private slots: a slot armed with
    /// `arm_rx_external(Some(_))` at the cursor is skipped (`Rx::Pending`,
    /// without consuming or copying) and left for `receive_external`, so the
    /// two paths never consume each other's slots.
    pub fn receive(
        &mut self, io: &mut impl Dma, out: &mut [u8], error_mask: u32,
    ) -> Result<Rx, Error> {
        if !self.ready {
            return Err(Error::Stopped);
        }
        let result = self.receive_frame(io, out, error_mask);
        if result.is_err() {
            self.ready = false;
        }
        result
    }

    fn receive_frame(
        &mut self, io: &mut impl Dma, out: &mut [u8], error_mask: u32,
    ) -> Result<Rx, Error> {
        if error_mask == 0 {
            return Err(Error::Layout);
        }
        self.rx_completion=0;
        let slot = self.rx;
        if self.rx_external[slot] != 0 {
            return Ok(Rx::Pending);
        }
        let flags = io.status(true, slot)?;
        if flags & OWN != 0 {
            return Ok(Rx::Pending);
        }
        let length = ((flags & 0x3fff) as usize).checked_sub(4);
        let valid = flags & (FIRST | LAST | error_mask) == FIRST | LAST
            && length.is_some_and(|n| (14..=1514).contains(&n) && n <= out.len());
        let packet = if valid {
            let n = length.unwrap();
            io.read_payload(slot, &mut out[..n])?;
            self.rx_completion=flags;
            Rx::Frame(n)
        } else {
            Rx::Rejected
        };
        io.sync_receive_buffer(slot)?;
        if let Some(bus) = self.rearm_next.take() {
            self.arm_rx_external(io, slot, Some(bus))?;
        } else {
            io.publish(true, slot, self.desc(true, slot, OWN | BUFFER as u32))?;
            self.rx_external[slot] = 0;
        }
        self.rx = (slot + 1) % N;
        Ok(packet)
    }

    /// Arms RX `slot` with `Some(bus)`, an external device-visible buffer
    /// (non-zero, 2048-aligned and outside the private arena), or with `None`,
    /// which re-arms the slot at its private buffer address and returns it to
    /// the copy path. The slot must not be device-owned in either case. The
    /// recorded address marks the slot as external (or private) for the
    /// receive paths until `initialize` clears the records.
    pub fn arm_rx_external(&mut self, io: &mut impl Dma, slot: usize, bus: Option<u64>) -> Result<(), Error> {
        if slot >= N {
            return Err(Error::Layout);
        }
        match bus {
            Some(bus) => {
                if bus == 0 {
                    return Err(Error::Layout);
                }
                let descriptor = Descriptor {
                    flags: OWN | if slot == N - 1 { END } else { 0 } | BUFFER as u32,
                    options: 0,
                    address: bus,
                };
                io.publish_external(true, slot, descriptor)?;
                self.rx_external[slot] = bus;
            }
            None => {
                io.publish(true, slot, self.desc(true, slot, OWN | BUFFER as u32))?;
                self.rx_external[slot] = 0;
            }
        }
        Ok(())
    }

    /// External counterpart of `receive`. Consumes only RX slots armed with
    /// `arm_rx_external(Some(_))`: a valid completion is returned as
    /// `Rx::External` without copying the payload and without re-arming the
    /// slot, so the caller must forward the buffer reference and re-arm the
    /// slot with `arm_rx_external` before the cursor wraps around to it again.
    /// An invalid completion is re-armed at the same external address and
    /// rejected. When the cursor rests on a private slot this returns
    /// `Rx::Pending` without consuming: `receive` handles private slots and
    /// skips external ones, so the two paths never consume each other's slots.
    pub fn receive_external(&mut self, io: &mut impl Dma, error_mask: u32) -> Result<Rx, Error> {
        if !self.ready {
            return Err(Error::Stopped);
        }
        let result = self.receive_external_frame(io, error_mask);
        if result.is_err() {
            self.ready = false;
        }
        result
    }

    fn receive_external_frame(&mut self, io: &mut impl Dma, error_mask: u32) -> Result<Rx, Error> {
        if error_mask == 0 {
            return Err(Error::Layout);
        }
        self.rx_completion = 0;
        let slot = self.rx;
        if self.rx_external[slot] == 0 {
            return Ok(Rx::Pending);
        }
        let flags = io.status(true, slot)?;
        if flags & OWN != 0 {
            return Ok(Rx::Pending);
        }
        let length = ((flags & 0x3fff) as usize).checked_sub(4);
        let valid = flags & (FIRST | LAST | error_mask) == FIRST | LAST
            && length.is_some_and(|n| (14..=1514).contains(&n));
        if valid {
            self.rx_completion = flags;
            self.rx = (slot + 1) % N;
            return Ok(Rx::External(slot, self.rx_external[slot], length.unwrap()));
        }
        let bus = self.rx_external[slot];
        self.arm_rx_external(io, slot, Some(bus))?;
        self.rx = (slot + 1) % N;
        Ok(Rx::Rejected)
    }

    /// Reads back the external buffer address currently recorded in TX `slot`.
    pub fn tx_external_address(&self, io: &mut impl Dma, slot: usize) -> Result<u64, Error> {
        io.descriptor_address(false, slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Model {
        flags: [[u32; 4]; 2],
        base: u64,
        kicks: usize,
        syncs: usize,
        reads: usize,
        kick_failure: bool,
        addresses: [[u64; 4]; 2],
        external_publishes: usize,
    }
    impl Dma for Model {
        fn status(&mut self, rx: bool, i: usize) -> Result<u32, Error> {
            Ok(self.flags[rx as usize][i])
        }
        fn publish(&mut self, rx: bool, i: usize, d: Descriptor) -> Result<(), Error> {
            assert_eq!(d.flags & END != 0, i == 3);
            assert_eq!(d.address, self.base + ((if rx { 4 } else { 0 }) + i) as u64 * 2048);
            self.flags[rx as usize][i] = d.flags;
            self.addresses[rx as usize][i] = d.address;
            Ok(())
        }
        fn publish_external(&mut self, rx: bool, i: usize, d: Descriptor) -> Result<(), Error> {
            if i >= 4 {
                return Err(Error::Layout);
            }
            let end = if i == 3 { END } else { 0 };
            let valid_flags = if rx {
                d.flags == OWN | end | BUFFER as u32
            } else {
                let length = d.flags & 0xffff;
                d.flags == end
                    || ((60..=1514).contains(&length) && d.flags == OWN | end | (3 << 28) | length)
            };
            let arena = self.base + (4 * BUFFER * 2) as u64;
            if !valid_flags || d.options != 0 || d.address & 2047 != 0
                || (d.address >= self.base && d.address < arena)
                || self.flags[rx as usize][i] & OWN != 0
            {
                return Err(Error::Layout);
            }
            self.flags[rx as usize][i] = d.flags;
            self.addresses[rx as usize][i] = d.address;
            self.external_publishes += 1;
            Ok(())
        }
        fn descriptor_address(&mut self, rx: bool, i: usize) -> Result<u64, Error> {
            if i >= 4 {
                return Err(Error::Layout);
            }
            Ok(self.addresses[rx as usize][i])
        }
        fn sync_receive_buffer(&mut self, _: usize) -> Result<(), Error> {
            self.syncs += 1;
            Ok(())
        }
        fn read_payload(&mut self, _: usize, out: &mut [u8]) -> Result<(), Error> {
            self.reads += 1;
            out.fill(0x55);
            Ok(())
        }
        fn write_padded_payload(&mut self, _: usize, p: &[u8], padded: usize) -> Result<(), Error> {
            assert_eq!(padded, p.len().max(60));
            Ok(())
        }
        fn kick_tx(&mut self) -> Result<(), Error> {
            self.kicks += 1;
            if self.kick_failure { Err(Error::Backend) } else { Ok(()) }
        }
    }
    #[test]
    fn method_requires_live_mac_signature() {
        assert_eq!(method(0x8125_10ec, 0x64100000), Some(Method::B51));
        assert_eq!(method(0x8125_10ec, 0x60900000), Some(Method::A49));
        assert_eq!(method(0x8125_10ec, 0x64200000), None);
        assert_eq!(method(0xffff_ffff, 0x64100000), None);
    }

    #[test]
    fn unknown_revision_never_selects_b51() {
        let known = Identity { pci_id: 0x8125_10ec, revision: 0x05, tx_config: 0x64100000 };
        assert_eq!(known.classify(), Some(Method::B51));
        assert_eq!(known.supported_method(), Some(Method::B51));
        for revision in [0x00, 0x01, 0x04, 0x06, 0xff] {
            let unknown = Identity { revision, ..known };
            assert_eq!(unknown.classify(), Some(Method::B51));
            assert_eq!(unknown.supported_method(), None);
        }
    }
    #[test]
    fn fault_after_own_publication_latches_until_reinitialization() {
        let mut ring = Ring::<4>::new(0x400000, 16384).unwrap();
        let mut io = Model { base: 0x400000, kick_failure: true, ..Model::default() };
        assert_eq!(ring.send(&mut io, &[0; 60]), Err(Error::Stopped));
        ring.initialize(&mut io).unwrap();
        assert_eq!(ring.send(&mut io, &[0; 60]), Err(Error::Backend));
        assert_ne!(io.flags[0][0] & OWN, 0);
        assert_eq!(ring.send(&mut io, &[0; 60]), Err(Error::Stopped));
        assert_eq!(ring.receive(&mut io, &mut [0; 60], 1 << 21), Err(Error::Stopped));
        assert_eq!(io.kicks, 1);
        io.flags = [[0; 4]; 2];
        io.kick_failure = false;
        ring.initialize(&mut io).unwrap();
        assert_eq!(ring.send(&mut io, &[0; 60]), Ok(()));
    }

    #[test]
    fn private_slots_migrate_to_external_buffers_on_rearm() {
        let mut ring = Ring::<4>::new(0x400000, 16384).unwrap();
        let mut io = Model { base: 0x400000, ..Model::default() };
        ring.initialize(&mut io).unwrap();
        // Complete private slot 0 and migrate its re-arm onto an external buffer.
        io.flags[1][0] = FIRST | LAST | 1518;
        ring.migrate_next_rx(Some(0x8000_1000));
        assert_eq!(ring.receive(&mut io, &mut [0; 1514], 1 << 20), Ok(Rx::Frame(1514)));
        assert_eq!(io.addresses[1][0], 0x8000_1000, "re-armed at the offered buffer");
        assert_ne!(io.flags[1][0] & OWN, 0);
        // Drain the rest of the ring so the cursor wraps to the migrated slot.
        ring.migrate_next_rx(None);
        for slot in 1..4 {
            io.flags[1][slot] = FIRST | LAST | 1518;
            assert_eq!(ring.receive(&mut io, &mut [0; 1514], 1 << 20), Ok(Rx::Frame(1514)));
        }
        // The migrated slot is external: receive() skips it, receive_external owns it.
        io.flags[1][0] = FIRST | LAST | 1518;
        assert_eq!(ring.receive(&mut io, &mut [0; 1514], 1 << 20), Ok(Rx::Pending));
        assert_eq!(ring.receive_external(&mut io, 1 << 20), Ok(Rx::External(0, 0x8000_1000, 1514)));
        // Without an offer the next consumed slot re-arms private again.
        assert_eq!(ring.arm_rx_external(&mut io, 0, None), Ok(()));
        io.flags[1][1] = FIRST | LAST | 1518;
        assert_eq!(ring.receive(&mut io, &mut [0; 1514], 1 << 20), Ok(Rx::Frame(1514)));
        assert_eq!(io.addresses[1][1], 0x400000 + (4 + 1) * 2048);
    }

    #[test]
    fn independent_rings_wrap_restart_and_reject_bad_completions() {
        let mut lan1 = Ring::<4>::new(0x400000, 16384).unwrap();
        let mut lan2 = Ring::<4>::new(0x800000, 16384).unwrap();
        let mut a = Model { base: 0x400000, ..Model::default() };
        let mut b = Model { base: 0x800000, ..Model::default() };
        lan1.initialize(&mut a).unwrap();
        lan2.initialize(&mut b).unwrap();
        let mut out = [0; 1514];
        let error_mask = 1 << 20;
        for _ in 0..9 {
            for slot in 0..4 {
                assert_eq!(lan1.receive(&mut a, &mut out, error_mask), Ok(Rx::Pending));
                a.flags[1][slot] = FIRST | LAST | 1518;
                assert_eq!(lan1.receive(&mut a, &mut out, error_mask), Ok(Rx::Frame(1514)));
                lan1.send(&mut a, &[0;14]).unwrap();
            }
            assert_eq!(lan1.send(&mut a, &[0;14]), Err(Error::Busy));
            a.flags[0].fill(0);
        }
        assert_eq!(lan1.send(&mut a, &[0;1515]), Err(Error::Length));
        for (slot, flags) in [0x3fff, FIRST | LAST | 3, FIRST | LAST | error_mask | 64, FIRST | 64].into_iter().enumerate() {
            a.flags[1][slot] = flags;
            assert_eq!(lan1.receive(&mut a, &mut out, error_mask), Ok(Rx::Rejected));
        }
        assert_eq!((a.kicks, a.reads, b.kicks, b.reads), (36, 36, 0, 0));
        lan1.initialize(&mut a).unwrap();
        assert_eq!(lan1.receive(&mut a, &mut out, error_mask), Ok(Rx::Pending));
        assert_eq!(b.flags[1][0] & OWN, OWN);
    }

    #[test]
    fn external_receive_returns_references_without_copying_and_defers_private_slots() {
        let mut ring = Ring::<4>::new(0x400000, 16384).unwrap();
        let mut io = Model { base: 0x400000, ..Model::default() };
        ring.initialize(&mut io).unwrap();
        // The device completed the first receptions into private slots 0..=2;
        // those slots are not owned and can now be converted to external use.
        // Slot 3 stays private.
        for slot in 0..3 {
            io.flags[1][slot] = FIRST | LAST | 64;
        }
        assert_eq!(ring.arm_rx_external(&mut io, 0, Some(0x8200_0000)), Ok(()));
        assert_eq!(ring.arm_rx_external(&mut io, 2, Some(0x8200_2000)), Ok(()));
        assert_eq!(io.external_publishes, 2);
        assert_eq!(io.addresses[1][0], 0x8200_0000);
        assert_eq!(io.addresses[1][2], 0x8200_2000);
        // Slot 0 completes with a 60-byte frame carrying a TCP transport
        // summary; no payload copy happens and the slot is not re-armed.
        io.flags[1][0] = FIRST | LAST | 64 | 1 << 17;
        assert_eq!(ring.receive_external(&mut io, 1 << 21), Ok(Rx::External(0, 0x8200_0000, 60)));
        assert_eq!(io.reads, 0);
        assert_eq!(ring.rx_transport_checksum(), 6);
        assert_eq!(io.flags[1][0] & OWN, 0);
        assert_eq!(io.external_publishes, 2);
        // The cursor rests on private slot 1: receive_external defers it and
        // the copy path consumes it.
        io.flags[1][1] = FIRST | LAST | 64;
        assert_eq!(ring.receive_external(&mut io, 1 << 21), Ok(Rx::Pending));
        assert_eq!(ring.receive(&mut io, &mut [0; 1514], 1 << 21), Ok(Rx::Frame(60)));
        assert_eq!(io.reads, 1);
        assert_eq!(io.addresses[1][1], 0x400000 + 5 * 2048);
        // An external slot that is still armed is pending until it completes.
        assert_eq!(ring.receive_external(&mut io, 1 << 21), Ok(Rx::Pending));
        io.flags[1][2] = FIRST | LAST | 204;
        assert_eq!(ring.receive_external(&mut io, 1 << 21), Ok(Rx::External(2, 0x8200_2000, 200)));
        assert_eq!(io.reads, 1);
    }

    #[test]
    fn copy_and_external_paths_never_consume_each_others_slots() {
        let mut ring = Ring::<4>::new(0x400000, 16384).unwrap();
        let mut io = Model { base: 0x400000, ..Model::default() };
        ring.initialize(&mut io).unwrap();
        // The device completed the first receptions; the slots are not owned
        // and can be armed.
        for slot in 0..4 {
            io.flags[1][slot] = FIRST | LAST | 64;
        }
        ring.arm_rx_external(&mut io, 0, Some(0x8200_0000)).unwrap();
        // The device then completes the external slot at the cursor; it must
        // not be stolen by the copy path or read from the stale private buffer.
        io.flags[1][0] = FIRST | LAST | 64;
        assert_eq!(ring.receive(&mut io, &mut [0; 1514], 1 << 21), Ok(Rx::Pending));
        assert_eq!(io.reads, 0);
        assert_eq!(ring.receive_external(&mut io, 1 << 21), Ok(Rx::External(0, 0x8200_0000, 60)));
        assert_eq!(io.reads, 0);
        // Pool exhaustion fallback: slot 1 returns to the private path and
        // receive_external defers it back to the copy path.
        ring.arm_rx_external(&mut io, 1, None).unwrap();
        assert_eq!(io.addresses[1][1], 0x400000 + 5 * 2048);
        assert_eq!(ring.receive_external(&mut io, 1 << 21), Ok(Rx::Pending));
        assert_eq!(io.reads, 0);
        io.flags[1][1] = FIRST | LAST | 64;
        assert_eq!(ring.receive(&mut io, &mut [0; 1514], 1 << 21), Ok(Rx::Frame(60)));
        assert_eq!(io.reads, 1);
    }

    #[test]
    fn invalid_external_completions_reject_and_rearm_at_same_address() {
        let mut ring = Ring::<4>::new(0x400000, 16384).unwrap();
        let mut io = Model { base: 0x400000, ..Model::default() };
        ring.initialize(&mut io).unwrap();
        for slot in 0..4 {
            io.flags[1][slot] = FIRST | LAST | 64;
            ring.arm_rx_external(&mut io, slot, Some(0x8200_0000 + slot as u64 * 0x1000)).unwrap();
        }
        let error_mask = 1 << 20;
        // Missing LAST, a 13-byte runt, a 1515-byte giant and an error-summary
        // bit are all invalid completions.
        let cases = [FIRST | 64, FIRST | LAST | 17, FIRST | LAST | 1519,
                     FIRST | LAST | error_mask | 64];
        for (turn, flags) in cases.into_iter().enumerate() {
            let slot = turn % 4;
            assert_eq!(ring.rx, slot);
            io.flags[1][slot] = flags;
            assert_eq!(ring.receive_external(&mut io, error_mask), Ok(Rx::Rejected));
            assert_eq!(io.addresses[1][slot], 0x8200_0000 + slot as u64 * 0x1000);
            assert_eq!(io.flags[1][slot], OWN | BUFFER as u32 | if slot == 3 { END } else { 0 });
        }
        assert_eq!(io.reads, 0);
        assert_eq!(io.external_publishes, 8); // four arms plus four re-arms
        // error_mask == 0 is refused like the copy path and stops the ring.
        assert_eq!(ring.receive_external(&mut io, 0), Err(Error::Layout));
        assert_eq!(ring.receive_external(&mut io, error_mask), Err(Error::Stopped));
    }

    #[test]
    fn send_external_publishes_external_address_and_validates() {
        let mut ring = Ring::<4>::new(0x400000, 16384).unwrap();
        let mut io = Model { base: 0x400000, ..Model::default() };
        ring.initialize(&mut io).unwrap();
        let bus = 0x8200_0800;
        assert_eq!(ring.send_external(&mut io, bus, 14, 0), Ok(()));
        assert_eq!(io.addresses[0][0], bus);
        assert_eq!(io.flags[0][0], OWN | FIRST | LAST | 60);
        assert_eq!(ring.send_external(&mut io, bus + 2048, 1514, 0), Ok(()));
        assert_eq!(io.addresses[0][1], bus + 2048);
        assert_eq!(io.flags[0][1], OWN | FIRST | LAST | 1514);
        assert_eq!(ring.send_external(&mut io, bus, 60, 0), Ok(()));
        assert_eq!(ring.send_external(&mut io, bus, 60, 0), Ok(()));
        assert_eq!(io.flags[0][3], OWN | FIRST | LAST | 60 | END);
        assert_eq!(io.kicks, 4);
        // The fifth send reuses an owned slot.
        assert_eq!(ring.send_external(&mut io, bus, 60, 0), Err(Error::Busy));
        assert_eq!(ring.send_external(&mut io, bus, 13, 0), Err(Error::Length));
        assert_eq!(ring.send_external(&mut io, bus, 1515, 0), Err(Error::Length));
        // The device completed every slot again; checksum offload options are
        // refused for external buffers and a Layout fault stops the ring.
        io.flags[0].fill(0);
        assert_eq!(ring.send_external(&mut io, bus, 60, TX_TCP_CHECKSUM), Err(Error::Layout));
        assert_eq!(ring.send_external(&mut io, bus, 60, 0), Err(Error::Stopped));
    }

    #[test]
    fn tx_external_address_reads_back_both_halves() {
        let mut ring = Ring::<4>::new(0x400000, 16384).unwrap();
        let mut io = Model { base: 0x400000, ..Model::default() };
        ring.initialize(&mut io).unwrap();
        let bus = 0x8123_4567_89AB_C800; // 2048-aligned, both halves non-zero
        assert_eq!(ring.send_external(&mut io, bus, 60, 0), Ok(()));
        assert_eq!(ring.tx_external_address(&mut io, 0), Ok(bus));
        // Untouched slots still report their private buffer addresses.
        assert_eq!(ring.tx_external_address(&mut io, 1), Ok(0x400000 + 2048));
    }

    fn tso_frags() -> [TsoFrag; 2] {
        [
            TsoFrag { bus: 0x8200_0800, length: 1514 },
            TsoFrag { bus: 0x8200_1000, length: 1540 },
        ]
    }

    fn tso_request(frags: &[TsoFrag]) -> TsoRequest<'_> {
        TsoRequest {
            ip_version: 4,
            protocol: 6,
            ip_hdr_len: 20,
            tcp_hdr_len: 20,
            mss: 1460,
            flags: 0,
            head_len: 54,
            payload_len: 3000,
            tso_id: 0x1234_5678,
            frags,
        }
    }

    #[test]
    fn tso_validation_rejects_malformed_requests() {
        let frags = tso_frags();
        assert_eq!(tso_request(&frags).validate(), Ok(()));
        for bad in [
            TsoRequest { mss: 63, ..tso_request(&frags) },
            TsoRequest { mss: 2048, ..tso_request(&frags) },
            TsoRequest { tcp_hdr_len: 21, ..tso_request(&frags) },
            TsoRequest { ip_hdr_len: 24, ..tso_request(&frags) },
            TsoRequest { head_len: 60, ..tso_request(&frags) },
            TsoRequest { payload_len: 2999, ..tso_request(&frags) },
            TsoRequest { payload_len: 0, ..tso_request(&frags) },
            TsoRequest { tso_id: 0, ..tso_request(&frags) },
            TsoRequest { frags: &[], ..tso_request(&frags) },
            TsoRequest { protocol: 17, ..tso_request(&frags) },
            TsoRequest { ip_version: 5, ip_hdr_len: 20, ..tso_request(&frags) },
            TsoRequest { tcp_hdr_len: 64, head_len: 98, ..tso_request(&frags) },
        ] {
            assert_eq!(bad.validate(), Err(Error::Length));
        }
        let skewed = [TsoFrag { bus: 0x8200_0800, length: 1515 }, frags[1]];
        assert_eq!(tso_request(&skewed).validate(), Err(Error::Length));
        let nine = [TsoFrag { bus: 0x8200_0000, length: 382 }; TSO_MAX_FRAGS + 1];
        assert_eq!(TsoRequest { frags: &nine, ..tso_request(&frags) }.validate(), Err(Error::Length));
        let misaligned = [TsoFrag { bus: 0x8200_0801, length: 1514 }, frags[1]];
        assert_eq!(tso_request(&misaligned).validate(), Err(Error::Length));
        let huge = [TsoFrag { bus: 0x8200_0800, length: 64001 }];
        assert_eq!(
            TsoRequest { payload_len: 63_947, frags: &huge, ..tso_request(&frags) }.validate(),
            Err(Error::Length),
        );
        let zeroed = [TsoFrag { bus: 0x8200_0800, length: 0 }, TsoFrag { bus: 0x8200_1000, length: 3054 }];
        assert_eq!(tso_request(&zeroed).validate(), Err(Error::Length));
        let short = [TsoFrag { bus: 0x8200_0800, length: 53 }, TsoFrag { bus: 0x8200_1000, length: 3001 }];
        assert_eq!(tso_request(&short).validate(), Err(Error::Length));
        let v6_frags = [TsoFrag { bus: 0x8200_0800, length: 1534 }, TsoFrag { bus: 0x8200_1000, length: 1540 }];
        let v6 = TsoRequest { ip_version: 6, ip_hdr_len: 40, head_len: 74, frags: &v6_frags, ..tso_request(&frags) };
        assert_eq!(v6.validate(), Ok(()));
    }

    #[test]
    fn tso_send_rejects_invalid_requests_without_stopping_the_ring() {
        let mut ring = Ring::<4>::new(0x400000, 16384).unwrap();
        let mut io = Model { base: 0x400000, ..Model::default() };
        ring.initialize(&mut io).unwrap();
        let frags = tso_frags();
        assert_eq!(
            ring.send_tso(&mut io, &TsoRequest { mss: 63, ..tso_request(&frags) }),
            Err(Error::Length),
        );
        assert_eq!(ring.send(&mut io, &[0; 60]), Ok(()));
        // The default backend does not implement TSO: a valid request faults
        // the ring like any other Layout error.
        assert_eq!(ring.send_tso(&mut io, &tso_request(&frags)), Err(Error::Layout));
        assert_eq!(ring.send(&mut io, &[0; 60]), Err(Error::Stopped));
    }
}
