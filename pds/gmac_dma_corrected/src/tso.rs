//! Shared TSO validation, software segmentation fallback and bounded
//! submission queue (contract `docs/tso-design.md`).
//!
//! A [`TsoRequest`] is an oversized TCP packet representation: Ethernet,
//! IP and TCP headers plus payload scattered over a few device-reachable
//! buffers. [`validate`] applies the shared acceptance rules (contract §2)
//! identically on every surface. [`segment`] is the software fallback
//! (contract §5): it splits a validated request into MSS-sized frames with
//! fully computed IPv4/IPv6 and TCP checksums and hands each frame to a
//! caller emitter. [`TsoQueue`] is the bounded SPSC ticket ring (contract
//! §6) mirroring `zc::RefQueue`; [`TsoCounters`] holds the per-NIC
//! diagnostics (contract §7).
//!
//! Outside `#[cfg(test)]` this module is `core`-only and self-contained: it
//! is copy-in compiled standalone (`rustc --test tso.rs`) and stays out of
//! every PD module tree until integration wires it. The fallback reads
//! fragment bytes through `TsoFrag::physical`, treated as a CPU-readable
//! address in the caller's address space (device translation happens only
//! at DMA submission).

#![allow(dead_code)]

use core::sync::atomic::{AtomicU32, Ordering};

// Limits from contract §1.
/// Maximum scatter-gather fragments per request.
pub const TSO_MAX_FRAGS: usize = 8;
/// Maximum `head_len + payload_len` (LSO_64K).
pub const TSO_MAX_PAYLOAD: u32 = 64000;
/// Minimum MSS (dwmac ETH_DMACxCR recommendation).
pub const TSO_MIN_MSS: u16 = 64;
/// Maximum MSS (RTL8125 MSS_MAX).
pub const TSO_MAX_MSS: u16 = 2047;
/// Maximum Ethernet+IP+TCP header bytes (dwmac TDES2 limit).
pub const TSO_MAX_HDR: u32 = 1023;
/// Maximum per-descriptor chunk (dwmac TDES2_BUFFER1_SIZE_MASK).
pub const TSO_MAX_CHUNK: u32 = 16383;

/// IPv6 TSO acceptance (`R5S_TSO_IPV6`, default enabled). When `false`,
/// IPv6 requests are rejected with [`TsoError::Unsupported`].
pub const TSO_IPV6: bool = true;

/// Largest ordinary Ethernet frame (14 + 1500). A request with
/// `head_len + payload_len` above this is oversized and needs TSO or the
/// software fallback.
pub const TSO_MAX_FRAME: u32 = 1514;

/// Rejection and queue errors (contract §2 / §6).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TsoError {
    /// `protocol` is not TCP (6).
    NotTcp,
    /// `ip_version` is not 4 or 6.
    BadIpVersion,
    /// `ip_hdr_len` disagrees with `ip_version`.
    BadIpHdrLen,
    /// `tcp_hdr_len` outside 20..=60 or not a multiple of 4.
    BadTcpHdrLen,
    /// `head_len != 14 + ip_hdr_len + tcp_hdr_len`.
    BadHeadLen,
    /// `mss` outside `TSO_MIN_MSS..=TSO_MAX_MSS`.
    BadMss,
    /// `head_len` exceeds `TSO_MAX_HDR`.
    BadHeader,
    /// `payload_len < 1`.
    BadPayload,
    /// `nfrags` zero or above `TSO_MAX_FRAGS`.
    BadFragCount,
    /// Zero fragment length or `frags[0]` shorter than `head_len`.
    BadFragLength,
    /// Fragment lengths do not sum to `head_len + payload_len`.
    LengthSum,
    /// A fragment `physical` is not 64-byte aligned.
    Misaligned,
    /// `tso_id == 0` (0 marks "no request" in completion reporting).
    ZeroId,
    /// IPv6 while `TSO_IPV6` is compiled out.
    Unsupported,
    /// `head_len + payload_len` exceeds `TSO_MAX_PAYLOAD`.
    Oversized,
    /// Queue backpressure state: a full queue answers `Ok(false)`, a queue
    /// with corrupt cursors answers `Err(Busy)`.
    Busy,
}

/// One scatter-gather fragment: device-visible bus address and byte count
/// (contract §1 `dma_tso_frag`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TsoFrag {
    pub physical: u64,
    pub length: u32,
}

/// Oversized TCP packet representation (contract §1 `dma_tso_request`).
/// `frags[0]` starts with the Ethernet+IP+TCP headers (`frags[0].length >=
/// head_len`); the bytes after `head_len` in `frags[0]` and all of
/// `frags[i>0]` are payload.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TsoRequest {
    pub ip_version: u8,
    pub protocol: u8,
    pub ip_hdr_len: u8,
    pub tcp_hdr_len: u8,
    pub mss: u16,
    pub flags: u16,
    pub head_len: u32,
    pub payload_len: u32,
    pub tso_id: u32,
    pub nfrags: u8,
    pub reserved: [u8; 3],
    pub frags: [TsoFrag; TSO_MAX_FRAGS],
}

/// Shared acceptance rules (contract §2). Every rejection maps to one
/// [`TsoError`]; valid requests are never silently dropped.
pub fn validate(request: &TsoRequest) -> Result<(), TsoError> {
    validate_with(request, TSO_IPV6)
}

/// [`validate`] with an explicit IPv6 policy (the build-time const is
/// folded in through [`validate`]; tests exercise both branches here).
fn validate_with(request: &TsoRequest, ipv6: bool) -> Result<(), TsoError> {
    if request.ip_version != 4 && request.ip_version != 6 {
        return Err(TsoError::BadIpVersion);
    }
    if request.ip_version == 6 && !ipv6 {
        return Err(TsoError::Unsupported);
    }
    if request.protocol != 6 {
        return Err(TsoError::NotTcp);
    }
    let want_ip_hdr_len = if request.ip_version == 4 { 20u8 } else { 40u8 };
    if request.ip_hdr_len != want_ip_hdr_len {
        return Err(TsoError::BadIpHdrLen);
    }
    if request.tcp_hdr_len < 20 || request.tcp_hdr_len > 60 || request.tcp_hdr_len % 4 != 0 {
        return Err(TsoError::BadTcpHdrLen);
    }
    // Header cap is checked before the exact-length rule so an oversized
    // head_len reports BadHeader rather than BadHeadLen.
    if request.head_len > TSO_MAX_HDR {
        return Err(TsoError::BadHeader);
    }
    let expect_head = 14u32 + u32::from(request.ip_hdr_len) + u32::from(request.tcp_hdr_len);
    if request.head_len != expect_head {
        return Err(TsoError::BadHeadLen);
    }
    if request.mss < TSO_MIN_MSS || request.mss > TSO_MAX_MSS {
        return Err(TsoError::BadMss);
    }
    // Every emitted segment is head_len + mss bytes at most and must fit
    // the 1500-byte MTU plus Ethernet header (IPv6 therefore caps MSS at
    // 1440 with a 20-byte TCP header).
    if request.head_len + u32::from(request.mss) > TSO_MAX_FRAME {
        return Err(TsoError::BadMss);
    }
    if request.payload_len < 1 {
        return Err(TsoError::BadPayload);
    }
    let total = u64::from(request.head_len) + u64::from(request.payload_len);
    if total > u64::from(TSO_MAX_PAYLOAD) {
        return Err(TsoError::Oversized);
    }
    let n = request.nfrags as usize;
    if request.nfrags < 1 || n > TSO_MAX_FRAGS {
        return Err(TsoError::BadFragCount);
    }
    let mut sum = 0u64;
    for frag in &request.frags[..n] {
        if frag.length == 0 {
            return Err(TsoError::BadFragLength);
        }
        if frag.physical & 63 != 0 {
            return Err(TsoError::Misaligned);
        }
        sum += u64::from(frag.length);
    }
    if request.frags[0].length < request.head_len {
        return Err(TsoError::BadFragLength);
    }
    if sum != total {
        return Err(TsoError::LengthSum);
    }
    if request.tso_id == 0 {
        return Err(TsoError::ZeroId);
    }
    Ok(())
}

/// `true` when the request cannot travel as one ordinary Ethernet frame
/// (`head_len + payload_len > 1514`) and needs TSO or the fallback.
pub fn is_oversized(request: &TsoRequest) -> bool {
    u64::from(request.head_len) + u64::from(request.payload_len) > u64::from(TSO_MAX_FRAME)
}

/// Copy `dst.len()` bytes from the concatenated fragment stream at byte
/// `offset` into `dst`. A stream shorter than needed (only reachable with a
/// request already rejected by [`validate`]) zeroes the tail so emitted
/// frames stay deterministic.
fn copy_out(frags: &[TsoFrag], offset: usize, dst: &mut [u8]) {
    let mut skip = offset;
    let mut done = 0usize;
    for frag in frags {
        let len = frag.length as usize;
        if skip >= len {
            skip -= len;
            continue;
        }
        let take = core::cmp::min(len - skip, dst.len() - done);
        // Safety: `physical` is a CPU-readable address owned by the caller
        // for the duration of the fallback (see module docs); `take` bytes
        // lie inside this fragment by construction.
        unsafe {
            core::ptr::copy_nonoverlapping(
                (frag.physical as *const u8).add(skip),
                dst.as_mut_ptr().add(done),
                take,
            );
        }
        done += take;
        skip = 0;
        if done == dst.len() {
            return;
        }
    }
    for byte in &mut dst[done..] {
        *byte = 0;
    }
}

/// Add `data` to a running 16-bit one's-complement sum (big-endian words;
/// an odd trailing byte is padded with a zero byte).
fn add_bytes(sum: &mut u32, data: &[u8]) {
    let mut rest = data;
    while rest.len() >= 2 {
        *sum += u32::from(u16::from_be_bytes([rest[0], rest[1]]));
        rest = &rest[2..];
    }
    if !rest.is_empty() {
        *sum += u32::from(rest[0]) << 8;
    }
}

/// Fold a running one's-complement sum and invert it.
fn finish(mut sum: u32) -> u16 {
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// Software segmentation fallback (contract §5).
///
/// Validates `request` first, then emits `ceil(payload_len / mss)` frames
/// through `emit`. Each frame is Ethernet(14, copied) + IP (v4: new total
/// length, `id = base_id + k` wrapping, checksum recomputed; v6: new
/// payload length) + TCP (`seq = base_seq + k*mss` wrapping, PSH only on
/// the final segment, checksum recomputed over the pseudo-header and the
/// segment) + that segment's payload, built in `scratch`.
///
/// The caller must provide `scratch.len() >= 14 + ip_hdr_len + tcp_hdr_len
/// + mss` (at most 2161 bytes); a smaller buffer is a caller bug and trips
/// an assertion. `emit` errors propagate; the return value is the number of
/// segments emitted. Checksums are computed in software to completion, so
/// the frames travel the ordinary TX path with offload disabled.
pub fn segment<F>(request: &TsoRequest, scratch: &mut [u8], emit: &mut F) -> Result<u32, TsoError>
where
    F: FnMut(&[u8]) -> Result<(), TsoError>,
{
    let mut next = 0;
    segment_window(request, scratch, &mut next, u32::MAX, emit)?;
    Ok(next)
}

pub fn segment_window<F>(
    request: &TsoRequest, scratch: &mut [u8], next: &mut u32, budget: u32, emit: &mut F,
) -> Result<bool, TsoError>
where F: FnMut(&[u8]) -> Result<(), TsoError>,
{
    validate(request)?;
    let n = request.nfrags as usize;
    let frags = &request.frags[..n];
    let mss = request.mss as usize;
    let head = request.head_len as usize;
    let payload = request.payload_len as usize;
    let ip_len = request.ip_hdr_len as usize;
    let tcp_len = request.tcp_hdr_len as usize;
    let ip_off = 14usize;
    let tcp_off = ip_off + ip_len;
    assert!(scratch.len() >= head + mss, "segment scratch too small");

    // Template identifiers, read before any per-segment patching.
    let mut word = [0u8; 4];
    copy_out(frags, ip_off + 4, &mut word[..2]);
    let base_id = u16::from_be_bytes([word[0], word[1]]);
    copy_out(frags, tcp_off + 4, &mut word);
    let base_seq = u32::from_be_bytes(word);

    let segments = (payload + mss - 1) / mss;
    let end = (*next as usize).saturating_add(budget as usize).min(segments);
    for k in *next as usize..end {
        let seg = core::cmp::min(mss, payload - k * mss);
        let frame_len = head + seg;
        {
            let frame = &mut scratch[..frame_len];
            copy_out(frags, 0, &mut frame[..head]);
            copy_out(frags, head + k * mss, &mut frame[head..]);

            if request.ip_version == 4 {
                let ip = &mut frame[ip_off..ip_off + 20];
                let total = (20usize + tcp_len + seg) as u16;
                ip[2..4].copy_from_slice(&total.to_be_bytes());
                let id = base_id.wrapping_add(k as u16);
                ip[4..6].copy_from_slice(&id.to_be_bytes());
                ip[10] = 0;
                ip[11] = 0;
                let mut sum = 0u32;
                add_bytes(&mut sum, ip);
                let csum = finish(sum);
                ip[10..12].copy_from_slice(&csum.to_be_bytes());
            } else {
                let ip = &mut frame[ip_off..ip_off + 40];
                let plen = (tcp_len + seg) as u16;
                ip[4..6].copy_from_slice(&plen.to_be_bytes());
            }

            {
                let seq = base_seq.wrapping_add((k as u32).wrapping_mul(mss as u32));
                let tcp = &mut frame[tcp_off..head];
                tcp[4..8].copy_from_slice(&seq.to_be_bytes());
                if k == segments - 1 {
                    tcp[13] |= 0x08;
                } else {
                    tcp[13] &= !0x08;
                }
                tcp[16] = 0;
                tcp[17] = 0;
            }

            // Pseudo-header + TCP header + segment payload, then the
            // completed checksum into the TCP header.
            let mut sum = 0u32;
            if request.ip_version == 4 {
                add_bytes(&mut sum, &frame[ip_off + 12..ip_off + 20]);
                sum += 6; // zero | protocol
                sum += (tcp_len + seg) as u32;
            } else {
                add_bytes(&mut sum, &frame[ip_off + 8..ip_off + 40]);
                let upper = (tcp_len + seg) as u32;
                sum += upper >> 16;
                sum += upper & 0xffff;
                sum += 6;
            }
            add_bytes(&mut sum, &frame[tcp_off..head]);
            add_bytes(&mut sum, &frame[head..frame_len]);
            let csum = finish(sum);
            frame[tcp_off + 16..tcp_off + 18].copy_from_slice(&csum.to_be_bytes());
        }
        emit(&scratch[..frame_len])?;
        *next += 1;
    }
    Ok(*next as usize == segments)
}

/// Compact TSO submission ticket ([`TsoQueue`] entry, contract §6).
/// Fixed 60-byte `#[repr(C)]` stride (the layout below carries 4 metadata
/// words, 8 packed fragment words and a 12-byte tail; the hard requirement
/// is a fixed stride, not a specific size): four metadata words, eight
/// packed fragment words, `nfrags` + pad, request id and producer epoch.
///
/// `meta[0]` packs `ip_version | protocol<<8 | ip_hdr_len<<16 |
/// tcp_hdr_len<<24`; `meta[1]` packs `mss | flags<<16`; `meta[2]` is
/// `head_len`; `meta[3]` is `payload_len`. `frags[i]` packs the pool buffer
/// id in the low 16 bits and the fragment length in the high 16 bits
/// (lengths are capped at 65535, fine for 2048-byte pool buffers).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TsoTicket {
    pub meta: [u32; 4],
    pub frags: [u32; 8],
    pub nfrags: u8,
    pub pad: [u8; 3],
    pub tso_id: u32,
    pub epoch: u32,
}

/// Unpacked ticket metadata plus `(id, length)` fragment pairs —
/// everything needed to rebuild a [`TsoRequest`] once the ids are
/// translated to device addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodedTicket {
    pub ip_version: u8,
    pub protocol: u8,
    pub ip_hdr_len: u8,
    pub tcp_hdr_len: u8,
    pub mss: u16,
    pub flags: u16,
    pub head_len: u32,
    pub payload_len: u32,
    pub tso_id: u32,
    pub epoch: u32,
    pub nfrags: u8,
    pub frags: [(u16, u16); TSO_MAX_FRAGS],
}

impl TsoTicket {
    /// Pack a request with per-fragment pool ids: ids replace the request's
    /// device addresses, lengths are preserved. `frag_ids` must hold
    /// exactly `nfrags` entries.
    pub fn from_request(request: &TsoRequest, frag_ids: &[u16]) -> Result<TsoTicket, TsoError> {
        let n = request.nfrags as usize;
        if request.nfrags < 1 || n > TSO_MAX_FRAGS || frag_ids.len() != n {
            return Err(TsoError::BadFragCount);
        }
        let mut ticket = TsoTicket {
            meta: [0; 4],
            frags: [0; 8],
            nfrags: request.nfrags,
            pad: [0; 3],
            tso_id: request.tso_id,
            epoch: 0,
        };
        ticket.meta[0] = u32::from(request.ip_version)
            | u32::from(request.protocol) << 8
            | u32::from(request.ip_hdr_len) << 16
            | u32::from(request.tcp_hdr_len) << 24;
        ticket.meta[1] = u32::from(request.mss) | u32::from(request.flags) << 16;
        ticket.meta[2] = request.head_len;
        ticket.meta[3] = request.payload_len;
        for i in 0..n {
            let length = request.frags[i].length;
            if length > u32::from(u16::MAX) {
                return Err(TsoError::BadFragLength);
            }
            ticket.frags[i] = u32::from(frag_ids[i]) | length << 16;
        }
        Ok(ticket)
    }

    /// Unpack a ticket, or `None` when `nfrags` is out of range.
    pub fn decode(&self) -> Option<DecodedTicket> {
        let n = self.nfrags as usize;
        if self.nfrags < 1 || n > TSO_MAX_FRAGS {
            return None;
        }
        let mut frags = [(0u16, 0u16); TSO_MAX_FRAGS];
        for (i, pair) in frags.iter_mut().enumerate() {
            if i < n {
                let word = self.frags[i];
                *pair = ((word & 0xffff) as u16, (word >> 16) as u16);
            }
        }
        Some(DecodedTicket {
            ip_version: self.meta[0] as u8,
            protocol: (self.meta[0] >> 8) as u8,
            ip_hdr_len: (self.meta[0] >> 16) as u8,
            tcp_hdr_len: (self.meta[0] >> 24) as u8,
            mss: self.meta[1] as u16,
            flags: (self.meta[1] >> 16) as u16,
            head_len: self.meta[2],
            payload_len: self.meta[3],
            tso_id: self.tso_id,
            epoch: self.epoch,
            nfrags: self.nfrags,
            frags,
        })
    }
}

/// Ticket queue geometry: power-of-two capacity, fixed 60-byte stride.
pub const TQ_COUNT: u32 = 64;
const TQ_ENTRIES: usize = 0x40;
/// Bytes of shared memory one ticket queue needs (header + entries).
pub const TQ_BYTES: usize = TQ_ENTRIES + TQ_COUNT as usize * core::mem::size_of::<TsoTicket>();

fn word(base: usize, offset: usize) -> &'static AtomicU32 {
    unsafe { &*((base as *const AtomicU32).add(offset / 4)) }
}

fn tq_head(base: usize) -> &'static AtomicU32 {
    word(base, 0)
}
fn tq_tail(base: usize) -> &'static AtomicU32 {
    word(base, 4)
}

/// Header of a bounded SPSC TSO ticket queue in shared memory, mirroring
/// `zc::RefQueue`: `head` is the producer cursor, `tail` the consumer
/// cursor; both are wrapping `u32` counters published with release/acquire
/// ordering. `Ok(false)` backpressures — the queue never blocks and never
/// drops a ticket silently.
pub struct TsoQueue;

impl TsoQueue {
    /// Push one ticket. `Ok(false)` means the queue is full (backpressure);
    /// `Err` means the shared cursors look corrupt.
    pub fn push(base: usize, ticket: &TsoTicket) -> Result<bool, TsoError> {
        let head = tq_head(base).load(Ordering::Relaxed);
        let tail = tq_tail(base).load(Ordering::Acquire);
        let pending = head.wrapping_sub(tail);
        if pending > TQ_COUNT {
            return Err(TsoError::Busy);
        }
        if pending == TQ_COUNT {
            return Ok(false);
        }
        let slot = base
            + TQ_ENTRIES
            + (head % TQ_COUNT) as usize * core::mem::size_of::<TsoTicket>();
        unsafe {
            core::ptr::write_volatile(slot as *mut TsoTicket, *ticket);
        }
        tq_head(base).store(head.wrapping_add(1), Ordering::Release);
        Ok(true)
    }

    /// Pop one ticket. `Ok(None)` means the queue is empty.
    pub fn pop(base: usize) -> Result<Option<TsoTicket>, TsoError> {
        let tail = tq_tail(base).load(Ordering::Relaxed);
        let head = tq_head(base).load(Ordering::Acquire);
        let pending = head.wrapping_sub(tail);
        if pending > TQ_COUNT {
            return Err(TsoError::Busy);
        }
        if pending == 0 {
            return Ok(None);
        }
        let slot = base
            + TQ_ENTRIES
            + (tail % TQ_COUNT) as usize * core::mem::size_of::<TsoTicket>();
        let ticket = unsafe { core::ptr::read_volatile(slot as *const TsoTicket) };
        tq_tail(base).store(tail.wrapping_add(1), Ordering::Release);
        Ok(Some(ticket))
    }

    /// Tickets currently queued.
    pub fn depth(base: usize) -> Result<u32, TsoError> {
        let head = tq_head(base).load(Ordering::Acquire);
        let tail = tq_tail(base).load(Ordering::Acquire);
        let pending = head.wrapping_sub(tail);
        if pending > TQ_COUNT {
            Err(TsoError::Busy)
        } else {
            Ok(pending)
        }
    }

    /// Producer cursor (tickets pushed since reset).
    pub fn produced(base: usize) -> u32 {
        tq_head(base).load(Ordering::Acquire)
    }

    /// Consumer cursor (tickets popped since reset).
    pub fn consumed(base: usize) -> u32 {
        tq_tail(base).load(Ordering::Acquire)
    }
}

/// Per-NIC TSO diagnostics counters (contract §7). Plain atomics, no
/// locks; counters saturate instead of wrapping.
pub struct TsoCounters {
    pub hw_completed: AtomicU32,
    pub requests: AtomicU32,
    pub hw_frames: AtomicU32,
    pub hw_segments_est: AtomicU32,
    pub fallback_frames: AtomicU32,
    pub fallback_segments: AtomicU32,
    pub invalid: AtomicU32,
    pub desc_errors: AtomicU32,
    pub completion_failures: AtomicU32,
    pub queue_drops: AtomicU32,
}

fn add(counter: &AtomicU32, by: u32) {
    if by == 1 {
        if counter.load(Ordering::Relaxed) != u32::MAX {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    } else if by != 0 {
        let value = counter.load(Ordering::Relaxed);
        counter.store(value.saturating_add(by), Ordering::Relaxed);
    }
}

impl TsoCounters {
    pub const fn new() -> Self {
        Self {
            hw_completed: AtomicU32::new(0),
            requests: AtomicU32::new(0),
            hw_frames: AtomicU32::new(0),
            hw_segments_est: AtomicU32::new(0),
            fallback_frames: AtomicU32::new(0),
            fallback_segments: AtomicU32::new(0),
            invalid: AtomicU32::new(0),
            desc_errors: AtomicU32::new(0),
            completion_failures: AtomicU32::new(0),
            queue_drops: AtomicU32::new(0),
        }
    }

    pub fn note_request(&self) {
        add(&self.requests, 1);
    }

    /// One hardware-submitted frame carrying `n_segments_est` estimated
    /// segments (`ceil(payload/mss)`).
    pub fn note_hw(&self, n_segments_est: u32) {
        add(&self.hw_frames, 1);
        add(&self.hw_segments_est, n_segments_est);
    }

    /// One fallback frame emitting `n_segments` software segments.
    pub fn note_fallback(&self, n_segments: u32) {
        add(&self.fallback_frames, 1);
        add(&self.fallback_segments, n_segments);
    }

    pub fn note_invalid(&self) {
        add(&self.invalid, 1);
    }

    pub fn note_desc_error(&self) {
        add(&self.desc_errors, 1);
    }

    pub fn note_completion_failure(&self) {
        add(&self.completion_failures, 1);
    }

    pub fn note_queue_drop(&self) {
        add(&self.queue_drops, 1);
    }

    /// Counters in contract §7 order: requests, hw_frames, hw_segments_est,
    /// fallback_frames, fallback_segments, invalid, desc_errors,
    /// completion_failures, queue_drops.
    pub fn snapshot(&self) -> [u32; 9] {
        [
            self.requests.load(Ordering::Relaxed),
            self.hw_frames.load(Ordering::Relaxed),
            self.hw_segments_est.load(Ordering::Relaxed),
            self.fallback_frames.load(Ordering::Relaxed),
            self.fallback_segments.load(Ordering::Relaxed),
            self.invalid.load(Ordering::Relaxed),
            self.desc_errors.load(Ordering::Relaxed),
            self.completion_failures.load(Ordering::Relaxed),
            self.queue_drops.load(Ordering::Relaxed),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    // 64-byte aligned backing storage so fragment physicals pass the
    // alignment rule and `copy_out` can read them as CPU addresses.
    #[repr(C, align(64))]
    struct Aligned {
        data: [u8; 8192],
    }

    fn buf64() -> Box<Aligned> {
        Box::new(Aligned { data: [0u8; 8192] })
    }

    fn zfrags() -> [TsoFrag; TSO_MAX_FRAGS] {
        [TsoFrag { physical: 0, length: 0 }; TSO_MAX_FRAGS]
    }

    fn request_v4(payload_len: u32, mss: u16) -> TsoRequest {
        TsoRequest {
            ip_version: 4,
            protocol: 6,
            ip_hdr_len: 20,
            tcp_hdr_len: 20,
            mss,
            flags: 0,
            head_len: 54,
            payload_len,
            tso_id: 7,
            nfrags: 1,
            reserved: [0; 3],
            frags: zfrags(),
        }
    }

    fn request_v6(payload_len: u32, mss: u16) -> TsoRequest {
        TsoRequest {
            ip_version: 6,
            protocol: 6,
            ip_hdr_len: 40,
            tcp_hdr_len: 20,
            mss,
            flags: 0,
            head_len: 74,
            payload_len,
            tso_id: 9,
            nfrags: 1,
            reserved: [0; 3],
            frags: zfrags(),
        }
    }

    /// Ethernet + IPv4 + TCP header template with known identifiers.
    fn v4_template() -> [u8; 54] {
        let mut h = [0u8; 54];
        h[0..6].copy_from_slice(&[0x02, 0, 0, 0, 0, 1]);
        h[6..12].copy_from_slice(&[0x02, 0, 0, 0, 0, 2]);
        h[12] = 0x08;
        h[13] = 0x00;
        h[14] = 0x45;
        h[15] = 0x00;
        h[18] = 0x12;
        h[19] = 0x34; // identification base 0x1234
        h[20] = 0x40;
        h[21] = 0x00; // DF
        h[22] = 64;
        h[23] = 6;
        h[26..30].copy_from_slice(&[192, 0, 2, 1]);
        h[30..34].copy_from_slice(&[198, 51, 100, 2]);
        h[34..36].copy_from_slice(&[0x30, 0x39]);
        h[36..38].copy_from_slice(&[0x01, 0xbb]);
        h[38..42].copy_from_slice(&[0x00, 0x00, 0x10, 0x00]); // seq base 0x1000
        h[46] = 0x50;
        h[47] = 0x10; // ACK
        h[48] = 0xff;
        h[49] = 0xff;
        h
    }

    /// Ethernet + IPv6 + TCP header template with known identifiers.
    fn v6_template() -> [u8; 74] {
        let mut h = [0u8; 74];
        h[0..6].copy_from_slice(&[0x02, 0, 0, 0, 0, 1]);
        h[6..12].copy_from_slice(&[0x02, 0, 0, 0, 0, 2]);
        h[12] = 0x86;
        h[13] = 0xdd;
        h[14] = 0x60;
        h[20] = 6;
        h[21] = 64;
        for i in 0..16 {
            h[22 + i] = 0xa0 + i as u8;
            h[38 + i] = 0xb0 + i as u8;
        }
        h[54..56].copy_from_slice(&[0x30, 0x39]);
        h[56..58].copy_from_slice(&[0x01, 0xbb]);
        h[58..62].copy_from_slice(&[0x00, 0x00, 0x10, 0x00]); // seq base 0x1000
        h[66] = 0x50;
        h[67] = 0x10;
        h[68] = 0xff;
        h[69] = 0xff;
        h
    }

    fn pattern(p: usize) -> u8 {
        (p as u8).wrapping_mul(31).wrapping_add(7)
    }

    // Independent 16-bit one's-complement sum: byte-oriented u64
    // accumulation (the library sums 16-bit words into u32), folded at the
    // end. Used to recompute every emitted checksum from scratch.
    fn sum16(data: &[u8]) -> u16 {
        let mut acc: u64 = 0;
        let mut i = 0;
        while i < data.len() {
            let hi = u64::from(data[i]);
            let lo = if i + 1 < data.len() { u64::from(data[i + 1]) } else { 0 };
            acc += (hi << 8) | lo;
            i += 2;
        }
        while acc > 0xffff {
            acc = (acc & 0xffff) + (acc >> 16);
        }
        acc as u16
    }

    fn csum16(data: &[u8]) -> u16 {
        !sum16(data)
    }

    // IPv4 pseudo-header + TCP + payload as one byte string (checksum field
    // untouched); its checksum field sits at byte 28.
    fn tcp_input_v4(frame: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&frame[26..34]);
        v.extend_from_slice(&[0, 6]);
        v.extend_from_slice(&((frame.len() - 34) as u16).to_be_bytes());
        v.extend_from_slice(&frame[34..]);
        v
    }

    // IPv6 pseudo-header + TCP + payload; its checksum field sits at byte 56.
    fn tcp_input_v6(frame: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&frame[22..54]);
        v.extend_from_slice(&((frame.len() - 54) as u32).to_be_bytes());
        v.extend_from_slice(&[0, 0, 0, 6]);
        v.extend_from_slice(&frame[54..]);
        v
    }

    fn verify_v4_frame(frame: &[u8]) {
        let mut ip = frame[14..34].to_vec();
        let ip_field = u16::from_be_bytes([ip[10], ip[11]]);
        ip[10] = 0;
        ip[11] = 0;
        assert_eq!(ip_field, csum16(&ip), "ipv4 header checksum");
        assert_eq!(sum16(&frame[14..34]), 0xffff, "ipv4 checksum property");

        let mut input = tcp_input_v4(frame);
        let tcp_field = u16::from_be_bytes([input[28], input[29]]);
        input[28] = 0;
        input[29] = 0;
        assert_eq!(tcp_field, csum16(&input), "tcp checksum");
        assert_eq!(sum16(&tcp_input_v4(frame)), 0xffff, "tcp checksum property");
    }

    fn verify_v6_frame(frame: &[u8]) {
        let mut input = tcp_input_v6(frame);
        let tcp_field = u16::from_be_bytes([input[56], input[57]]);
        input[56] = 0;
        input[57] = 0;
        assert_eq!(tcp_field, csum16(&input), "tcp checksum (v6 pseudo)");
        assert_eq!(sum16(&tcp_input_v6(frame)), 0xffff, "tcp checksum property");
    }

    fn emit_all(
        request: &TsoRequest,
    ) -> (u32, Vec<Vec<u8>>) {
        let mut scratch = [0u8; 4096];
        let mut frames: Vec<Vec<u8>> = Vec::new();
        let n = segment(request, &mut scratch, &mut |frame: &[u8]| {
            frames.push(frame.to_vec());
            Ok(())
        })
        .expect("segment");
        (n, frames)
    }

    // -- validation (contract §2) ------------------------------------------

    #[test]
    fn validate_accepts_valid_ipv4_and_ipv6_requests() {
        let mut v4 = request_v4(2500, 1000);
        v4.frags[0] = TsoFrag { physical: 0x1000, length: 2554 };
        assert_eq!(validate(&v4), Ok(()));
        let mut v6 = request_v6(1200, 500);
        v6.frags[0] = TsoFrag { physical: 0x2000, length: 1274 };
        assert_eq!(validate(&v6), Ok(()));
    }

    #[test]
    fn validate_rejects_bad_ip_version_protocol_and_header_lengths() {
        let mut r = request_v4(100, 1000);
        r.frags[0] = TsoFrag { physical: 0x1000, length: 154 };

        let mut bad = r;
        bad.ip_version = 5;
        assert_eq!(validate(&bad), Err(TsoError::BadIpVersion));

        let mut bad = r;
        bad.protocol = 17;
        assert_eq!(validate(&bad), Err(TsoError::NotTcp));

        let mut bad = r;
        bad.ip_hdr_len = 24;
        assert_eq!(validate(&bad), Err(TsoError::BadIpHdrLen));

        let mut bad = r;
        bad.ip_version = 6;
        bad.ip_hdr_len = 20;
        assert_eq!(validate(&bad), Err(TsoError::BadIpHdrLen));

        for tcp_hdr_len in [18u8, 62, 22] {
            let mut bad = r;
            bad.tcp_hdr_len = tcp_hdr_len;
            assert_eq!(validate(&bad), Err(TsoError::BadTcpHdrLen), "tcp_hdr_len");
        }
    }

    #[test]
    fn validate_rejects_ipv6_when_compiled_out() {
        let mut v6 = request_v6(100, 1000);
        v6.frags[0] = TsoFrag { physical: 0x1000, length: 174 };
        assert_eq!(validate_with(&v6, false), Err(TsoError::Unsupported));
        assert_eq!(validate_with(&v6, true), Ok(()));
    }

    #[test]
    fn validate_rejects_bad_head_len_mss_and_header_cap() {
        let mut r = request_v4(100, 1000);
        r.frags[0] = TsoFrag { physical: 0x1000, length: 154 };

        let mut bad = r;
        bad.head_len = 1200;
        assert_eq!(validate(&bad), Err(TsoError::BadHeader));

        let mut bad = r;
        bad.head_len = 53;
        assert_eq!(validate(&bad), Err(TsoError::BadHeadLen));

        let mut bad = r;
        bad.mss = 63;
        assert_eq!(validate(&bad), Err(TsoError::BadMss));

        let mut bad = r;
        bad.mss = 2048;
        assert_eq!(validate(&bad), Err(TsoError::BadMss));
    }

    #[test]
    fn validate_rejects_bad_payload_and_total_size() {
        let mut r = request_v4(100, 1000);
        r.frags[0] = TsoFrag { physical: 0x1000, length: 154 };

        let mut bad = r;
        bad.payload_len = 0;
        bad.frags[0].length = 54;
        assert_eq!(validate(&bad), Err(TsoError::BadPayload));

        let mut bad = r;
        bad.payload_len = 63947; // 54 + 63947 = 64001 > TSO_MAX_PAYLOAD
        bad.frags[0] = TsoFrag { physical: 0x1000, length: 54 };
        bad.frags[1] = TsoFrag { physical: 0x2000, length: 63947 };
        bad.nfrags = 2;
        assert_eq!(validate(&bad), Err(TsoError::Oversized));
    }

    #[test]
    fn validate_rejects_bad_fragment_rules() {
        let mut r = request_v4(100, 1000);
        r.frags[0] = TsoFrag { physical: 0x1000, length: 154 };

        let mut bad = r;
        bad.nfrags = 0;
        assert_eq!(validate(&bad), Err(TsoError::BadFragCount));

        let mut bad = r;
        bad.nfrags = 9;
        assert_eq!(validate(&bad), Err(TsoError::BadFragCount));

        let mut bad = r;
        bad.nfrags = 2;
        bad.frags[0] = TsoFrag { physical: 0x1000, length: 54 };
        bad.frags[1] = TsoFrag { physical: 0x2000, length: 0 };
        assert_eq!(validate(&bad), Err(TsoError::BadFragLength));

        let mut bad = r;
        bad.nfrags = 2;
        bad.frags[0] = TsoFrag { physical: 0x1000, length: 50 };
        bad.frags[1] = TsoFrag { physical: 0x2000, length: 104 };
        assert_eq!(validate(&bad), Err(TsoError::BadFragLength));

        let mut bad = r;
        bad.nfrags = 2;
        bad.frags[0] = TsoFrag { physical: 0x1000, length: 54 };
        bad.frags[1] = TsoFrag { physical: 0x2000, length: 99 };
        assert_eq!(validate(&bad), Err(TsoError::LengthSum));

        let mut bad = r;
        bad.frags[0] = TsoFrag { physical: 0x1008, length: 154 };
        assert_eq!(validate(&bad), Err(TsoError::Misaligned));
    }

    #[test]
    fn validate_rejects_zero_tso_id() {
        let mut r = request_v4(100, 1000);
        r.frags[0] = TsoFrag { physical: 0x1000, length: 154 };
        r.tso_id = 0;
        assert_eq!(validate(&r), Err(TsoError::ZeroId));
    }

    // -- software segmentation (contract §5) -------------------------------

    #[test]
    fn ipv4_segmentation_matches_hand_computed_vectors_and_checksums() {
        let mut buf = buf64();
        let template = v4_template();
        buf.data[..54].copy_from_slice(&template);
        for p in 0..2500usize {
            buf.data[54 + p] = pattern(p);
        }
        let mut request = request_v4(2500, 1000);
        request.frags[0] = TsoFrag {
            physical: buf.data.as_ptr() as u64,
            length: 2554,
        };

        let (n, frames) = emit_all(&request);
        assert_eq!(n, 3);
        // Hand-computed vectors: frame lengths, IP total lengths (20+20+seg),
        // IP ids 0x1234+k, TCP seqs 0x1000+k*1000, PSH on the last frame only.
        assert_eq!(frames.iter().map(Vec::len).collect::<Vec<_>>(), [1054, 1054, 554]);
        assert_eq!(
            frames.iter().map(|f| u16::from_be_bytes([f[16], f[17]])).collect::<Vec<_>>(),
            [1040u16, 1040, 540]
        );
        assert_eq!(
            frames.iter().map(|f| u16::from_be_bytes([f[18], f[19]])).collect::<Vec<_>>(),
            [0x1234u16, 0x1235, 0x1236]
        );
        assert_eq!(
            frames
                .iter()
                .map(|f| u32::from_be_bytes([f[38], f[39], f[40], f[41]]))
                .collect::<Vec<_>>(),
            [0x1000u32, 0x13e8, 0x17d0]
        );
        assert_eq!(
            frames.iter().map(|f| f[47] & 0x08 != 0).collect::<Vec<_>>(),
            [false, false, true]
        );
        for (k, frame) in frames.iter().enumerate() {
            verify_v4_frame(frame);
            let seg = core::cmp::min(1000, 2500 - k * 1000);
            for (j, byte) in frame[54..54 + seg].iter().enumerate() {
                assert_eq!(*byte, pattern(k * 1000 + j), "payload byte");
            }
        }
    }

    #[test]
    fn ipv4_odd_length_segments_checksum_correctly() {
        let mut buf = buf64();
        buf.data[..54].copy_from_slice(&v4_template());
        for p in 0..97usize {
            buf.data[54 + p] = pattern(p);
        }
        let mut request = request_v4(97, 64);
        request.frags[0] = TsoFrag {
            physical: buf.data.as_ptr() as u64,
            length: 151,
        };

        let (n, frames) = emit_all(&request);
        assert_eq!(n, 2);
        assert_eq!(frames.iter().map(Vec::len).collect::<Vec<_>>(), [118, 87]);
        // Second segment has a 33-byte odd payload; both checksums must be
        // complete and correct (the odd byte is padded with zero for the sum).
        for frame in &frames {
            verify_v4_frame(frame);
        }
        let tail: Vec<u8> = (64..97).map(pattern).collect();
        assert_eq!(frames[1][54..87], tail[..]);
    }

    #[test]
    fn ipv6_segmentation_updates_payload_length_and_tcp_checksum() {
        let mut buf = buf64();
        buf.data[..74].copy_from_slice(&v6_template());
        for p in 0..1200usize {
            buf.data[74 + p] = pattern(p);
        }
        let mut request = request_v6(1200, 500);
        request.frags[0] = TsoFrag {
            physical: buf.data.as_ptr() as u64,
            length: 1274,
        };

        let (n, frames) = emit_all(&request);
        assert_eq!(n, 3);
        assert_eq!(frames.iter().map(Vec::len).collect::<Vec<_>>(), [574, 574, 274]);
        // IPv6 payload_length = tcp_hdr_len + segment payload (20 + seg).
        assert_eq!(
            frames.iter().map(|f| u16::from_be_bytes([f[18], f[19]])).collect::<Vec<_>>(),
            [520u16, 520, 220]
        );
        assert_eq!(
            frames
                .iter()
                .map(|f| u32::from_be_bytes([f[58], f[59], f[60], f[61]]))
                .collect::<Vec<_>>(),
            [0x1000u32, 0x11f4, 0x13e8]
        );
        assert_eq!(
            frames.iter().map(|f| f[67] & 0x08 != 0).collect::<Vec<_>>(),
            [false, false, true]
        );
        for frame in &frames {
            verify_v6_frame(frame);
        }
    }

    #[test]
    fn segment_frame_sizes_respect_mtu_and_scratch_bounds() {
        assert!(!is_oversized(&request_v4(1460, 1000))); // 54 + 1460 = 1514
        assert!(is_oversized(&request_v4(1461, 1000)));

        let mut buf = buf64();
        buf.data[..54].copy_from_slice(&v4_template());
        for p in 0..1150usize {
            buf.data[54 + p] = pattern(p);
        }
        let mut request = request_v4(1150, 600);
        request.frags[0] = TsoFrag {
            physical: buf.data.as_ptr() as u64,
            length: 1204,
        };

        let (n, frames) = emit_all(&request);
        assert_eq!(n, 2);
        for frame in &frames {
            assert!(frame.len() <= 1514, "MTU-sized fallback frame");
            assert!(frame.len() <= 14 + 40 + 60 + 600, "scratch bound");
            assert!(frame.len() <= 2161, "maximal frame bound");
        }
    }

    #[test]
    fn segment_streams_payload_across_three_fragments() {
        // head_len=54, frags = [54+100 inline | 1000 | 50], payload_len=1150,
        // mss=600 -> segments of 600 and 550 bytes taken from the right
        // slices of the concatenated fragment stream.
        let mut buf = buf64();
        let template = v4_template();
        buf.data[..54].copy_from_slice(&template);
        for p in 0..100usize {
            buf.data[54 + p] = pattern(p);
        }
        for j in 0..1000usize {
            buf.data[2048 + j] = pattern(100 + j);
        }
        for j in 0..50usize {
            buf.data[4096 + j] = pattern(1100 + j);
        }
        let base = buf.data.as_ptr() as u64;
        let mut request = request_v4(1150, 600);
        request.nfrags = 3;
        request.frags[0] = TsoFrag { physical: base, length: 154 };
        request.frags[1] = TsoFrag { physical: base + 2048, length: 1000 };
        request.frags[2] = TsoFrag { physical: base + 4096, length: 50 };

        let (n, frames) = emit_all(&request);
        assert_eq!(n, 2);
        assert_eq!(frames[0].len(), 654);
        assert_eq!(frames[1].len(), 604);
        let mut expected: Vec<u8> = (0..600).map(pattern).collect();
        assert_eq!(frames[0][54..], expected[..]);
        expected = (600..1150).map(pattern).collect();
        assert_eq!(frames[1][54..], expected[..]);
        for frame in &frames {
            verify_v4_frame(frame);
        }
    }

    #[test]
    fn segment_propagates_emit_errors() {
        let mut buf = buf64();
        buf.data[..54].copy_from_slice(&v4_template());
        let mut request = request_v4(2500, 1000);
        request.frags[0] = TsoFrag {
            physical: buf.data.as_ptr() as u64,
            length: 2554,
        };
        let mut scratch = [0u8; 4096];
        let mut calls = 0;
        let result = segment(&request, &mut scratch, &mut |_frame: &[u8]| {
            calls += 1;
            if calls == 2 {
                Err(TsoError::Busy)
            } else {
                Ok(())
            }
        });
        assert_eq!(result, Err(TsoError::Busy));
        assert_eq!(calls, 2);
    }

    #[test]
    fn segment_rejects_empty_payload() {
        let mut request = request_v4(0, 1000);
        request.frags[0] = TsoFrag { physical: 0x1000, length: 54 };
        let mut scratch = [0u8; 4096];
        let mut frames = 0;
        let result = segment(&request, &mut scratch, &mut |_frame: &[u8]| {
            frames += 1;
            Ok(())
        });
        assert_eq!(result, Err(TsoError::BadPayload));
        assert_eq!(frames, 0);
    }

    // -- ticket + queue (contract §6) --------------------------------------

    #[test]
    fn ticket_from_request_and_decode_round_trip() {
        assert_eq!(core::mem::size_of::<TsoTicket>(), 60, "fixed stride");
        let mut r = request_v4(1150, 600);
        r.flags = 1;
        r.nfrags = 3;
        r.frags[0] = TsoFrag { physical: 0x1000, length: 154 };
        r.frags[1] = TsoFrag { physical: 0x2000, length: 1000 };
        r.frags[2] = TsoFrag { physical: 0x3000, length: 50 };

        let ticket = TsoTicket::from_request(&r, &[10, 20, 30]).expect("pack");
        let d = ticket.decode().expect("decode");
        assert_eq!(d.ip_version, 4);
        assert_eq!(d.protocol, 6);
        assert_eq!(d.ip_hdr_len, 20);
        assert_eq!(d.tcp_hdr_len, 20);
        assert_eq!(d.mss, 600);
        assert_eq!(d.flags, 1);
        assert_eq!(d.head_len, 54);
        assert_eq!(d.payload_len, 1150);
        assert_eq!(d.tso_id, 7);
        assert_eq!(d.epoch, 0);
        assert_eq!(d.nfrags, 3);
        assert_eq!(d.frags[..3], [(10u16, 154u16), (20, 1000), (30, 50)]);

        assert_eq!(TsoTicket::from_request(&r, &[10, 20]), Err(TsoError::BadFragCount));
        let mut heavy = r;
        heavy.frags[0].length = 70000;
        assert_eq!(
            TsoTicket::from_request(&heavy, &[10, 20, 30]),
            Err(TsoError::BadFragLength)
        );
        let mut bad = ticket;
        bad.nfrags = 0;
        assert_eq!(bad.decode(), None);
        bad.nfrags = 9;
        assert_eq!(bad.decode(), None);
    }

    #[test]
    fn ticket_queue_preserves_order_and_backpressures_across_wraparound() {
        let memory: Vec<AtomicU32> = (0..TQ_BYTES / 4).map(|_| AtomicU32::new(0)).collect();
        let base = memory.as_ptr() as usize;
        tq_head(base).store(u32::MAX - 2, Ordering::Relaxed);
        tq_tail(base).store(u32::MAX - 2, Ordering::Relaxed);
        for n in 0..TQ_COUNT {
            let mut ticket = TsoTicket {
                meta: [n, 0, 54, 100],
                frags: [n | 10 << 16; 8],
                nfrags: 1,
                pad: [0; 3],
                tso_id: 1000 + n,
                epoch: 7,
            };
            ticket.meta[0] = 4 | 6 << 8 | 20 << 16 | 20 << 24;
            assert_eq!(TsoQueue::push(base, &ticket), Ok(true));
        }
        let overflow = TsoTicket {
            meta: [0; 4],
            frags: [0; 8],
            nfrags: 1,
            pad: [0; 3],
            tso_id: 0,
            epoch: 0,
        };
        assert_eq!(TsoQueue::push(base, &overflow), Ok(false), "backpressure");
        for n in 0..TQ_COUNT {
            let ticket = TsoQueue::pop(base).unwrap().expect("entries in order");
            assert_eq!(ticket.tso_id, 1000 + n);
            assert_eq!(ticket.epoch, 7);
            assert_eq!(ticket.frags[0], n | 10 << 16);
            assert_eq!(ticket.decode().expect("decode").nfrags, 1);
        }
        assert_eq!(TsoQueue::pop(base), Ok(None));
        assert_eq!(TsoQueue::depth(base), Ok(0));
        assert_eq!(TsoQueue::produced(base).wrapping_sub(TsoQueue::consumed(base)), 0);
    }

    #[test]
    fn ticket_queue_rejects_corrupt_cursors() {
        let memory: Vec<AtomicU32> = (0..TQ_BYTES / 4).map(|_| AtomicU32::new(0)).collect();
        let base = memory.as_ptr() as usize;
        tq_head(base).store(5, Ordering::Relaxed);
        tq_tail(base).store(5 + TQ_COUNT + 1, Ordering::Relaxed);
        let ticket = TsoTicket {
            meta: [0; 4],
            frags: [0; 8],
            nfrags: 1,
            pad: [0; 3],
            tso_id: 1,
            epoch: 0,
        };
        assert_eq!(TsoQueue::push(base, &ticket), Err(TsoError::Busy));
        assert_eq!(TsoQueue::pop(base), Err(TsoError::Busy));
        assert_eq!(TsoQueue::depth(base), Err(TsoError::Busy));
    }

    // -- diagnostics (contract §7) -----------------------------------------

    #[test]
    fn counters_record_events_in_snapshot_order() {
        let counters = TsoCounters::new();
        counters.note_request();
        counters.note_request();
        counters.note_hw(3);
        counters.note_fallback(2);
        counters.note_fallback(4);
        counters.note_invalid();
        counters.note_desc_error();
        counters.note_completion_failure();
        counters.note_queue_drop();
        assert_eq!(counters.snapshot(), [2, 1, 3, 2, 6, 1, 1, 1, 1]);
        assert_eq!(TsoCounters::new().snapshot(), [0; 9]);
    }
}

/// Ref-group submission: a TSO request rides the existing bounded zero-copy
/// TX queues as a group of consecutive references (contract §6 worker path).
/// Exactly one `FLAG_TSO_FIRST` opens the group, zero or more
/// `FLAG_TSO_CONT` fragments follow, and exactly one `FLAG_TSO_LAST` closes
/// it. The first reference's `meta` packs the request metadata, each
/// reference's length is one fragment. Ordinary references (no TSO flag
/// bits) are untouched. Groups are contiguous because the queues are
/// single-producer FIFO; the driver PD is the sole consumer and the only
/// descriptor-ring owner, so no concurrent descriptor corruption is
/// possible.
pub const FLAG_TSO_FIRST: u16 = 0x0100;
pub const FLAG_TSO_CONT: u16 = 0x0200;
pub const FLAG_TSO_LAST: u16 = 0x0400;
pub const FLAG_TSO_GRO: u16 = 0x0800;
pub const GRO_SOFTWARE: u16 = 1;
pub const PROBE_SOFTWARE: u16 = 2;
pub const SOFTWARE_SEGMENTATION: u16 = GRO_SOFTWARE | PROBE_SOFTWARE;
pub const fn status_flags(ipv6: bool, completed: u32) -> u32 {
    let max = u32::MAX >> 2;
    let count = if completed > max {max} else {completed};
    (count << 2) | 2 | ipv6 as u32
}
pub const FLAG_TSO_MASK: u16 = FLAG_TSO_FIRST | FLAG_TSO_CONT | FLAG_TSO_LAST;

#[cfg(all(feature = "zc", target_os = "none"))]
pub struct QueuedGroup {
    pub fragments: u32,
    pub gmac_slots: u32,
}

#[cfg(all(feature = "zc", target_os = "none"))]
pub fn queued_group(queue: usize, first: crate::zc::Ref) -> Option<QueuedGroup> {
    if first.flags & crate::zc::FLAG_COPY != 0 || !(1..=crate::zc::BUFFER).contains(&usize::from(first.len)) {
        return None;
    }
    let meta = unpack_first_meta(first.meta)?;
    let head = 14 + if meta.ipv6 { 40 } else { 20 } + u32::from(meta.tcp_hdr_len);
    let first_payload = u32::from(first.len).checked_sub(head)?;
    let mut bytes = first_payload;
    let mut gmac_slots = 2 + first_payload.div_ceil(TSO_MAX_CHUNK);
    for offset in 1..TSO_MAX_FRAGS as u32 {
        let next = crate::zc::RefQueue::peek_at(queue, offset).ok()??;
        if next.epoch != first.epoch || next.flags & crate::zc::FLAG_COPY != 0
            || !(1..=crate::zc::BUFFER).contains(&usize::from(next.len))
        {
            return None;
        }
        bytes += u32::from(next.len);
        gmac_slots += u32::from(next.len).div_ceil(TSO_MAX_CHUNK);
        match next.flags & FLAG_TSO_MASK {
            kind if kind == FLAG_TSO_LAST || kind == (FLAG_TSO_CONT | FLAG_TSO_LAST) => {
                return (bytes != 0).then_some(QueuedGroup { fragments: offset+1, gmac_slots });
            }
            FLAG_TSO_CONT => {}
            _ => return None,
        }
    }
    None
}

pub fn gro_payload(frame: &[u8]) -> Option<u16> {
    let mss = frame.len().checked_sub(54)?;
    if !(usize::from(TSO_MIN_MSS)..=1460).contains(&mss)
        || frame[12..15] != [8, 0, 0x45] || frame[15] & 3 != 0
        || u16::from_be_bytes([frame[16], frame[17]]) as usize != 40 + mss
        || frame[20..22] != [0x40, 0] || frame[23] != 6
        || frame[46] != 0x50 || !matches!(frame[47], 0x10 | 0x18) || frame[52..54] != [0, 0]
    {
        return None;
    }
    Some(mss as u16)
}

pub fn gro_starts(frame: &[u8]) -> bool {
    frame.get(47) == Some(&0x10) && gro_payload(frame).is_some()
}

pub fn gro_pair(first: &[u8], last: &[u8]) -> Option<u16> {
    if last.get(47) != Some(&0x18) { return None; }
    gro_follow(first, last)
}

pub fn gro_follow(first: &[u8], last: &[u8]) -> Option<u16> {
    let mss = gro_payload(first)?;
    if gro_payload(last)? != mss || first[47] != 0x10
        || first[..18] != last[..18] || first[20..24] != last[20..24]
        || first[26..38] != last[26..38] || first[42..47] != last[42..47]
        || first[48..50] != last[48..50]
    {
        return None;
    }
    let id = |frame: &[u8]| u16::from_be_bytes([frame[18], frame[19]]);
    let seq = |frame: &[u8]| u32::from_be_bytes(frame[38..42].try_into().unwrap());
    (id(last) == id(first).wrapping_add(1)
        && seq(last) == seq(first).wrapping_add(u32::from(mss))).then_some(mss)
}

pub struct TcpAggregate<'a> {
    pub header: &'a [u8],
    pub payloads: [&'a [u8]; 2],
    pub mss: u16,
}

impl<'a> TcpAggregate<'a> {
    pub fn from_segments(first: &'a [u8], last: &'a [u8]) -> Option<Self> {
        let mss = gro_pair(first, last)?;
        Some(Self { header: &first[..54], payloads: [&first[54..], &last[54..]], mss })
    }
}

#[test]
fn gro_requires_matching_headers_and_exact_sequence_and_id_progression() {
    let mut first = [0; 118];
    first[12..15].copy_from_slice(&[8, 0, 0x45]);
    first[16..18].copy_from_slice(&104u16.to_be_bytes());
    first[18..20].copy_from_slice(&u16::MAX.to_be_bytes());
    first[20] = 0x40;
    first[22] = 63;
    first[23] = 6;
    first[38..42].copy_from_slice(&(u32::MAX-62).to_be_bytes());
    first[46] = 0x50;
    first[47] = 0x10;
    let mut last = first;
    last[18..20].fill(0);
    last[38..42].copy_from_slice(&1u32.to_be_bytes());
    last[47] = 0x18;
    assert_eq!(gro_pair(&first, &last), Some(64));
    let aggregate = TcpAggregate::from_segments(&first, &last).unwrap();
    assert_eq!(aggregate.header, &first[..54]);
    assert_eq!(aggregate.payloads, [&first[54..], &last[54..]]);
    assert_eq!(aggregate.mss, 64);
    for index in [15, 18, 20, 22, 29, 37, 41, 45, 46, 47, 49, 53] {
        let mut mismatch = last;
        mismatch[index] ^= 1;
        assert_eq!(gro_pair(&first, &mismatch), None, "index {index}");
    }
}

#[test]
fn gro_payload_fragments_segment_back_to_the_original_wire_frames() {
    #[repr(align(64))]
    struct Packet([u8; 128]);
    let mut first = Packet([0; 128]);
    let mut last = Packet([0; 128]);
    for (index, buffer) in [&mut first.0, &mut last.0].into_iter().enumerate() {
        buffer[12..15].copy_from_slice(&[8, 0, 0x45]);
        buffer[16..18].copy_from_slice(&104u16.to_be_bytes());
        buffer[18..20].copy_from_slice(&(19 + index as u16).to_be_bytes());
        buffer[20] = 0x40;
        buffer[22] = 63;
        buffer[23] = 6;
        buffer[26..34].copy_from_slice(&[10, 1, 1, 1, 10, 2, 2, 2]);
        buffer[38..42].copy_from_slice(&(42 + index as u32 * 64).to_be_bytes());
        buffer[46] = 0x50;
        buffer[47] = if index == 0 {0x10} else {0x18};
        buffer[54..118].fill(121 + index as u8);
        let mut ip = 0;
        add_bytes(&mut ip, &buffer[14..34]);
        buffer[24..26].copy_from_slice(&finish(ip).to_be_bytes());
        let mut tcp = 90;
        add_bytes(&mut tcp, &buffer[26..34]);
        add_bytes(&mut tcp, &buffer[34..118]);
        buffer[50..52].copy_from_slice(&finish(tcp).to_be_bytes());
    }
    assert_eq!(gro_pair(&first.0[..118], &last.0[..118]), Some(64));
    let originals = [first.0, last.0];
    last.0.copy_within(54..118, 0);
    let request = request_from_group(&GroupMeta {mss:64, ipv6:false, tcp_hdr_len:20},
        &[TsoFrag {physical:first.0.as_ptr() as u64, length:118},
          TsoFrag {physical:last.0.as_ptr() as u64, length:64}], 1).unwrap();
    let mut index = 0;
    assert_eq!(segment(&request, &mut [0; 128], &mut |frame| {
        assert_eq!(frame, &originals[index][..118]);
        index += 1;
        Ok(())
    }), Ok(2));
}

/// First-ref `meta`: mss (bits 10:0), IPv6 (bit 11), TCP header length in
/// 32-bit words (bits 15:12).
pub const fn pack_first_meta(mss: u16, ipv6: bool, tcp_hdr_len: u8) -> u32 {
    (mss as u32 & 0x7ff) | ((ipv6 as u32) << 11) | (((tcp_hdr_len / 4) as u32 & 0xf) << 12)
}

/// Decoded first-ref metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupMeta {
    pub mss: u16,
    pub ipv6: bool,
    pub tcp_hdr_len: u8,
}

pub fn unpack_first_meta(meta: u32) -> Option<GroupMeta> {
    let mss = (meta & 0x7ff) as u16;
    let ipv6 = meta & (1 << 11) != 0;
    let words = ((meta >> 12) & 0xf) as u8;
    if mss < TSO_MIN_MSS || mss > TSO_MAX_MSS || words < 5 || words > 15 {
        return None;
    }
    Some(GroupMeta { mss, ipv6, tcp_hdr_len: words * 4 })
}

/// One received reference of a TSO group, as carried on the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupRef {
    pub flags: u16,
    pub meta: u32,
    pub len: u16,
}

impl GroupRef {
    pub fn is_tso(self) -> bool {
        self.flags & FLAG_TSO_MASK != 0
    }
}

/// Collects consecutive references of one TSO group. The caller resolves
/// each reference to a device-visible fragment address and passes it with
/// the reference; `request` builds the validated [`TsoRequest`] once the
/// group is complete.
pub struct GroupCollector {
    frags: [TsoFrag; TSO_MAX_FRAGS],
    nfrags: u8,
    meta: Option<GroupMeta>,
    complete: bool,
    gro_software: bool,
}

impl GroupCollector {
    pub fn new() -> Self {
        Self { frags: [TsoFrag { physical: 0, length: 0 }; TSO_MAX_FRAGS], nfrags: 0,
               meta: None, complete: false, gro_software: false }
    }

    /// Feed one reference. `Ok(true)` when the group is complete,
    /// `Ok(false)` while still collecting. Any sequencing violation answers
    /// [`TsoError::BadFragCount`] and leaves the collector unusable.
    pub fn push(&mut self, g: GroupRef, physical: u64) -> Result<bool, TsoError> {
        if self.complete || self.nfrags as usize >= TSO_MAX_FRAGS {
            return Err(TsoError::BadFragCount);
        }
        let kind = g.flags & FLAG_TSO_MASK;
        if self.meta.is_none() {
            if kind != FLAG_TSO_FIRST || g.len as u32 > TSO_MAX_CHUNK {
                return Err(TsoError::BadFragCount);
            }
            self.meta = Some(unpack_first_meta(g.meta).ok_or(TsoError::BadMss)?);
            self.gro_software = g.flags & FLAG_TSO_GRO != 0
                && option_env!("R5S_GRO_TSO") != Some("1");
        } else if kind == FLAG_TSO_LAST || kind == (FLAG_TSO_CONT | FLAG_TSO_LAST) {
            self.complete = true;
        } else if kind != FLAG_TSO_CONT {
            return Err(TsoError::BadFragCount);
        }
        if physical & 63 != 0 {
            return Err(TsoError::Misaligned);
        }
        self.frags[self.nfrags as usize] = TsoFrag { physical, length: g.len as u32 };
        self.nfrags += 1;
        Ok(self.complete)
    }

    /// The collected group's first-ref metadata (present after the first push).
    pub fn meta(&self) -> Option<GroupMeta> {
        self.meta
    }

    /// Build and validate the completed request. `tso_id` is the driver's
    /// nonzero completion identifier.
    pub fn request(&self, tso_id: u32) -> Result<TsoRequest, TsoError> {
        if !self.complete {
            return Err(TsoError::BadFragCount);
        }
        let meta = self.meta.ok_or(TsoError::BadFragCount)?;
        let mut request = request_from_group(&meta, &self.frags[..self.nfrags as usize], tso_id)?;
        if self.gro_software { request.flags |= GRO_SOFTWARE; }
        Ok(request)
    }
}

/// Assemble a validated request from group metadata and resolved fragments.
pub fn request_from_group(meta: &GroupMeta, frags: &[TsoFrag], tso_id: u32)
    -> Result<TsoRequest, TsoError>
{
    if frags.is_empty() || frags.len() > TSO_MAX_FRAGS {
        return Err(TsoError::BadFragCount);
    }
    let ip_hdr_len: u32 = if meta.ipv6 { 40 } else { 20 };
    let head_len = 14 + ip_hdr_len + meta.tcp_hdr_len as u32;
    let total: u32 = frags.iter().map(|f| f.length).sum();
    let payload_len = total.checked_sub(head_len).ok_or(TsoError::LengthSum)?;
    let mut request = TsoRequest {
        ip_version: if meta.ipv6 { 6 } else { 4 },
        protocol: 6,
        ip_hdr_len: ip_hdr_len as u8,
        tcp_hdr_len: meta.tcp_hdr_len,
        mss: meta.mss,
        flags: 0,
        head_len,
        payload_len,
        tso_id,
        nfrags: frags.len() as u8,
        reserved: [0; 3],
        frags: [TsoFrag { physical: 0, length: 0 }; TSO_MAX_FRAGS],
    };
    request.frags[..frags.len()].copy_from_slice(frags);
    validate(&request)?;
    Ok(request)
}

#[cfg(test)]
mod refgroup_tests {
    use super::*;

    fn frag(physical: u64, length: u32) -> TsoFrag {
        TsoFrag { physical, length }
    }

    #[test]
    fn meta_round_trips_and_rejects_out_of_range() {
        for (mss, ipv6, tcp) in [(64u16, false, 20u8), (1460, true, 60), (2047, false, 40)] {
            let packed = pack_first_meta(mss, ipv6, tcp);
            assert_eq!(unpack_first_meta(packed),
                       Some(GroupMeta { mss, ipv6, tcp_hdr_len: tcp }));
        }
        assert_eq!(unpack_first_meta(pack_first_meta(63, false, 20)), None);
        assert_eq!(unpack_first_meta(2048 & 0x7ff), None);
        assert_eq!(unpack_first_meta(pack_first_meta(1460, false, 16)), None);
    }

    #[test]
    fn collector_assembles_a_two_fragment_group() {
        let mut c = GroupCollector::new();
        let first = GroupRef { flags: FLAG_TSO_FIRST,
                               meta: pack_first_meta(1460, false, 20), len: 54 + 100 };
        assert_eq!(c.push(first, 0x8200_0000), Ok(false));
        let last = GroupRef { flags: FLAG_TSO_CONT | FLAG_TSO_LAST, meta: 0, len: 1150 };
        assert_eq!(c.push(last, 0x8200_0800), Ok(true));
        let request = c.request(7).unwrap();
        assert_eq!(request.ip_version, 4);
        assert_eq!(request.head_len, 54);
        assert_eq!(request.payload_len, 1250);
        assert_eq!(request.mss, 1460);
        assert_eq!(&request.frags[..2], &[frag(0x8200_0000, 154), frag(0x8200_0800, 1150)]);
    }

    #[test]
    fn collector_rejects_bad_sequences() {
        let first = GroupRef { flags: FLAG_TSO_FIRST,
                               meta: pack_first_meta(1460, false, 20), len: 54 };
        // CONT before FIRST.
        let mut c = GroupCollector::new();
        assert_eq!(c.push(GroupRef { flags: FLAG_TSO_CONT, meta: 0, len: 100 }, 0x8200_0000),
                   Err(TsoError::BadFragCount));
        // FIRST again mid-group.
        let mut c = GroupCollector::new();
        assert_eq!(c.push(first, 0x8200_0000), Ok(false));
        assert_eq!(c.push(first, 0x8200_0800), Err(TsoError::BadFragCount));
        // Missing LAST: request() refuses.
        let mut c = GroupCollector::new();
        assert_eq!(c.push(first, 0x8200_0000), Ok(false));
        assert_eq!(c.request(1), Err(TsoError::BadFragCount));
        // Misaligned fragment address.
        let mut c = GroupCollector::new();
        assert_eq!(c.push(first, 0x8200_0020), Err(TsoError::Misaligned));
        // More than TSO_MAX_FRAGS fragments.
        let mut c = GroupCollector::new();
        assert_eq!(c.push(first, 0x8200_0000), Ok(false));
        for i in 1..TSO_MAX_FRAGS as u16 {
            let cont = GroupRef { flags: FLAG_TSO_CONT, meta: 0, len: 100 };
            assert_eq!(c.push(cont, 0x8200_0000 + u64::from(i) * 2048), Ok(false));
        }
        let overflow = GroupRef { flags: FLAG_TSO_LAST, meta: 0, len: 100 };
        assert_eq!(c.push(overflow, 0x8202_0000), Err(TsoError::BadFragCount));
    }

    #[test]
    fn group_request_matches_direct_validation() {
        let meta = GroupMeta { mss: 1460, ipv6: false, tcp_hdr_len: 20 };
        let frags = [frag(0x8200_0000, 54 + 1460), frag(0x8200_0800, 1460)];
        let request = request_from_group(&meta, &frags, 9).unwrap();
        assert_eq!(request.payload_len, 2920);
        assert_eq!(request.tso_id, 9);
        // A zero-payload group is not a valid oversized request.
        let bad = [frag(0x8200_0000, 54)];
        assert_eq!(request_from_group(&meta, &bad, 1), Err(TsoError::BadPayload));
    }
}
