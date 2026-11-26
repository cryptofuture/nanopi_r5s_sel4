//! Shared zero-copy DMA packet pool: ownership state machine, bounded
//! reference queues and per-device DMA address translation.
//!
//! The pool holds `BUFFERS` fixed 2048-byte frames at physically contiguous,
//! 2048-aligned addresses. Payload bytes live only in the pool: every
//! dataplane queue carries a 16-byte [`Ref`] instead of a copied frame. A
//! buffer moves `FREE -> RX_DMA -> CPU -> TX_DMA -> FREE`; each transition is a
//! compare-and-swap on its ownership word and a mismatching transition is
//! counted and refused, never forced. Generations advance on allocation so a
//! reference to a previous life is rejected at release.
//!
//! DMA addresses are never assumed equal to CPU physical addresses. GMAC0
//! programs 32-bit low physical identity addresses; the RTL8125 controllers
//! reach memory only through their PCIe inbound windows. [`Window::translate`]
//! enforces bounds and records mapping failures. The default pool is Normal non-cacheable.
//! The write-back profile adds payload maintenance at DMA ownership boundaries.
//!
//! Host tests model the protocol with separate CPU and device views; they do
//! not establish hardware cache visibility or PCIe translation.

#![allow(dead_code)]

use core::sync::atomic::{AtomicU32, Ordering};

/// Payload capacity of one pool buffer. Holds a 1514-byte frame plus slack.
pub const BUFFER: usize = 2048;
pub const WIDE_POOL: bool = matches!(option_env!("R5S_B51_RING"), Some(value) if matches!(value.as_bytes(), b"256" | b"512"));
/// Total pool buffers. `POOL_BYTES` is the Microkit region size.
pub const LARGE_GRO: bool = matches!(option_env!("R5S_GRO_TSO"), Some(value) if matches!(value.as_bytes(), b"1"));
const SHARED_RESERVE: bool = WIDE_POOL
    && matches!(option_env!("R5S_ZC_POOL_BUFFERS"), Some(value) if matches!(value.as_bytes(), b"1024"));
pub const BUFFERS: usize = if WIDE_POOL && !SHARED_RESERVE { 512 } else { 1024 };
pub const POOL_BYTES: usize = BUFFERS * BUFFER;
/// Size of the metadata region holding ownership words, free lists, counters
/// and the driver-facing reference queues.
pub const META_BYTES: usize = 0x4000;

// Fixed virtual addresses of the shared regions in every protection domain
// that takes part in zero-copy forwarding. The pool is Normal non-cacheable.
pub const POOL_VA: usize = 0x6600_0000;
pub const META_VA: usize = 0x6680_0000;
pub const LINKS_VA: usize = 0x6690_0000;
pub const WAN_CONTROL_TX: usize = 0x66a1_0000;
pub const WAN_CONTROL_TX_CHANNEL: usize = 16;
pub const WAN_NEIGHBOR_TX: usize = 0x66a1_4000;
pub const WAN_NEIGHBOR_TX_CHANNEL: usize = 17;
pub const fn lan_control_tx(port: usize) -> usize { 0x66f0_0000 + port * 0x4000 }
// CPU physical address of the pool. GMAC0 programs this directly (32-bit
// physical identity); the RTL8125 windows translate it per device.
pub const WIDEBACK: bool = matches!(option_env!("R5S_ZC_CACHE"), Some(value) if matches!(value.as_bytes(), b"writeback"))
    && matches!(option_env!("R5S_B51_RING"), Some(value) if matches!(value.as_bytes(), b"256" | b"512"));
/// The 2 MiB shared-pool placement at 0x24600000 (lan2 private at 0x24800000).
/// Placement follows the buffer count, not the cache policy: a NC payload A/B
/// keeps identical physical addresses and differs only in mapping attributes.
/// Wide rings without the 1024-buffer expansion keep the 1 MiB pool at
/// 0x24500000.
pub const WIDE_PLACED: bool = WIDE_POOL && (SHARED_RESERVE || WIDEBACK);
pub const POOL_PA: u64 = match option_env!("R5S_B51_RING") {
    _ if WIDE_PLACED => 0x2460_0000,
    Some(value) if matches!(value.as_bytes(), b"256" | b"512") => 0x2450_0000,
    _ => 0x2440_0000,
};

// Ownership states. Only these four exist; there is no forced release.
pub const FREE: u32 = 0;
pub const RX_DMA: u32 = 1;
pub const CPU: u32 = 2;
pub const TX_DMA: u32 = 3;

/// Buffer-index width of [`Ref::handle`]; the generation takes the rest of the
/// 32-bit handle and the top 24 bits of the ownership word.
const ID_BITS: u32 = (BUFFERS as u32).trailing_zeros();
const GEN_MASK: u32 = u32::MAX >> ID_BITS;
const ID_MASK: u32 = (BUFFERS as u32) - 1;

// Free-list partitions give each RX driver a quota so one port cannot exhaust
// another. A buffer stays in its partition after forwarding; only its owning
// driver allocates, any driver may release. Quotas scale with the pool.
const QUOTA: u32 = (BUFFERS as u32) / 512;
pub const PARTITION_WAN: usize = 0;
pub const PARTITION_LAN1: usize = 1;
pub const PARTITION_LAN2: usize = 2;
pub const PARTITIONS: usize = 3;
const FREE_LISTS: usize = PARTITIONS + SHARED_RESERVE as usize;
const PARTITION_BOUNDS: [(u32, u32); 4] = if SHARED_RESERVE {
    let wan = if matches!(option_env!("R5S_GMAC_RING"), Some(v) if matches!(v.as_bytes(), b"256")) {288} else {160};
    [(0, wan), (wan, wan+304), (wan+304, wan+608), (wan+608, 1024)]
} else {
    [(0, 160 * QUOTA), (160 * QUOTA, 336 * QUOTA), (336 * QUOTA, BUFFERS as u32), (0, 0)]
};
pub const C_PORT_BORROW: usize = 51;
pub const C_GRO_SEGMENTS: usize = 54;
pub const C_GMAC_ROUTE_BLOCKED: usize = 55;
pub const C_GMAC_STALE: usize = 56;
pub const C_GMAC_GROUP_DEFERRED: usize = 57;
pub const C_GMAC_TX_PEAK: usize = 58;

// Metadata layout, offsets from the metadata region base. All access goes
// through the helpers below; the layout tracks `BUFFERS` (512 or 1024).
const OWNERSHIP: usize = 0x0000; // BUFFERS * AtomicU32
const FREE_WORDS: usize = BUFFERS / 32;
const FREE_BITS: usize = OWNERSHIP + BUFFERS * 4; // PARTITIONS * FREE_WORDS * AtomicU32
const IN_USE: usize = FREE_BITS + FREE_LISTS * FREE_WORDS * 4; // [3] current, [3] peak
const INIT: usize = IN_USE + FREE_LISTS * 2 * 4; // one-time free-list initialiser state
const COUNTERS: usize = INIT + 4; // 64 * AtomicU32
// Per-(worker, client) fallback-token tallies: armed by the worker on each
// download-fallback send, retired exactly once when that send is finished.
pub const TOKEN_CLIENTS: usize = 256;
const TOKEN_BYTES: usize = 4 * TOKEN_CLIENTS * 4;
const TOKEN_ARMED: usize = COUNTERS + 64 * 4;
const TOKEN_RETIRED: usize = TOKEN_ARMED + TOKEN_BYTES;
const WAN_DMA_COMPLETED: usize = META_BYTES - 16;
const _: () = assert!(TOKEN_RETIRED + TOKEN_BYTES <= WAN_DMA_COMPLETED);

// Counter indices.
pub const C_RX_PUBLISHED: usize = 0;
pub const C_TX_POSTED: usize = 1;
pub const C_TX_COMPLETED: usize = 2;
pub const C_FORWARDED: usize = 3;
pub const C_DROPPED: usize = 4;
pub const C_COPIED_PACKETS: usize = 5;
pub const C_COPIED_BYTES: usize = 6;
pub const C_QUEUE_DROPS: usize = 7;
pub const C_MAPPING_FAILURES: usize = 8;
pub const C_OWNERSHIP_ERRORS: usize = 9;
pub const C_RX_RECYCLED: usize = 10;
pub const C_ALLOC_FAILED: usize = 11;
pub const C_FALLBACK_POOL: usize = 12;
pub const C_FALLBACK_MAPPING: usize = 13;
pub const C_FALLBACK_LAYOUT: usize = 14;
pub const C_FALLBACK_QUEUE: usize = 15;
pub const C_PEAK_OCCUPANCY: usize = 16;
pub const C_RX_DROPPED: usize = 17;
pub const C_GRO_ELIGIBLE: usize = 18;
pub const C_GRO_GROUPS: usize = 19;
pub const C_GRO_INCOMPATIBLE: usize = 20;
pub const C_GRO_FALLBACK: usize = 21;
pub const C_GRO_SW_SEGMENTS: usize = 22;
pub const C_GRO_FLUSH: usize = 23;
pub const C_GRO_TIMEOUT: usize = 24;
pub const C_GRO_WORKER_PAIRS: usize = 25;
pub const C_GRO_DRIVER_PAIRS: usize = 26;

pub const C_PORT_ALLOC_FAILED: usize = 32;
pub const C_PORT_RX_COPY: usize = 35;
pub const C_PORT_RX_COPY_BYTES: usize = 38;

pub fn count_rx_copy(partition: usize, length: usize) {
    count(META_VA, C_PORT_RX_COPY + partition);
    add(counter(META_VA, C_PORT_RX_COPY_BYTES + partition), length as u32);
}

// Link reference queues live in the `zc_links` region (one SPSC queue per
// producer/consumer pair). Worker request/result queues reuse [`RefQueue`]
// against the per-worker regions instead.
pub const LINK_STRIDE: usize = 0x800;
pub const QUEUES: usize = 8;
pub const Q_GMAC_RX: usize = 0; // gmac driver -> wan app
pub const Q_APP_FWD: usize = 1; // wan app -> logical_lan (forwarded RX)
pub const Q_APP_TX: usize = 2; // wan app -> gmac driver (egress)
pub const Q_WAN_EG: usize = 3; // logical_lan -> wan app (egress)
pub const Q_LAN1_RX: usize = 4; // lan1 -> logical_lan
pub const Q_LAN1_TX: usize = 5; // logical_lan -> lan1
pub const Q_LAN2_RX: usize = 6; // lan2 -> logical_lan
pub const Q_LAN2_TX: usize = 7; // logical_lan -> lan2

/// Reference queue geometry: header then `COUNT` packed entries.
pub const RQ_COUNT: u32 = 64;
const PADDED: bool = match option_env!("R5S_QUEUE_PAD") {
    Some(value) => matches!(value.as_bytes(), b"1"),
    None => false,
};
const RQ_TAIL: usize = if PADDED {64}else{4};
const RQ_WAKE: usize = if PADDED {128}else{8};
const RQ_ENTRIES: usize = if PADDED {192}else{64};
pub const RQ_BYTES: usize = RQ_ENTRIES + RQ_COUNT as usize * core::mem::size_of::<Ref>();

/// A pool reference. `handle` packs the buffer index (low 9 bits) and its
/// allocation generation; `meta` carries the RX-verified transport protocol
/// (6, 17, or 0) exactly like the copy path's queue metadata.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ref {
    pub handle: u32,
    pub len: u16,
    pub flags: u16,
    pub epoch: u32,
    pub meta: u32,
}

pub const FLAG_WAN: u16 = 1;
pub const FLAG_TTL: u16 = 2;
pub const FLAG_COPY: u16 = 4;
pub const FLAG_NAT_VALIDATED: u16 = 8;
pub const LAN_INGRESS_SHIFT: u32 = 4;
pub const LAN_INGRESS_MASK: u16 = 3 << LAN_INGRESS_SHIFT;
pub const FLAG_SOURCE_LEARN: u16 = 1 << 6;
/// Marker/ref carries a download-fallback return token in [`Ref::meta`]
/// (`worker << 8 | client`, see [`arm_lan_token`]).
pub const FLAG_LAN_TOKEN: u16 = 1 << 7;
/// Payload is finalised on a worker core (L2/checksum seed/padding written)
/// and cache-maintained for TX. Honoured by both the GMAC submit path and the
/// b51 LAN TX path; each reference has exactly one TX consumer.
pub const FLAG_WAN_READY: u16 = 1 << 14;
/// Checksum fields were seeded for hardware insertion along with
/// [`FLAG_WAN_READY`]; the TX driver derives its insertion options read-only.
pub const FLAG_WAN_TRANSPORT_READY: u16 = 1 << 15;
pub const FLAG_GRO_FIRST: u16 = 1 << 12;
pub const FLAG_GRO_LAST: u16 = 1 << 13;
pub const FLAG_GRO_MASK: u16 = FLAG_GRO_FIRST | FLAG_GRO_LAST;
pub const TX_CHECKSUM_REQUESTED: bool = match option_env!("R5S_ZC_TX_CHECKSUM") {
    Some(value) => matches!(value.as_bytes(), b"1"),
    None => false,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxChecksum {
    None,
    Ipv4,
    Tcp,
    Udp,
}

pub fn nat_tx_checksum_kind(frame: &[u8], flags: u16) -> TxChecksum {
    if flags & FLAG_NAT_VALIDATED == 0 || frame.len() < 34
        || frame[12..15] != [8, 0, 0x45] || frame[20] & 0xbf != 0 || frame[21] != 0
    {
        return TxChecksum::None;
    }
    let ip_len = usize::from(u16::from_be_bytes([frame[16], frame[17]]));
    if !(20..=frame.len() - 14).contains(&ip_len) {
        return TxChecksum::None;
    }
    let transport_len = ip_len - 20;
    match frame[23] {
        6 if transport_len >= 20
            && (20..=transport_len).contains(&(usize::from(frame[46] >> 4) * 4)) =>
            TxChecksum::Tcp,
        17 if transport_len >= 8 && usize::from(u16::from_be_bytes([frame[38], frame[39]])) == transport_len => {
            if frame[40..42] == [0, 0] { TxChecksum::Ipv4 } else { TxChecksum::Udp }
        }
        6 | 17 => TxChecksum::None,
        _ => TxChecksum::Ipv4,
    }
}

pub fn prepare_nat_tx_checksum(frame: &mut [u8], flags: u16, pseudo_header_seed: bool) -> TxChecksum {
    let mode = nat_tx_checksum_kind(frame, flags);
    let offset = match mode {
        TxChecksum::None => return mode,
        TxChecksum::Ipv4 => 0,
        TxChecksum::Tcp => 50,
        TxChecksum::Udp => 40,
    };
    frame[24..26].fill(0);
    if offset != 0 {
        let seed = if pseudo_header_seed {
            let ip_len = u16::from_be_bytes([frame[16], frame[17]]);
            let mut sum = u32::from(ip_len - 20) + u32::from(frame[23]);
            for pair in frame[26..34].chunks_exact(2) {
                sum += u32::from(u16::from_be_bytes([pair[0], pair[1]]));
            }
            sum = (sum & 0xffff) + (sum >> 16);
            ((sum & 0xffff) + (sum >> 16)) as u16
        } else { 0 };
        frame[offset..offset + 2].copy_from_slice(&seed.to_be_bytes());
    }
    mode
}

pub fn handle(id: u32, generation: u32) -> u32 {
    (generation & GEN_MASK) << ID_BITS | (id & ID_MASK)
}
pub fn handle_id(h: u32) -> u32 {
    h & ID_MASK
}
pub fn handle_generation(h: u32) -> u32 {
    (h >> ID_BITS) & GEN_MASK
}

impl Ref {
    pub fn new(id: u32, generation: u32) -> Self {
        Self { handle: handle(id, generation), len: 0, flags: 0, epoch: 0, meta: 0 }
    }
    pub fn id(&self) -> u32 {
        handle_id(self.handle)
    }
    pub fn generation(&self) -> u32 {
        handle_generation(self.handle)
    }
}

fn word(base: usize, offset: usize) -> &'static AtomicU32 {
    unsafe { &*((base as *const AtomicU32).add(offset / 4)) }
}

fn ownership(meta: usize, id: u32) -> &'static AtomicU32 {
    word(meta + OWNERSHIP, (id as usize) * 4)
}

fn owner_word(state: u32, generation: u32) -> u32 {
    (generation & GEN_MASK) << 8 | (state & 0xff)
}
fn owner_state(value: u32) -> u32 {
    value & 0xff
}
fn owner_generation(value: u32) -> u32 {
    (value >> 8) & GEN_MASK
}

pub fn counter(meta: usize, index: usize) -> &'static AtomicU32 {
    word(meta + COUNTERS, index * 4)
}

// Dataplane counters used to take a shared-meta atomic per event, ping-ponging
// the counter cache line across cores. Events now land in this PD-private
// staging array and reach shared meta on flush_counts (called at the end of
// every service/notification pass).
const PENDING_SLOTS: usize = 64;
static PENDING: [AtomicU32; PENDING_SLOTS] = [const { AtomicU32::new(0) }; PENDING_SLOTS];

/// Count a dataplane event into this PD's accumulator. Counters saturate
/// instead of wrapping; publish with [`flush_counts`].
pub fn count(meta: usize, index: usize) {
    if index < PENDING_SLOTS {
        PENDING[index].fetch_add(1, Ordering::Relaxed);
    } else {
        add(counter(meta, index), 1);
    }
}

/// Accumulate `n` dataplane events of one kind into this PD's accumulator.
pub fn count_n(meta: usize, index: usize, n: u32) {
    if n == 0 {
        return;
    }
    if index < PENDING_SLOTS {
        PENDING[index].fetch_add(n, Ordering::Relaxed);
    } else {
        add(counter(meta, index), n);
    }
}

/// Publish this PD's accumulated counter events to shared meta. Call once at
/// the end of each service pass.
pub fn flush_counts(meta: usize) {
    for (index, cell) in PENDING.iter().enumerate() {
        let n = cell.swap(0, Ordering::Relaxed);
        if n != 0 {
            add(counter(meta, index), n);
        }
    }
}

/// Accumulate copied payload bytes. Saturates instead of wrapping.
pub fn add_copied_bytes(meta: usize, bytes: u32) {
    let c = counter(meta, C_COPIED_BYTES);
    add(c, bytes);
}

fn add(counter: &AtomicU32, amount: u32) {
    let _ = counter.try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        (value != u32::MAX).then(|| value.saturating_add(amount))
    });
}

fn note_ownership_error(meta: usize) {
    count(meta, C_OWNERSHIP_ERRORS);
}

// ---------------------------------------------------------------------------
// Ownership state machine
// ---------------------------------------------------------------------------

/// Read `(state, generation)` of one buffer.
pub fn status(meta: usize, id: u32) -> (u32, u32) {
    let value = ownership(meta, id).load(Ordering::Acquire);
    (owner_state(value), owner_generation(value))
}

fn transition(meta: usize, handle: u32, from: u32, to: u32) -> Result<u32, ()> {
    let id = handle_id(handle);
    if id >= BUFFERS as u32 {
        note_ownership_error(meta);
        return Err(());
    }
    let generation = handle_generation(handle);
    let cell = ownership(meta, id);
    let expected = owner_word(from, generation);
    let next = owner_word(to, generation);
    match cell.compare_exchange(expected, next, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => Ok(generation),
        Err(_) => {
            note_ownership_error(meta);
            Err(())
        }
    }
}

/// Allocate from a partition: `FREE -> RX_DMA`, advancing the generation so a
/// reference to the previous life cannot release the new one.
pub fn alloc(meta: usize, partition: usize) -> Option<Ref> {
    if partition >= PARTITIONS || !ready(meta) {
        if partition >= PARTITIONS {
            note_ownership_error(meta);
        } else {
            count(meta, C_PORT_ALLOC_FAILED + partition);
            count(meta, C_ALLOC_FAILED);
            count(meta, C_FALLBACK_POOL);
        }
        return None;
    }
    if let Some(reference) = alloc_from(meta, partition) {
        return Some(reference);
    }
    if SHARED_RESERVE && let Some(reference) = alloc_from(meta, PARTITIONS) {
        count(meta, C_PORT_BORROW + partition);
        return Some(reference);
    }
    count(meta, C_PORT_ALLOC_FAILED + partition);
    count(meta, C_ALLOC_FAILED);
    count(meta, C_FALLBACK_POOL);
    None
}

fn alloc_from(meta: usize, partition: usize) -> Option<Ref> {
    let (first, end) = PARTITION_BOUNDS[partition];
    let words = free_words(meta, partition);
    for index in first / 32..=(end - 1) / 32 {
        let bitmap = &words[index as usize];
        let mut available = bitmap.load(Ordering::Acquire);
        let id = loop {
            if available == 0 {
                break None;
            }
            let bit = available & available.wrapping_neg();
            match bitmap.compare_exchange_weak(
                available, available & !bit, Ordering::AcqRel, Ordering::Acquire,
            ) {
                Ok(_) => break Some(index * 32 + bit.trailing_zeros()),
                Err(current) => available = current,
            }
        };
        let Some(id) = id else { continue };
        let bit = 1 << (id % 32);
        // The bit was ours; the buffer must still be FREE.
        let cell = ownership(meta, id);
        let value = cell.load(Ordering::Acquire);
        if owner_state(value) != FREE {
            // Never reallocate a live buffer: restore the bit and fail.
            words[(id / 32) as usize].fetch_or(bit, Ordering::Release);
            note_ownership_error(meta);
            return None;
        }
        let generation = owner_generation(value).wrapping_add(1) & GEN_MASK;
        if cell
            .compare_exchange(
                owner_word(FREE, owner_generation(value)),
                owner_word(RX_DMA, generation),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            words[(id / 32) as usize].fetch_or(bit, Ordering::Release);
            note_ownership_error(meta);
            return None;
        }
        use_growth(meta, partition, true);
        return Some(Ref::new(id, generation));
    }
    None
}

/// The RX driver verified a completion: `RX_DMA -> CPU`.
pub fn publish(meta: usize, r: &Ref) -> Result<(), ()> {
    let result = transition(meta, r.handle, RX_DMA, CPU);
    if result.is_ok() {
        count(meta, C_RX_PUBLISHED);
    }
    result.map(|_| ())
}

/// The RX driver discards a completed frame without forwarding:
/// `RX_DMA -> FREE`.
pub fn recycle(meta: usize, r: &Ref) -> Result<(), ()> {
    let result = transition(meta, r.handle, RX_DMA, FREE);
    if result.is_ok() {
        release_free(meta, handle_id(r.handle));
        count(meta, C_RX_RECYCLED);
    }
    result.map(|_| ())
}

/// A CPU owner drops the reference instead of transmitting: `CPU -> FREE`.
pub const C_CPU_RELEASE: usize = 28;
pub const C_STOPPED_TX_CANCEL: usize = 29;
pub const C_CPU_REJECT: usize = 30;

fn free_cpu(meta: usize, r: &Ref) -> Result<(), ()> {
    transition(meta, r.handle, CPU, FREE)?;
    release_free(meta, handle_id(r.handle));
    Ok(())
}

pub fn release_cpu(meta: usize, r: &Ref) -> Result<(), ()> {
    free_cpu(meta, r)?;
    count(meta, C_CPU_RELEASE);
    Ok(())
}

pub fn reject(meta: usize, r: &Ref) -> Result<(), ()> {
    free_cpu(meta, r)?;
    count(meta, C_CPU_REJECT);
    count(meta, C_DROPPED);
    Ok(())
}

/// Arm a TX descriptor at the buffer: `CPU -> TX_DMA`. The caller must have
/// written the payload and issued [`barrier::dma_publish`] first.
pub fn submit(meta: usize, r: &Ref) -> Result<(), ()> {
    let result = transition(meta, r.handle, CPU, TX_DMA);
    if result.is_ok() {
        count(meta, C_TX_POSTED);
    }
    result.map(|_| ())
}

/// The TX driver verified completion: `TX_DMA -> FREE`. The caller must issue
/// [`barrier::dma_consume`] before reading any completion data.
pub fn complete(meta: usize, r: &Ref) -> Result<(), ()> {
    let result = transition(meta, r.handle, TX_DMA, FREE);
    if result.is_ok() {
        release_free(meta, handle_id(r.handle));
        count(meta, C_TX_COMPLETED);
        count(meta, C_FORWARDED);
    }
    result.map(|_| ())
}

pub fn cancel_stopped_tx(meta: usize, r: &Ref) -> Result<(), ()> {
    let result = transition(meta, r.handle, TX_DMA, FREE);
    if result.is_ok() {
        release_free(meta, handle_id(r.handle));
        count(meta, C_DROPPED);
        count(meta, C_STOPPED_TX_CANCEL);
    }
    result.map(|_| ())
}

// ---------------------------------------------------------------------------
// Download-fallback return tokens
// ---------------------------------------------------------------------------

/// A worker arms one token per download-fallback send of client `client`
/// (`dst_ip & 255`); the stamp travels in `Ref.meta` beside `FLAG_LAN_TOKEN`.
/// The dispatcher and the NIC drivers retire the stamp exactly once — on
/// verified TX completion, a confirmed software discard, or a dispatch-time
/// drop — and the client returns to the direct worker lane only when every
/// armed token has retired ([`lan_token_settled`]), so a direct-lane frame can
/// never overtake an uncompleted fallback frame of the same client. Tallies
/// wrap together, so `==` is the correct settled test.
fn token_word(meta: usize, base: usize, worker: u32, client: u32) -> &'static AtomicU32 {
    word(meta + base, (worker as usize * TOKEN_CLIENTS + client as usize) * 4)
}

pub fn arm_lan_token(meta: usize, worker: u32, client: u32) -> u32 {
    let _ = token_word(meta, TOKEN_ARMED, worker, client).fetch_add(1, Ordering::AcqRel);
    (worker << 8 | (client & 0xff)) + 1
}

pub fn retire_lan_token(meta: usize, stamp: u32) {
    let Some(key) = stamp.checked_sub(1) else { return };
    let (worker, client) = (key >> 8, key & 0xff);
    if worker < 4 {
        let _ = token_word(meta, TOKEN_RETIRED, worker, client).fetch_add(1, Ordering::AcqRel);
    }
}

/// A tokened frame flooded to `total` egress ports retires once per copy, so
/// the armed side must account for every copy before the client may return.
pub fn extend_lan_token(meta: usize, stamp: u32, total: u32) {
    let Some(key) = stamp.checked_sub(1) else { return };
    let (worker, client) = (key >> 8, key & 0xff);
    if worker < 4 && total > 1 {
        let _ = token_word(meta, TOKEN_ARMED, worker, client)
            .fetch_add(total - 1, Ordering::AcqRel);
    }
}

pub fn lan_token_settled(meta: usize, worker: u32, client: u32) -> bool {
    token_word(meta, TOKEN_ARMED, worker, client).load(Ordering::Acquire)
        == token_word(meta, TOKEN_RETIRED, worker, client).load(Ordering::Acquire)
}

pub fn direct_lan_token(r: &Ref) -> u32 {
    if r.flags & FLAG_LAN_TOKEN == 0 { return 0; }
    if r.flags & FLAG_COPY != 0 { r.meta } else { r.meta >> 16 }
}

// ---------------------------------------------------------------------------
// Source-learn coalescing
// ---------------------------------------------------------------------------

/// Per-worker MAC-learning coalescer: suppresses identical source-learn
/// records inside a short window so flow-affine bursts do not consume one
/// result-queue slot and one `learn_source` call per frame. The key includes
/// the ingress LAN (a MAC move is never suppressed) and the route revision;
/// the window is far below the bridge's learning age, so a suppressed stream
/// still refreshes its FDB entry.
pub const LEARN_COALESCE_MS: u64 = 250;

pub struct LearnCoalescer {
    mac: [u8; 6],
    lan: u16,
    revision: u32,
    next_ms: u64,
    primed: bool,
}

impl LearnCoalescer {
    pub const fn new() -> Self {
        Self { mac: [0; 6], lan: 0, revision: 0, next_ms: 0, primed: false }
    }

    /// Whether a learn record for `(mac, lan, revision)` must be pushed now.
    pub fn due(&self, mac: &[u8], lan: u16, revision: u32, now_ms: u64) -> bool {
        if self.primed && self.lan == lan && self.revision == revision
            && now_ms < self.next_ms && mac.len() == 6 && self.mac[..] == *mac
        {
            return false;
        }
        true
    }

    /// Record a successful push so the next identical learn inside the window
    /// is suppressed. A failed push must not call this.
    pub fn pushed(&mut self, mac: &[u8], lan: u16, revision: u32, now_ms: u64) {
        if mac.len() == 6 {
            self.mac.copy_from_slice(mac);
            self.lan = lan;
            self.revision = revision;
            self.next_ms = now_ms.saturating_add(LEARN_COALESCE_MS);
            self.primed = true;
        }
    }
}

/// Cross-worker source-learn batch: the dispatcher collects consecutive learn
/// records (from any worker) and applies one `learn_source` per MAC. Only
/// identical `(mac, port)` learns coalesce — a MAC move must be applied at its
/// own queue position, so `push` refuses it and the caller flushes first.
pub struct LearnBatch {
    entries: [(u16, [u8; 6]); LearnBatch::CAPACITY],
    len: usize,
}

impl LearnBatch {
    pub const CAPACITY: usize = 8;

    pub const fn new() -> Self {
        Self { entries: [(0, [0; 6]); Self::CAPACITY], len: 0 }
    }

    /// Record one learn. `false` means the caller must apply the batch first
    /// (it is full, or the MAC is moving ports).
    pub fn push(&mut self, port: u16, mac: [u8; 6]) -> bool {
        for entry in self.entries[..self.len].iter_mut() {
            if entry.1 == mac {
                return entry.0 == port;
            }
        }
        if self.len == Self::CAPACITY {
            return false;
        }
        self.entries[self.len] = (port, mac);
        self.len += 1;
        true
    }

    /// Apply every collected learn in insertion order and empty the batch.
    pub fn apply(&mut self, mut learn: impl FnMut(u16, [u8; 6])) {
        for index in 0..self.len {
            let (port, mac) = self.entries[index];
            learn(port, mac);
        }
        self.len = 0;
    }
}

/// # Safety
/// Only after the owning device has verifiably stopped DMA and the fabric has
/// drained every outstanding transaction for this pool generation. Force every
/// buffer back to `FREE` with an advanced generation, invalidating all
/// outstanding references.
pub unsafe fn quiesce(meta: usize) {
    for id in 0..BUFFERS as u32 {
        let cell = ownership(meta, id);
        let value = cell.load(Ordering::Acquire);
        if owner_state(value) != FREE {
            cell.store(
                owner_word(FREE, owner_generation(value).wrapping_add(1) & GEN_MASK),
                Ordering::Release,
            );
            release_free(meta, id);
        }
    }
}

/// `(in_use, free, peak)` across all partitions.
pub fn occupancy(meta: usize) -> (u32, u32, u32) {
    let mut used = 0;
    let mut free = 0;
    for partition in 0..FREE_LISTS {
        let (first, end) = PARTITION_BOUNDS[partition];
        let active = word(meta + IN_USE, partition * 4).load(Ordering::Relaxed).min(end - first);
        used += active;
        free += end - first - active;
    }
    (used, free, counter(meta, C_PEAK_OCCUPANCY).load(Ordering::Relaxed))
}

fn free_words(meta: usize, partition: usize) -> &'static [AtomicU32] {
    unsafe {
        core::slice::from_raw_parts(
            (meta + FREE_BITS + partition * FREE_WORDS * 4) as *const AtomicU32,
            FREE_WORDS,
        )
    }
}

/// Initialise the free bitmaps of a freshly zeroed metadata region. Safe to
/// call from several protection domains: one caller wins the initialisation
/// and the others return. [`alloc`] refuses to hand out buffers until the
/// free lists are ready, so a fast starter falls back to the copy path.
pub fn init(meta: usize) {
    let state = word(meta + INIT, 0);
    if state
        .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    for partition in 0..FREE_LISTS {
        let (first, end) = PARTITION_BOUNDS[partition];
        for id in first..end {
            free_words(meta, partition)[(id / 32) as usize].fetch_or(1 << (id % 32), Ordering::Release);
        }
        word(meta + IN_USE, partition * 4).store(0, Ordering::Relaxed);
        word(meta + IN_USE, (FREE_LISTS + partition) * 4).store(0, Ordering::Relaxed);
    }
    state.store(1, Ordering::Release);
}

/// Whether the shared free lists are ready for allocation.
pub fn ready(meta: usize) -> bool {
    word(meta + INIT, 0).load(Ordering::Acquire) == 1
}

fn release_free(meta: usize, id: u32) {
    let partition = PARTITION_BOUNDS
        .iter()
        .position(|&(first, end)| id >= first && id < end)
        .unwrap_or(0);
    let bit = 1u32 << (id % 32);
    use_growth(meta, partition, false);
    let previous = free_words(meta, partition)[(id / 32) as usize].fetch_or(bit, Ordering::AcqRel);
    if previous & bit != 0 {
        // Double free: the bit was already set. Keep the error visible and do
        // not corrupt the free list further.
        use_growth(meta, partition, true);
        note_ownership_error(meta);
    }
}

fn use_growth(meta: usize, partition: usize, allocate: bool) {
    let cell = word(meta + IN_USE, partition * 4);
    if !allocate {
        cell.fetch_sub(1, Ordering::AcqRel);
        return;
    }
    let value = cell.fetch_add(1, Ordering::AcqRel).saturating_add(1);
    let peak = word(meta + IN_USE, (FREE_LISTS + partition) * 4);
    if value > peak.load(Ordering::Relaxed) {
        peak.fetch_max(value, Ordering::Relaxed);
    }
    let total = (0..FREE_LISTS).map(|p| word(meta + IN_USE, p * 4).load(Ordering::Relaxed)).sum();
    let global = counter(meta, C_PEAK_OCCUPANCY);
    if total > global.load(Ordering::Relaxed) {
        global.fetch_max(total, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Reference queues
// ---------------------------------------------------------------------------

/// Header of a bounded SPSC reference queue in shared memory.
/// `head` is the producer cursor, `tail` the consumer cursor; both are
/// wrapping `u32` counters published with release/acquire ordering.
pub struct RefQueue;

pub struct IngressBatch {
    meta: usize,
    pending: Option<(usize, Ref, u64)>,
    pin: Option<TransportPin>,
}

impl IngressBatch {
    pub const fn new(meta: usize) -> Self {
        Self { meta, pending: None, pin: None }
    }

    fn publish(&self, queue: usize, packet: Ref) -> u32 {
        if RefQueue::push(queue, &packet) == Ok(true) { return 0; }
        let _ = reject(self.meta, &packet);
        1
    }

    pub fn flush(&mut self) -> u32 {
        let _pin = self.pin.take();
        self.pending.take().map_or(0, |(queue, packet, _)| self.publish(queue, packet))
    }

    pub fn flush_expired(&mut self, clock: impl FnOnce() -> u64) -> u32 {
        if self.pending.is_some_and(|(_, _, arrival)| clock().saturating_sub(arrival) >= Self::hold_us()) {
            self.flush()
        } else { 0 }
    }

    pub fn offer(&mut self, queue: usize, packet: Ref, first: bool, now_us: u64,
                 compatible: impl FnOnce(Ref, Ref) -> bool) -> u32 {
        self.offer_inner(queue,packet,first,now_us,false,None,compatible)
    }

    fn offer_inner(&mut self, queue: usize, packet: Ref, first: bool, now_us: u64,
                   mark_pair: bool, pin: Option<TransportPin>, compatible: impl FnOnce(Ref, Ref) -> bool) -> u32 {
        let mut drops = 0;
        let _previous_pin = self.pin.take();
        if let Some((previous_queue, previous, arrival)) = self.pending.take() {
            if previous_queue == queue && previous.epoch == packet.epoch
                && previous.flags == packet.flags && previous.id() != packet.id()
                && now_us.saturating_sub(arrival) < Self::hold_us()
                && compatible(previous, packet)
            {
                let pair=if mark_pair {
                    [Ref {flags:previous.flags|FLAG_GRO_FIRST,..previous},
                     Ref {flags:packet.flags|FLAG_GRO_LAST,..packet}]
                } else {[previous,packet]};
                if RefQueue::push_batch(queue,&pair)==Ok(true) {
                    if option_env!("R5S_SDK_CONFIG") == Some("benchmark") {
                        count(self.meta, C_GRO_DRIVER_PAIRS);
                    }
                    return 0;
                }
                return self.publish(queue, previous) + self.publish(queue, packet);
            }
            drops += self.publish(previous_queue, previous);
        }
        if first {
            self.pending = Some((queue, packet, now_us));
            self.pin = pin;
        }
        else { drops += self.publish(queue, packet); }
        drops
    }

    fn hold_us() -> u64 {
        match option_env!("R5S_GRO_HOLD_US") {
            Some("100") => 100,
            Some("500") => 500,
            Some("1000") => 1000,
            _ => 250,
        }
    }

    /// # Safety
    /// Each input reference must be exclusively CPU-owned in the mapped pool.
    pub unsafe fn offer_pool(&mut self, queue: usize, packet: Ref, first: bool, now_us: u64,
                            compatible: impl FnOnce(&[u8], &[u8]) -> bool) -> u32 {
        unsafe { self.offer_pinned_pool(queue, packet, first, now_us, None, compatible) }
    }

    pub unsafe fn offer_pinned_pool(&mut self, queue: usize, packet: Ref, first: bool, now_us: u64,
                            pin: Option<TransportPin>, compatible: impl FnOnce(&[u8], &[u8]) -> bool) -> u32 {
        self.offer_inner(queue, packet, first, now_us, true, pin, |previous, packet| {
            if [previous, packet].iter().any(|r| !(14..=1514).contains(&r.len)) { return false; }
            let frame = |r: Ref| unsafe {
                core::slice::from_raw_parts((POOL_VA + r.id() as usize * BUFFER) as *const u8, usize::from(r.len))
            };
            compatible(frame(previous), frame(packet))
        })
    }
}

impl Drop for IngressBatch {
    fn drop(&mut self) {
        if let Some((_, packet, _)) = self.pending.take() {
            let _ = reject(self.meta, &packet);
        }
    }
}

fn rq_head(base: usize) -> &'static AtomicU32 {
    word(base, 0)
}
fn rq_tail(base: usize) -> &'static AtomicU32 {
    word(base, RQ_TAIL)
}

impl RefQueue {
    pub fn push_copy(base: usize, copy: impl FnOnce() -> Result<bool, ()>) -> Result<bool, ()> {
        Self::push_copy_at(base, 0, 0, copy)
    }

    /// Like [`Self::push_copy`] but the marker carries a download-fallback
    /// return token (`FLAG_LAN_TOKEN`, stamp in `meta`) that the driver
    /// retires when the copy completes.
    pub fn push_copy_token(base: usize, token: u32, copy: impl FnOnce() -> Result<bool, ()>) -> Result<bool, ()> {
        Self::push_copy_token_at(base, 0, token, copy)
    }

    pub fn push_copy_token_at(base: usize, revision: u32, token: u32,
                              copy: impl FnOnce() -> Result<bool, ()>) -> Result<bool, ()> {
        if Self::depth(base)? == RQ_COUNT || !copy()? {
            return Ok(false);
        }
        Self::push(base, &Ref {
            flags: FLAG_COPY | FLAG_LAN_TOKEN, epoch: revision, meta: token, ..Ref::new(0, 0)
        })
    }

    pub fn push_copy_at(base: usize, revision: u32, time: u32,
                        copy: impl FnOnce() -> Result<bool, ()>) -> Result<bool, ()> {
        if Self::depth(base)? == RQ_COUNT || !copy()? {
            return Ok(false);
        }
        Self::push(base, &Ref { flags: FLAG_COPY, epoch: revision, meta: time, ..Ref::new(0, 0) })
    }

    pub fn claim_wake(base: usize) -> bool {
        word(base, RQ_WAKE).swap(1, Ordering::AcqRel) == 0
    }

    pub fn rearm_wake(base: usize) {
        word(base, RQ_WAKE).swap(0, Ordering::AcqRel);
    }

    /// Push one reference. `Ok(false)` means the queue is full (backpressure);
    /// `Err` means the shared memory looks corrupt.
    pub fn push(base: usize, r: &Ref) -> Result<bool, ()> {
        Self::push_batch(base, core::slice::from_ref(r))
    }

    pub fn push_batch(base: usize, refs: &[Ref]) -> Result<bool, ()> {
        let head = rq_head(base).load(Ordering::Relaxed);
        let tail = rq_tail(base).load(Ordering::Acquire);
        let pending = head.wrapping_sub(tail);
        if pending > RQ_COUNT {
            return Err(());
        }
        if refs.len() > (RQ_COUNT - pending) as usize {
            return Ok(false);
        }
        for (offset, r) in refs.iter().enumerate() {
            let index = head.wrapping_add(offset as u32) % RQ_COUNT;
            let slot = base + RQ_ENTRIES + index as usize * core::mem::size_of::<Ref>();
            unsafe { core::ptr::write_volatile(slot as *mut Ref, *r); }
        }
        rq_head(base).store(head.wrapping_add(refs.len() as u32), Ordering::Release);
        Ok(true)
    }

    pub fn peek(base: usize) -> Result<Option<Ref>, ()> {
        Self::peek_at(base, 0)
    }

    pub fn peek_at(base: usize, offset: u32) -> Result<Option<Ref>, ()> {
        let tail = rq_tail(base).load(Ordering::Relaxed);
        let head = rq_head(base).load(Ordering::Acquire);
        let pending = head.wrapping_sub(tail);
        if pending > RQ_COUNT {
            return Err(());
        }
        if offset >= pending {
            return Ok(None);
        }
        Ok(Some(Self::read_slot(base, tail.wrapping_add(offset))))
    }

    fn read_slot(base: usize, cursor: u32) -> Ref {
        let slot = base + RQ_ENTRIES + (cursor % RQ_COUNT) as usize * core::mem::size_of::<Ref>();
        unsafe { core::ptr::read_volatile(slot as *const Ref) }
    }

    /// Pop one reference. Only the SPSC consumer may advance the tail.
    pub fn pop(base: usize) -> Result<Option<Ref>, ()> {
        let value = Self::peek(base)?;
        if value.is_some() {
            let tail = rq_tail(base).load(Ordering::Relaxed);
            rq_tail(base).store(tail.wrapping_add(1), Ordering::Release);
        }
        Ok(value)
    }

    pub fn pop_pair_if(base: usize, compatible: impl FnOnce([Ref; 2]) -> bool) -> Result<Option<[Ref; 2]>, ()> {
        let tail = rq_tail(base).load(Ordering::Relaxed);
        let pending = rq_head(base).load(Ordering::Acquire).wrapping_sub(tail);
        if pending > RQ_COUNT { return Err(()); }
        if pending < 2 { return Ok(None); }
        let pair = [Self::read_slot(base, tail), Self::read_slot(base, tail.wrapping_add(1))];
        if pair.iter().any(|entry| entry.flags & FLAG_COPY != 0) || !compatible(pair) {
            return Ok(None);
        }
        rq_tail(base).store(tail.wrapping_add(2), Ordering::Release);
        Ok(Some(pair))
    }

    pub fn depth(base: usize) -> Result<u32, ()> {
        let head = rq_head(base).load(Ordering::Acquire);
        let tail = rq_tail(base).load(Ordering::Acquire);
        let pending = head.wrapping_sub(tail);
        if pending > RQ_COUNT {
            Err(())
        } else {
            Ok(pending)
        }
    }

    pub fn produced(base: usize) -> u32 {
        rq_head(base).load(Ordering::Acquire)
    }
    pub fn consumed(base: usize) -> u32 {
        rq_tail(base).load(Ordering::Acquire)
    }
}

pub const FAST_WAN_FRAMES: usize = 0x66a00000;
pub const FAST_WAN_TX: usize = FAST_WAN_FRAMES + 0x7100;
pub const FAST_WAN_CONTROL: usize = 0x65000000;
pub const FAST_WAN_CHANNEL: usize = 10;
const WAN_ACTIVE_BASE: usize = META_VA + META_BYTES - 64;
const _: () = assert!(TOKEN_RETIRED + TOKEN_BYTES <= META_BYTES - 64);

pub fn publish_wan_tso(available: bool) {
    let status = word(WAN_ACTIVE_BASE, 4);
    if status.load(Ordering::Relaxed) != u32::from(available) {
        status.store(u32::from(available), Ordering::Release);
    }
}

pub fn wan_tso_available() -> bool {
    word(WAN_ACTIVE_BASE, 4).load(Ordering::Acquire) != 0
}

pub fn wan_active_lanes() -> u32 {
    word(WAN_ACTIVE_BASE, 0).load(Ordering::Acquire)
}

pub fn wan_lane_bit(refs: usize) -> u32 {
    1 << ((refs - worker_wan_refs(0)) / 0x4000)
}

pub fn activate_wan_lanes(lanes: u32) -> bool {
    lanes != 0 && word(WAN_ACTIVE_BASE, 0).fetch_or(lanes, Ordering::AcqRel) & lanes != lanes
}

pub fn clear_idle_wan_lane(lane: usize, refs: usize) {
    word(WAN_ACTIVE_BASE, 0).fetch_and(!(1 << lane), Ordering::AcqRel);
    if RefQueue::depth(refs) != Ok(0) {
        word(WAN_ACTIVE_BASE, 0).fetch_or(1 << lane, Ordering::Release);
    }
}

pub const WORKER_TX_WAKEUPS: usize = 41;
pub const WORKER_TX_DRAINED: usize = 42;
pub fn add_worker_tx_drained(count: u32) {
    if count != 0 { add(counter(META_VA, WORKER_TX_DRAINED), count); }
}
pub const WORKER_TX_FULL: usize = 43;
pub const WORKER_TX_BUDGET: usize = 44;
pub const WORKER_TX_RING_STALL: usize = 45;
pub const WORKER_TX_WAKE_PASSES: usize = 46;
pub const C_NAT_COPY_CHECKSUM_BYPASS: usize = 47;
pub const C_LEARN_SUBMITTED: usize = 48;
pub const C_LEARN_DROPS: usize = 49;
pub const C_LEARN_COALESCED: usize = 50;
pub const WORKER_TX_EVENT_DRIVEN: bool = matches!(option_env!("R5S_GMAC_WORKER_TX"), Some(v) if matches!(v.as_bytes(), b"1"));
pub fn drain_workers(
    next: &mut usize, budget: u32, mut service: impl FnMut(usize) -> Option<bool>,
) -> u32 {
    let mut drained = 0;
    let mut idle = 0;
    while drained < budget && idle < 4 {
        let Some(progress) = service(*next) else { break };
        *next = (*next + 1) % 4;
        if progress {
            drained += 1;
            idle = 0;
        } else {
            idle += 1;
        }
    }
    drained
}

#[test]
fn worker_tx_spends_spare_quota_and_keeps_cursor_on_ring_stall() {
    let mut next = 0;
    assert_eq!(drain_workers(&mut next, 16, |worker| Some(worker == 2)), 16);
    assert_eq!(next, 3);
    assert_eq!(drain_workers(&mut next, 16, |_| None), 0);
    assert_eq!(next, 3);
    let mut order = [0; 4];
    let mut index = 0;
    assert_eq!(drain_workers(&mut next, 4, |worker| {
        order[index] = worker;
        index += 1;
        Some(true)
    }), 4);
    assert_eq!(order, [3, 0, 1, 2]);
}

pub const WORKER_WAN_ROUTE: usize = 0x66d00000;
pub const STEERING_TABLE: usize = 0x67500000;
pub const STEERING_LOAD: usize = 0x67580000;
pub const STEERING_SLOTS: usize = 0x10000;

const STEERING_WAYS: usize = 16;
const STEERING_SETS: usize = STEERING_SLOTS / STEERING_WAYS;
const STEERING_LOCKS: usize = STEERING_LOAD + 0x1000;
const PIN_COUNT: u64 = 0x78;
const PIN_ONE: u64 = 8;
const PIN_REAPING: u64 = PIN_COUNT;

use core::sync::atomic::AtomicU64;

fn steering_slot(index: usize) -> &'static AtomicU64 {
    unsafe { &*(STEERING_TABLE as *const AtomicU64).add(index) }
}

pub struct TransportPin {
    pub worker: usize,
    slot: &'static AtomicU64,
}

impl Drop for TransportPin {
    fn drop(&mut self) { self.slot.fetch_sub(PIN_ONE, Ordering::Release); }
}

fn admit_transport(slot: &'static AtomicU64, key: u64, mut value: u64) -> Option<TransportPin> {
    for _ in 0..4 {
        if value >> 7 != key || value & PIN_COUNT >= PIN_REAPING - PIN_ONE { break; }
        match slot.compare_exchange_weak(value, value + PIN_ONE, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Some(TransportPin { worker: ((value & 7) - 1) as usize, slot }),
            Err(actual) => value = actual,
        }
    }
    None
}

pub fn pin_transport(key: u64, hash: u32, depth: impl Fn(usize) -> u32) -> Option<TransportPin> {
    let set = hash as usize & (STEERING_SETS - 1);
    let slots = || (set * STEERING_WAYS..(set + 1) * STEERING_WAYS).map(steering_slot);
    for slot in slots() {
        let value = slot.load(Ordering::Acquire);
        if value != 0 && value >> 7 == key { return admit_transport(slot, key, value); }
    }
    let lock = word(STEERING_LOCKS, set * 4);
    if lock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() { return None; }
    let mut free = None;
    let mut result = None;
    let mut existing = false;
    for slot in slots() {
        let value = slot.load(Ordering::Acquire);
        if value == 0 { free.get_or_insert(slot); }
        else if value >> 7 == key {
            existing = true;
            result = admit_transport(slot, key, value);
            break;
        }
    }
    if !existing && let Some(slot) = free {
        let preferred = (hash >> 20) as usize % 4;
        let worker = (0..4).map(|offset| (preferred + offset) % 4)
            .min_by_key(|&worker| depth(worker).saturating_mul(128)
                .saturating_add(word(STEERING_LOAD, worker * 64).load(Ordering::Relaxed)))
            .unwrap_or(preferred);
        slot.store((key << 7) | PIN_ONE | (worker as u64 + 1), Ordering::Release);
        result = Some(TransportPin { worker, slot });
    }
    lock.store(0, Ordering::Release);
    result
}

pub fn reap_transport_pins(worker: usize, cursor: &mut usize,
    queued: impl Fn() -> bool, live: impl Fn(u64) -> bool) {
    for _ in 0..256 {
        let index = *cursor;
        *cursor = (index + 1) % STEERING_SLOTS;
        let slot = steering_slot(index);
        let value = slot.load(Ordering::Acquire);
        if value & (PIN_COUNT | 7) != worker as u64 + 1 || live(value >> 7) { continue; }
        let lock = word(STEERING_LOCKS, index / STEERING_WAYS * 4);
        if lock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() { continue; }
        if slot.compare_exchange(value, value | PIN_REAPING, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            slot.store(if queued() { value } else { 0 }, Ordering::Release);
        }
        lock.store(0, Ordering::Release);
    }
}

pub fn publish_steering_load(worker: usize, active: u32) {
    word(STEERING_LOAD, worker * 64).store(active, Ordering::Relaxed);
}

/// Runtime TX checksum-offload state, published by the GMAC PD after
/// `dma_start`. Workers consult it when preparing payloads so a GMAC without
/// the insertion capability keeps software checksums in the frame.
pub const TX_OFFLOAD_STATE: usize = META_BYTES - 4;
pub fn publish_tx_checksum_offload(meta: usize, active: bool) {
    word(meta + TX_OFFLOAD_STATE, 0).store(u32::from(active), Ordering::Release);
}
pub fn tx_checksum_offload(meta: usize) -> bool {
    word(meta + TX_OFFLOAD_STATE, 0).load(Ordering::Acquire) != 0
}
pub const DEDICATED_TX_LANES: usize = 4;
pub const TX_FLOW_LANES: usize = DEDICATED_TX_LANES + 1;
pub const WORKER_TX_LANES: usize = 4 * TX_FLOW_LANES;
pub const fn tx_lane(worker: usize, lane: usize) -> usize {
    worker * TX_FLOW_LANES + lane
}
pub fn next_ready_lane(ready: u32, start: usize) -> Option<usize> {
    if ready == 0 { return None; }
    let after_start = ready >> start;
    Some(if after_start == 0 { ready.trailing_zeros() as usize }
        else { start + after_start.trailing_zeros() as usize })
}
pub const fn worker_wan_frames(lane: usize) -> usize { 0x67000000 + lane * 0x4000 }
pub const fn worker_wan_refs(lane: usize) -> usize { worker_wan_frames(lane) + 0x3100 }
pub fn publish_wan_tx_consumed(refs: usize) {
    word(refs, 0xe00).store(RefQueue::consumed(refs), Ordering::Release);
}
pub fn worker_wan_tx_drained(refs: usize) -> bool {
    word(refs, 0xe00).load(Ordering::Acquire) == RefQueue::produced(refs)
}

fn wan_dma_completed(channel: usize) -> &'static AtomicU64 {
    unsafe { &*((META_VA + WAN_DMA_COMPLETED + channel * 8) as *const AtomicU64) }
}

fn wan_tx_fence(refs: usize) -> &'static AtomicU64 {
    unsafe { &*((refs + 0xe08) as *const AtomicU64) }
}

pub fn note_wan_dma_completion(channel: usize, count: u64) {
    if count != 0 { wan_dma_completed(channel).fetch_add(count, Ordering::Release); }
}

pub fn record_wan_tx_fence(refs: usize, channel: usize, pending: u32) {
    let target = wan_dma_completed(channel).load(Ordering::Relaxed) + u64::from(pending);
    wan_tx_fence(refs).store((target << 1) | channel as u64, Ordering::Release);
}

pub fn clear_wan_tx_fence(refs: usize) {
    wan_tx_fence(refs).store(0, Ordering::Release);
}

pub fn worker_wan_tx_complete(refs: usize) -> bool {
    if !worker_wan_tx_drained(refs) { return false; }
    let fence = wan_tx_fence(refs).load(Ordering::Acquire);
    wan_dma_completed((fence & 1) as usize).load(Ordering::Acquire) >= fence >> 1
}
pub const fn worker_wan_egress() -> bool {
    match option_env!("R5S_WORKER_WAN_EGRESS") {
        Some(value) => matches!(value.as_bytes(), b"1"),
        None => false,
    }
}

pub type LanRoute = (u32, [u8; 6], usize, u64);
pub const LAN_ROUTE_OFFSET: usize = 0x440;
pub const fn worker_lan_frames(port: usize, lane: usize) -> usize {
    0x67200000 + (port * WORKER_TX_LANES + lane) * 0x4000
}
pub const fn worker_lan_refs(port: usize, lane: usize) -> usize {
    worker_lan_frames(port, lane) + 0x3100
}
const LAN_TX_PENDING_OFFSET: usize = 0xe00;
const LAN_TX_CLOSED: u32 = 1 << 31;
const LAN_TX_PUBLISHING: u32 = 1 << 30;
const LAN_TX_CONSUMER_STOPPED: u32 = 1 << 29;
const LAN_TX_FLAGS: u32 = LAN_TX_CLOSED | LAN_TX_PUBLISHING | LAN_TX_CONSUMER_STOPPED;
const _: () = assert!(RQ_BYTES <= LAN_TX_PENDING_OFFSET);

pub fn publish_worker_lan_tx(refs: usize, publish: impl FnOnce() -> bool) -> bool {
    publish_worker_lan_tx_many(refs, 1, publish)
}

pub fn publish_worker_lan_tx_many(refs: usize, count: u32, publish: impl FnOnce() -> bool) -> bool {
    let pending = word(refs, LAN_TX_PENDING_OFFSET);
    if pending.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
        (n & (LAN_TX_CLOSED | LAN_TX_PUBLISHING) == 0).then_some((n + count) | LAN_TX_PUBLISHING)
    }).is_err() { return false; }
    let queued = publish();
    if !queued { pending.fetch_sub(count, Ordering::Release); }
    pending.fetch_and(!LAN_TX_PUBLISHING, Ordering::Release);
    queued
}

pub fn retire_worker_lan_tx(refs: usize) {
    let pending = word(refs, LAN_TX_PENDING_OFFSET);
    let _ = pending.fetch_update(Ordering::Release, Ordering::Relaxed, |n| {
        (n & !LAN_TX_FLAGS != 0).then(|| n - 1)
    });
}

pub fn worker_lan_tx_drained(refs: usize) -> bool {
    word(refs, LAN_TX_PENDING_OFFSET).load(Ordering::Acquire) & !(LAN_TX_CLOSED | LAN_TX_CONSUMER_STOPPED) == 0
}

pub fn worker_lan_tx_depths(port: usize) -> u32 {
    (0..4).fold(1 << 31, |packed, worker| {
        let pending: u32 = (0..TX_FLOW_LANES).map(|lane| {
            let refs = worker_lan_refs(port, worker * TX_FLOW_LANES + lane);
            word(refs, LAN_TX_PENDING_OFFSET).load(Ordering::Relaxed) & !LAN_TX_FLAGS
        }).sum();
        packed | (pending.min(127) << (worker * 7))
    })
}

pub fn worker_lan_tx_closed(refs: usize) -> bool {
    word(refs, LAN_TX_PENDING_OFFSET).load(Ordering::Acquire) & LAN_TX_CLOSED != 0
}

pub fn close_worker_lan_tx(refs: usize) -> bool {
    word(refs, LAN_TX_PENDING_OFFSET).fetch_or(LAN_TX_CLOSED, Ordering::AcqRel) & LAN_TX_PUBLISHING == 0
}
pub fn release_worker_lan_consumer(refs: usize) {
    word(refs, LAN_TX_PENDING_OFFSET).fetch_or(LAN_TX_CONSUMER_STOPPED, Ordering::Release);
}

pub fn worker_lan_consumer_stopped(refs: usize) -> bool {
    word(refs, LAN_TX_PENDING_OFFSET).load(Ordering::Acquire) & LAN_TX_CONSUMER_STOPPED != 0
}
/// Per-client download path selection. `resolved` is the route's egress port
/// when the neighbor is known and that lane is open; `settled` reports that
/// every fallback frame this client sent has retired (token tallies equal);
/// `drained` reports that the old direct lane has no outstanding submission.
///
/// Returns `Some(port)` to send directly on the worker lane, `None` to take
/// the dispatcher fallback (the caller arms a return token), and `Err(())` to
/// discard the frame while a path transition drains. A client on fallback
/// returns to the direct lane only when settled, so no direct-lane frame can
/// overtake an uncompleted fallback frame; other clients are never gated.
pub fn select_lan_tx_path(
    path: &mut u8, resolved: Option<usize>, settled: bool, drained: impl FnOnce(usize) -> bool,
) -> Result<Option<usize>, ()> {
    if *path == 1 {
        if settled {
            if let Some(port) = resolved {
                *path = port as u8 + 2;
                return Ok(Some(port));
            }
        }
        return Ok(None);
    }
    if let Some(port) = resolved.filter(|port| *path == 0 || usize::from(*path) == *port + 2) {
        *path = port as u8 + 2;
        return Ok(Some(port));
    }
    if *path >= 2 && !drained(usize::from(*path - 2)) { return Err(()); }
    *path = 1;
    Ok(None)
}
pub fn worker_lan_tx_valid(control: usize, frame: &[u8], port: usize, epoch: u32, now: u64) -> bool {
    if frame.len() < 34 || frame[12..15] != [8, 0, 0x45] { return false; }
    let ip = u32::from_be_bytes(frame[30..34].try_into().unwrap());
    route_revision(control, None) == Some(epoch)
        && lan_route(control + LAN_ROUTE_OFFSET, ip, now)
            .ok().flatten()
            .is_some_and(|(mac, target, _)| target == port && frame[..6] == mac)
}
const LAN_ROUTE_SLOTS: usize = 64;
const _: () = assert!(LAN_ROUTE_OFFSET + 4 + LAN_ROUTE_SLOTS * 20 <= 0x1000);

fn lan_route_slot(ip: u32, probe: usize) -> usize {
    (ip as usize + probe) & (LAN_ROUTE_SLOTS - 1)
}

pub fn publish_lan_routes(base: usize, routes: impl Iterator<Item = LanRoute>) {
    let mut entries = [(0, [0; 6], 0, 0); LAN_ROUTE_SLOTS];
    for route in routes.take(LAN_ROUTE_SLOTS / 2) {
        if route.0 == 0 { continue; }
        for probe in 0..LAN_ROUTE_SLOTS {
            let slot = &mut entries[lan_route_slot(route.0, probe)];
            if slot.0 == 0 || slot.0 == route.0 { *slot = route; break; }
        }
    }
    let packed = entries.map(|(ip, mac, port, until)| [ip,
        u32::from_le_bytes(mac[..4].try_into().unwrap()),
        u32::from(u16::from_le_bytes([mac[4], mac[5]])) | ((port as u32) << 16),
        until as u32, (until >> 32) as u32]);
    if packed.iter().enumerate().all(|(index, row)| row.iter().enumerate().all(|(i, value)|
        word(base + 4 + index * 20, i * 4).load(Ordering::Relaxed) == *value)) { return; }
    word(base, 0).fetch_add(1, Ordering::AcqRel);
    for (index, row) in packed.iter().enumerate() {
        let entry = base + 4 + index * 20;
        for (i, value) in row.iter().enumerate() { word(entry, i * 4).store(*value, Ordering::Relaxed); }
    }
    word(base, 0).fetch_add(1, Ordering::Release);
}

pub fn lan_route(base: usize, ip: u32, now: u64) -> Result<Option<([u8; 6], usize, u64)>, ()> {
    let sequence = word(base, 0).load(Ordering::Acquire);
    if sequence & 1 != 0 { return Err(()); }
    if ip == 0 { return Ok(None); }
    let mut route = None;
    for probe in 0..LAN_ROUTE_SLOTS {
        let index = lan_route_slot(ip, probe);
        let entry = base + 4 + index * 20;
        let address = word(entry, 0).load(Ordering::Relaxed);
        if address == 0 { break; }
        if address != ip { continue; }
        let low = word(entry, 4).load(Ordering::Relaxed).to_le_bytes();
        let high = word(entry, 8).load(Ordering::Relaxed).to_le_bytes();
        let until = u64::from(word(entry, 12).load(Ordering::Relaxed))
            | (u64::from(word(entry, 16).load(Ordering::Relaxed)) << 32);
        route = (until > now && high[2] < 2 && high[3] == 0)
            .then_some(([low[0], low[1], low[2], low[3], high[0], high[1]], usize::from(high[2]), until));
        break;
    }
    core::sync::atomic::fence(Ordering::Acquire);
    if word(base, 0).load(Ordering::Relaxed) != sequence { return Err(()); }
    Ok(route)
}

pub fn publish_wan_route(base: usize, revision: u32, mac: [u8; 6], until: u64) {
    word(base, 0).fetch_add(1, Ordering::AcqRel);
    word(base, 4).store(revision, Ordering::Relaxed);
    word(base, 8).store(u32::from_le_bytes(mac[..4].try_into().unwrap()), Ordering::Relaxed);
    word(base, 12).store(u32::from(u16::from_le_bytes(mac[4..].try_into().unwrap())), Ordering::Relaxed);
    word(base, 16).store(until as u32, Ordering::Relaxed);
    word(base, 20).store((until >> 32) as u32, Ordering::Relaxed);
    word(base, 0).fetch_add(1, Ordering::Release);
}

pub fn wan_route(base: usize, revision: u32, now: u64) -> Option<[u8; 6]> {
    let seq = word(base, 0).load(Ordering::Acquire);
    if seq & 1 != 0 || word(base, 4).load(Ordering::Relaxed) != revision { return None; }
    let low = word(base, 8).load(Ordering::Relaxed).to_le_bytes();
    let high = (word(base, 12).load(Ordering::Relaxed) as u16).to_le_bytes();
    let until = u64::from(word(base, 16).load(Ordering::Relaxed))
        | (u64::from(word(base, 20).load(Ordering::Relaxed)) << 32);
    core::sync::atomic::fence(Ordering::Acquire);
    if until <= now || word(base, 0).load(Ordering::Relaxed) != seq { return None; }
    Some([low[0], low[1], low[2], low[3], high[0], high[1]])
}

pub fn source_learn(mac: &[u8], lan: u16, revision: u32) -> Ref {
    Ref {
        handle: u32::from_le_bytes(mac[..4].try_into().unwrap()),
        len: u16::from_le_bytes(mac[4..6].try_into().unwrap()),
        flags: FLAG_SOURCE_LEARN | lan << LAN_INGRESS_SHIFT,
        epoch: revision, meta: 0,
    }
}

pub fn learned_mac(record: Ref) -> [u8; 6] {
    let low = record.handle.to_le_bytes();
    let high = record.len.to_le_bytes();
    [low[0], low[1], low[2], low[3], high[0], high[1]]
}

pub fn worker_wan_header(frame: &mut [u8], next_hop: [u8; 6]) -> Option<usize> {
    if frame.len() < 34 || frame[12..15] != [8, 0, 0x45] { return None; }
    let size = 14 + usize::from(u16::from_be_bytes([frame[16], frame[17]]));
    if !(34..=frame.len()).contains(&size) { return None; }
    frame[..6].copy_from_slice(&next_hop);
    frame[6..12].copy_from_slice(&WAN_MAC);
    Some(size)
}
pub const fn fast_wan_frames(worker: usize) -> usize { 0x66b00000 + worker * 0x4000 }
pub const fn fast_wan_requests(worker: usize) -> usize { fast_wan_frames(worker) + 0x3100 }
pub const fn fast_lan_frames(port: usize, worker: usize) -> usize {
    0x66c00000 + (port * 4 + worker) * 0x4000
}
pub const fn fast_lan_requests(port: usize, worker: usize) -> usize { fast_lan_frames(port, worker) + 0x3100 }
pub const fn lan_fast_path() -> bool {
    match option_env!("R5S_LAN_FAST_PATH") {
        Some(value) => matches!(value.as_bytes(), b"1"),
        None => false,
    }
}

pub fn fast_wan_tag(control: usize, epoch: u32) -> Option<u32> {
    route_revision(control, Some(epoch))
}

pub fn route_revision(control: usize, epoch: Option<u32>) -> Option<u32> {
    let before = word(control, 0).load(Ordering::Acquire);
    if before == 0 || before & 1 != 0 { return None; }
    let revision = word(control, 4).load(Ordering::Relaxed);
    let valid = word(control, 12).load(Ordering::Relaxed) == 1;
    let link_epoch = word(control, 52).load(Ordering::Relaxed);
    core::sync::atomic::fence(Ordering::Acquire);
    (valid && epoch.is_none_or(|expected| expected == link_epoch)
        && before == word(control, 0).load(Ordering::Acquire)).then_some(revision)
}

pub fn fast_wan_owner(frame: &[u8]) -> Option<usize> {
    wan_fast_packet(frame, true).then(|| usize::from(frame[37] & 3))
}

pub const WAN_MAC: [u8; 6] = [2, 0x52, 0x35, 0x53, 0, 1];

pub const fn wan_fast_path() -> bool {
    match option_env!("R5S_WAN_FAST_PATH") {
        Some(value) => matches!(value.as_bytes(), b"1"),
        None => false,
    }
}

pub fn wan_fast_packet(frame: &[u8], ingress: bool) -> bool {
    let minimum = match frame.get(23) {
        Some(6) => 40,
        Some(17) => 28,
        _ => return false,
    };
    if frame.len() < 14 + minimum || frame[12..15] != [8, 0, 0x45]
        || u16::from_be_bytes([frame[20], frame[21]]) & 0xbfff != 0
        || !(minimum..=frame.len() - 14).contains(&usize::from(u16::from_be_bytes([frame[16], frame[17]])))
    {
        return false;
    }
    if ingress {
        frame[..6] == WAN_MAC && u16::from_be_bytes([frame[36], frame[37]]) >= 49152
    } else {
        frame[6..12] == WAN_MAC && frame[0] & 1 == 0
    }
}

/// Base of one link reference queue inside the `zc_links` region.
pub fn link_queue(links: usize, index: usize) -> usize {
    assert!(index < QUEUES);
    links + index * LINK_STRIDE
}

// ---------------------------------------------------------------------------
// DMA address translation
// ---------------------------------------------------------------------------

/// One device-visible window: CPU physical `[physical, physical+size)` appears
/// on the device bus at `bus`. The RTL8125 inbound iATU windows are 4 KiB
/// aligned and wholly below 4 GiB; GMAC0 uses [`Addressing::Identity32`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub physical: u64,
    pub bus: u64,
    pub size: u64,
}

impl Window {
    /// Device (bus) address of a CPU-physical range, or `None` when the range
    /// leaves the window or overflows. Never a pointer cast.
    pub fn translate(&self, physical: u64, size: u64) -> Option<u64> {
        if size == 0 {
            return None;
        }
        let end = physical.checked_add(size)?;
        if physical < self.physical || end > self.physical.checked_add(self.size)? {
            return None;
        }
        let offset = physical - self.physical;
        let bus = self.bus.checked_add(offset)?;
        bus.checked_add(size - 1)?;
        Some(bus)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Addressing {
    /// GMAC0 on the RK3568: locked EQoS programming uses 32-bit low physical
    /// identity addresses. High address words are zeroed by the driver.
    Identity32,
    /// PCIe endpoint behind an inbound address-match window.
    Window(Window),
}

/// Device address of a CPU-physical range under one device's addressing.
pub fn translate(
    addressing: &Addressing,
    physical: u64,
    size: u64,
    meta: usize,
) -> Option<u64> {
    let result = match addressing {
        Addressing::Identity32 => {
            let end = physical.checked_add(size)?;
            (size != 0 && end - 1 <= u64::from(u32::MAX)).then_some(physical)
        }
        Addressing::Window(window) => window.translate(physical, size),
    };
    if result.is_none() {
        count(meta, C_MAPPING_FAILURES);
        count(meta, C_FALLBACK_MAPPING);
    }
    result
}

/// CPU-physical address of a pool buffer. The pool base must be 2048-aligned.
pub fn buffer_physical(pool_physical: u64, id: u32) -> Option<u64> {
    if id >= BUFFERS as u32 || pool_physical & (BUFFER as u64 - 1) != 0 {
        return None;
    }
    pool_physical.checked_add(u64::from(id) * BUFFER as u64)
}

// ---------------------------------------------------------------------------
// Barriers
// ---------------------------------------------------------------------------

pub const C_CACHE_MAINTENANCE: usize = 27;

#[derive(Clone, Copy)]
pub enum PayloadCache {
    BeforeRx,
    AfterRx(usize),
    BeforeTx(usize),
}

pub fn maintain_payload(r: &Ref, operation: PayloadCache) -> Result<(), ()> {
    #[cfg(all(target_arch = "aarch64", target_os = "none",
              any(feature = "rawio", feature = "lan-port", feature = "forward-workers")))]
    if matches!(option_env!("R5S_ZC_CACHE"), Some("writeback")) {
        let start = payload_cache_address(r.id()).ok_or(())?;
        let length = match operation {
            PayloadCache::BeforeRx => BUFFER,
            PayloadCache::AfterRx(length) | PayloadCache::BeforeTx(length) => length,
        };
        if length == 0 || length > BUFFER {
            return Err(());
        }
        let end = start + ((length + 63) & !63);
        core::sync::atomic::compiler_fence(Ordering::SeqCst);
        let error = sel4::with_ipc_buffer_mut(|buffer| {
            let raw = buffer.inner_mut();
            match operation {
                PayloadCache::BeforeRx => raw.seL4_ARM_VSpace_CleanInvalidate_Data(3, start as _, end as _),
                PayloadCache::AfterRx(_) => raw.seL4_ARM_VSpace_Invalidate_Data(3, start as _, end as _),
                PayloadCache::BeforeTx(_) => raw.seL4_ARM_VSpace_Clean_Data(3, start as _, end as _),
            }
        });
        if error != 0 { return Err(()); }
        barrier::dma_publish();
        count(META_VA, C_CACHE_MAINTENANCE);
        return Ok(());
    }
    let _ = (r, operation);
    // The Normal-NC policy needs no maintenance. Under the write-back policy
    // reaching this point means this build has no maintenance path compiled
    // in — report failure so callers never treat the payload as DMA-ready
    // (a silently skipped clean corrupts the frame on the wire).
    if matches!(option_env!("R5S_ZC_CACHE"), Some(v) if matches!(v.as_bytes(), b"writeback")) {
        return Err(());
    }
    Ok(())
}

/// Clean a burst of TX-bound buffers with one cache syscall (write-back
/// policy). Pool IDs ascend within a burst, so one Clean over [min, max] beats
/// one syscall per buffer: the operation only writes back dirty lines
/// (data-preserving) and cache maintenance is local to this core, so
/// intervening buffers are unaffected. Spans above 64 KiB fall back to
/// per-buffer maintenance.
pub fn maintain_payload_tx_span(refs: &[Ref]) -> Result<(), ()> {
    #[cfg(all(target_arch = "aarch64", target_os = "none",
              any(feature = "rawio", feature = "lan-port", feature = "forward-workers")))]
    if matches!(option_env!("R5S_ZC_CACHE"), Some("writeback")) {
        if refs.is_empty() {
            return Ok(());
        }
        let mut start = usize::MAX;
        let mut end = 0usize;
        for r in refs {
            let base = payload_cache_address(r.id()).ok_or(())?;
            start = start.min(base);
            end = end.max(base + BUFFER);
        }
        if end - start > 65536 {
            for r in refs {
                maintain_payload(r, PayloadCache::BeforeTx(BUFFER))?;
            }
            return Ok(());
        }
        core::sync::atomic::compiler_fence(Ordering::SeqCst);
        let error = sel4::with_ipc_buffer_mut(|buffer| {
            let raw = buffer.inner_mut();
            raw.seL4_ARM_VSpace_Clean_Data(3, start as _, end as _)
        });
        if error != 0 {
            return Err(());
        }
        barrier::dma_publish();
        count_n(META_VA, C_CACHE_MAINTENANCE, refs.len() as u32);
        return Ok(());
    }
    let _ = refs;
    if matches!(option_env!("R5S_ZC_CACHE"), Some(v) if matches!(v.as_bytes(), b"writeback")) {
        return Err(());
    }
    Ok(())
}

fn payload_cache_address(id: u32) -> Option<usize> {
    (id < BUFFERS as u32).then(|| POOL_VA + id as usize * BUFFER)
}

#[test]
fn payload_cache_ranges_do_not_cross_pages_or_neighbor_buffers() {
    for id in 0..BUFFERS as u32 {
        let start = payload_cache_address(id).unwrap();
        assert_eq!(start % 64, 0);
        assert_eq!(start / 4096, (start + BUFFER - 1) / 4096);
        assert_eq!(start + BUFFER, POOL_VA + (id as usize + 1) * BUFFER);
    }
    assert_eq!(payload_cache_address(BUFFERS as u32), None);
}

/// Ordering around DMA-visible ownership changes.
pub mod barrier {
    use core::sync::atomic::{compiler_fence, Ordering};
    #[cfg(not(target_arch = "aarch64"))]
    use core::sync::atomic::fence;

    /// The default is `dsb sy` (full system): the project does not weaken
    /// DMA barriers until the weaker form is validated on the board. The
    /// outer-shareable pair (`dma_wmb`/`dma_rmb` equivalents — `dsb oshst` /
    /// `dsb oshld`, store/load completion to the point of coherency) is the
    /// opt-in A/B via `R5S_ZC_DMA_BARRIERS=outer`, matching the B51 option.
    /// CPU-to-CPU queue publication is already ordered by the queues'
    /// release/acquire cursors and is not covered by this switch.
    const OUTER_DMA: bool = matches!(option_env!("R5S_ZC_DMA_BARRIERS"), Some(v) if matches!(v.as_bytes(), b"outer"));

    /// Issue before handing a buffer to a device (descriptor OWN, queue push):
    /// payload writes are complete and ordered before the device may read.
    pub fn dma_publish() {
        compiler_fence(Ordering::SeqCst);
        #[cfg(target_arch = "aarch64")]
        unsafe {
            if OUTER_DMA {
                core::arch::asm!("dsb oshst", options(nostack));
            } else {
                core::arch::asm!("dsb sy", options(nostack));
            }
        }
        #[cfg(not(target_arch = "aarch64"))]
        fence(Ordering::SeqCst);
        compiler_fence(Ordering::SeqCst);
    }

    /// Issue before reading completion data or a buffer the device wrote.
    pub fn dma_consume() {
        compiler_fence(Ordering::SeqCst);
        #[cfg(target_arch = "aarch64")]
        unsafe {
            if OUTER_DMA {
                core::arch::asm!("dsb oshld", options(nostack));
            } else {
                core::arch::asm!("dsb sy", options(nostack));
            }
        }
        #[cfg(not(target_arch = "aarch64"))]
        fence(Ordering::SeqCst);
        compiler_fence(Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn closing_worker_lane_preserves_a_concurrent_publication() {
        use super::*;
        let memory = [const { AtomicU32::new(0) }; 0xf00 / 4];
        let refs = memory.as_ptr() as usize;
        assert!(publish_worker_lan_tx(refs, || {
            assert!(!close_worker_lan_tx(refs));
            assert!(!worker_lan_consumer_stopped(refs));
            release_worker_lan_consumer(refs);
            RefQueue::push(refs, &Ref::new(0, 1)) == Ok(true)
        }));
        assert!(worker_lan_tx_closed(refs) && close_worker_lan_tx(refs));
        assert!(worker_lan_consumer_stopped(refs));
        assert!(!publish_worker_lan_tx(refs, || panic!()));
        assert!(matches!(RefQueue::pop(refs), Ok(Some(_))));
        retire_worker_lan_tx(refs);
        assert!(worker_lan_tx_drained(refs));
    }
    #[test]
    fn direct_lan_remains_busy_after_pop_until_tx_retirement() {
        use super::*;
        let memory = [const { AtomicU32::new(0) }; 0xf00 / 4];
        let refs = memory.as_ptr() as usize;
        assert!(worker_lan_tx_drained(refs));
        assert!(!publish_worker_lan_tx(refs, || false));
        assert!(worker_lan_tx_drained(refs));
        assert!(publish_worker_lan_tx(refs, || RefQueue::push(refs, &Ref::new(0, 1)) == Ok(true)));
        assert!(matches!(RefQueue::pop(refs), Ok(Some(_))));
        assert!(!worker_lan_tx_drained(refs));
        retire_worker_lan_tx(refs);
        assert!(worker_lan_tx_drained(refs));
    }
    #[test]
    fn direct_lan_path_cannot_overtake_fallback_or_switch_while_queued() {
        use super::*;
        let mut path = 0;
        assert_eq!(select_lan_tx_path(&mut path, Some(0), true, |_| panic!()), Ok(Some(0)));
        assert_eq!(select_lan_tx_path(&mut path, Some(1), true, |port| {assert_eq!(port,0);false}), Err(()));
        assert_eq!(select_lan_tx_path(&mut path, Some(0), true, |_| panic!()), Ok(Some(0)));
        assert_eq!(select_lan_tx_path(&mut path, None, true, |_| true), Ok(None));
        // Fallback sticks while the client's fallback tokens are unsettled...
        assert_eq!(select_lan_tx_path(&mut path, Some(1), false, |_| panic!()), Ok(None));
        assert_eq!(select_lan_tx_path(&mut path, None, false, |_| panic!()), Ok(None));
        // ...and the client returns to the direct lane only once they retire.
        assert_eq!(select_lan_tx_path(&mut path, Some(1), true, |_| panic!()), Ok(Some(1)));
        assert_eq!(path, 3);
    }
    #[test]
    fn lan_return_tokens_settle_after_every_armed_send_retires() {
        use super::*;
        let meta = Meta::new();
        let base = meta.base();
        assert!(lan_token_settled(base, 2, 42));
        let first = arm_lan_token(base, 2, 42);
        assert_eq!(first, 2 << 8 | 42);
        let second = arm_lan_token(base, 2, 42);
        let other = arm_lan_token(base, 1, 42);
        assert!(!lan_token_settled(base, 2, 42));
        assert!(!lan_token_settled(base, 1, 42));
        retire_lan_token(base, first);
        retire_lan_token(base, other);
        assert!(!lan_token_settled(base, 2, 42), "second send still in flight");
        assert!(lan_token_settled(base, 1, 42), "other worker unaffected");
        retire_lan_token(base, second);
        assert!(lan_token_settled(base, 2, 42));
    }
    #[test]
    fn flooded_tokens_settle_only_after_every_copy_retires() {
        use super::*;
        let meta = Meta::new();
        let base = meta.base();
        let stamp = arm_lan_token(base, 0, 7);
        extend_lan_token(base, stamp, 2);
        retire_lan_token(base, stamp);
        assert!(!lan_token_settled(base, 0, 7), "one of two flood copies done");
        retire_lan_token(base, stamp);
        assert!(lan_token_settled(base, 0, 7));
    }
    #[test]
    fn token_copy_markers_carry_the_stamp_and_the_copy_precedes_them() {
        use super::*;
        let memory: Vec<AtomicU32> = (0..RQ_BYTES / 4).map(|_| AtomicU32::new(0)).collect();
        let base = memory.as_ptr() as usize;
        let mut slots = 0;
        let stamp = (3u32 << 8) | 9;
        assert_eq!(RefQueue::push_copy_token(base, stamp, || {
            slots += 1;
            Ok(true)
        }), Ok(true));
        assert_eq!(slots, 1, "the copy slot is published before the marker");
        let marker = RefQueue::pop(base).unwrap().expect("marker");
        assert_eq!(marker.flags, FLAG_COPY | FLAG_LAN_TOKEN);
        assert_eq!(marker.meta, stamp);
        assert_eq!(RefQueue::pop(base), Ok(None));
    }
    #[test]
    fn learn_coalescer_suppresses_duplicates_but_never_mac_moves() {
        use super::*;
        let mac = [2, 3, 4, 5, 6, 7];
        let mut coalescer = LearnCoalescer::new();
        assert!(coalescer.due(&mac, 1, 7, 100));
        coalescer.pushed(&mac, 1, 7, 100);
        assert!(!coalescer.due(&mac, 1, 7, 100 + LEARN_COALESCE_MS - 1));
        assert!(coalescer.due(&mac, 1, 7, 100 + LEARN_COALESCE_MS));
        assert!(coalescer.due(&mac, 2, 7, 101), "MAC move to the other port");
        assert!(coalescer.due(&mac, 1, 8, 101), "revision change");
        assert!(coalescer.due(&[2, 3, 4, 5, 6, 8], 1, 7, 101), "different MAC");
        coalescer.pushed(&mac, 2, 7, 101);
        assert!(!coalescer.due(&mac, 2, 7, 150));
        assert!(coalescer.due(&mac, 1, 7, 150), "key includes the ingress LAN");
    }
    #[test]
    fn learn_batch_coalesces_cross_worker_duplicates_but_flushes_mac_moves() {
        use super::*;
        let (a, b) = ([2, 3, 4, 5, 6, 7], [2, 3, 4, 5, 6, 8]);
        let mut batch = LearnBatch::new();
        // The same client learned by two workers (same port) collapses to one.
        assert!(batch.push(1, a));
        assert!(batch.push(1, a));
        assert!(batch.push(0, b));
        let mut applied = Vec::new();
        batch.apply(|port, mac| applied.push((port, mac)));
        assert_eq!(applied, vec![(1, a), (0, b)]);
        // A MAC move must not be reordered past later records: refuse and
        // flush first, then record the move alone.
        assert!(batch.push(0, a));
        assert!(!batch.push(1, a), "MAC move flushes first");
        let mut applied = Vec::new();
        batch.apply(|port, mac| applied.push((port, mac)));
        assert_eq!(applied, vec![(0, a)]);
        assert!(batch.push(1, a));
        // Capacity also forces a flush.
        assert!(batch.push(0, [9; 6]));
        let mut fills = 0;
        while batch.push(2, [7, 0, 0, 0, 0, fills as u8]) { fills += 1; }
        assert_eq!(fills, LearnBatch::CAPACITY - 2, "one taken by [9;6], one by a");
    }
    #[test]
    fn lan_route_snapshot_expires_and_removes_entries() {
        use super::*;
        let page = [const { AtomicU32::new(0) }; 1 + LAN_ROUTE_SLOTS * 5];
        let base = page.as_ptr() as usize;
        let mac = [2, 3, 4, 5, 6, 7];
        let until = (1 << 32) + 10;
        let routes = [(42, mac, 1, until), (106, mac, 0, until)];
        publish_lan_routes(base, routes.into_iter());
        let sequence = page[0].load(Ordering::Relaxed);
        publish_lan_routes(base, routes.into_iter());
        assert_eq!(page[0].load(Ordering::Relaxed), sequence);
        assert_eq!(lan_route(base, 106, until - 1), Ok(Some((mac, 0, until))));
        assert_eq!(lan_route(base, 42, until - 1), Ok(Some((mac, 1, until))));
        assert_eq!(lan_route(base, 42, until), Ok(None));
        page[0].fetch_add(1, Ordering::Relaxed);
        assert_eq!(lan_route(base, 42, until - 1), Err(()));
        assert_eq!(lan_route(base, 99, until - 1), Err(()));
        page[0].fetch_add(1, Ordering::Relaxed);
        publish_lan_routes(base, core::iter::empty());
        assert_eq!(lan_route(base, 42, 0), Ok(None));
    }

    #[test]
    fn direct_lan_tx_rejects_changed_route_and_revision() {
        use super::*;
        let page = [const { AtomicU32::new(0) }; 1024];
        let control = page.as_ptr() as usize;
        let routes = control + LAN_ROUTE_OFFSET;
        page[0].store(2, Ordering::Relaxed);
        page[1].store(7, Ordering::Relaxed);
        page[3].store(1, Ordering::Relaxed);
        let mac = [2, 3, 4, 5, 6, 7];
        let mut frame = [0u8; 60];
        frame[..6].copy_from_slice(&mac);
        frame[12..15].copy_from_slice(&[8, 0, 0x45]);
        frame[33] = 42;
        publish_lan_routes(routes, [(42, mac, 0, 100)].into_iter());
        assert!(worker_lan_tx_valid(control, &frame, 0, 7, 9));
        assert!(!worker_lan_tx_valid(control, &frame, 0, 8, 9));
        publish_lan_routes(routes, [(42, mac, 1, 100)].into_iter());
        assert!(!worker_lan_tx_valid(control, &frame, 0, 7, 9));
    }
    #[test]
    fn tx_checksum_seeds_only_nat_validated_supported_frames() {
        use super::{FLAG_NAT_VALIDATED, TxChecksum, prepare_nat_tx_checksum};
        let mut tcp = [0u8; 58];
        tcp[12..15].copy_from_slice(&[8, 0, 0x45]);
        tcp[16..18].copy_from_slice(&44u16.to_be_bytes());
        tcp[23] = 6;
        tcp[24..34].copy_from_slice(&[0xab, 0xcd, 192, 0, 2, 1, 198, 51, 100, 5]);
        tcp[46] = 0x50;
        tcp[50..52].copy_from_slice(&0x9876u16.to_be_bytes());
        let original = tcp;
        assert_eq!(prepare_nat_tx_checksum(&mut tcp, 0, true), TxChecksum::None);
        assert_eq!(tcp, original);
        assert_eq!(prepare_nat_tx_checksum(&mut tcp, FLAG_NAT_VALIDATED, true), TxChecksum::Tcp);
        assert_eq!(tcp[24..26], [0, 0]);
        assert_eq!(tcp[50..52], 0xec58u16.to_be_bytes());
        assert_eq!(tcp[52..], original[52..]);
        let mut gmac = original;
        assert_eq!(prepare_nat_tx_checksum(&mut gmac, FLAG_NAT_VALIDATED, false), TxChecksum::Tcp);
        assert_eq!(gmac[50..52], [0, 0]);

        let mut udp = original;
        udp[23] = 17;
        udp[38..40].copy_from_slice(&24u16.to_be_bytes());
        let mut zero = udp;
        udp[40..42].copy_from_slice(&0x1234u16.to_be_bytes());
        assert_eq!(prepare_nat_tx_checksum(&mut udp, FLAG_NAT_VALIDATED, true), TxChecksum::Udp);
        assert_eq!(udp[40..42], 0xec63u16.to_be_bytes());
        assert_eq!(prepare_nat_tx_checksum(&mut zero, FLAG_NAT_VALIDATED, true), TxChecksum::Ipv4);
        assert_eq!(zero[40..42], [0, 0]);
        let mut fragment = original;
        fragment[20] = 0x20;
        let expected = fragment;
        assert_eq!(prepare_nat_tx_checksum(&mut fragment, FLAG_NAT_VALIDATED, true), TxChecksum::None);
        assert_eq!(fragment, expected);
        let mut malformed = original;
        malformed[46] = 0;
        let expected = malformed;
        assert_eq!(prepare_nat_tx_checksum(&mut malformed, FLAG_NAT_VALIDATED, true), TxChecksum::None);
        assert_eq!(malformed, expected);
    }

    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn worker_wan_route_and_mixed_queue_require_current_metadata() {
        let route = [const { AtomicU32::new(0) }; 6];
        let base = route.as_ptr() as usize;
        let mac = [2, 1, 2, 3, 4, 5];
        let until = (1 << 32) + 50;
        publish_wan_route(base, 7, mac, until);
        assert_eq!(wan_route(base, 7, until - 1), Some(mac));
        assert_eq!(wan_route(base, 8, until - 1), None);
        assert_eq!(wan_route(base, 7, until), None);
        route[0].fetch_add(1, Ordering::Relaxed);
        assert_eq!(wan_route(base, 7, until - 1), None);
        assert_eq!(learned_mac(source_learn(&mac, 2, 7)), mac);

        let queue = [0u32; 0x400];
        let base = queue.as_ptr() as usize;
        let first = Ref { epoch: 7, meta: 49, ..Ref::new(2, 60) };
        assert_eq!(RefQueue::push(base, &first), Ok(true));
        assert_eq!(RefQueue::push_copy_at(base, 7, 50, || Ok(true)), Ok(true));
        assert_eq!(RefQueue::peek(base).unwrap().unwrap().meta, 49);
        assert_eq!(RefQueue::pop(base).unwrap().unwrap().meta, 49);
        let copy = RefQueue::pop(base).unwrap().unwrap();
        assert_eq!((copy.flags, copy.epoch, copy.meta), (FLAG_COPY, 7, 50));

        let mut frame = [0; 60];
        frame[12..15].copy_from_slice(&[8, 0, 0x45]);
        frame[16..18].copy_from_slice(&28u16.to_be_bytes());
        assert_eq!(worker_wan_header(&mut frame, mac), Some(42));
        assert_eq!(&frame[..6], &mac);
        assert_eq!(&frame[6..12], &WAN_MAC);
        frame[17] = 61;
        assert_eq!(worker_wan_header(&mut frame, mac), None);
    }

    struct Meta {
        words: Vec<AtomicU32>,
    }
    impl Meta {
        fn new() -> Self {
            let words = (0..META_BYTES / 4).map(|_| AtomicU32::new(0)).collect();
            let meta = Self { words };
            init(meta.base());
            meta
        }
        fn base(&self) -> usize {
            self.words.as_ptr() as usize
        }
    }

    #[test]
    fn stopped_tx_cancels_without_reporting_a_completed_packet() {
        let meta = Meta::new();
        let base = meta.base();
        let r = alloc(base, PARTITION_WAN).unwrap();
        publish(base, &r).unwrap();
        submit(base, &r).unwrap();
        cancel_stopped_tx(base, &r).unwrap();
        assert_eq!(status(base, r.id()).0, FREE);
        assert_eq!(counter(base, C_DROPPED).load(Ordering::Relaxed), 1);
        assert_eq!(counter(base, C_TX_COMPLETED).load(Ordering::Relaxed), 0);
        assert!(cancel_stopped_tx(base, &r).is_err());
    }

    #[test]
    fn fast_wan_revision_requires_a_current_published_lease() {
        let memory = [const { AtomicU32::new(0) }; 32];
        let base = memory.as_ptr() as usize;
        assert_eq!(fast_wan_tag(base, 1), None);
        word(base, 0).store(2, Ordering::Relaxed);
        word(base, 4).store(27, Ordering::Relaxed);
        word(base, 12).store(1, Ordering::Relaxed);
        word(base, 52).store(3, Ordering::Relaxed);
        assert_eq!(fast_wan_tag(base, 1), None);
        assert_eq!(fast_wan_tag(base, 3), Some(27));
        word(base, 0).store(3, Ordering::Relaxed);
        assert_eq!(fast_wan_tag(base, 3), None);
    }

    #[test]
    fn wan_fast_packet_keeps_control_and_fragments_on_legacy_path() {
        let mut frame = [0; 60];
        frame[..6].copy_from_slice(&WAN_MAC);
        frame[12..15].copy_from_slice(&[8, 0, 0x45]);
        frame[16..18].copy_from_slice(&40u16.to_be_bytes());
        frame[23] = 6;
        frame[36..38].copy_from_slice(&49152u16.to_be_bytes());
        assert!(wan_fast_packet(&frame, true));
        frame[20] = 0x20;
        assert!(!wan_fast_packet(&frame, true));
        frame[20] = 0;
        frame[23] = 1;
        assert!(!wan_fast_packet(&frame, true));
        frame[23] = 17;
        for port in [68u16, 48125, 49151] {
            frame[36..38].copy_from_slice(&port.to_be_bytes());
            assert!(!wan_fast_packet(&frame, true));
        }
        assert!(!wan_fast_packet(&frame[..30], true));
    }

    #[test]
    fn worker_tx_wake_covers_publication_on_both_sides_of_rearm() {
        let memory: Vec<AtomicU32> = (0..RQ_BYTES / 4).map(|_| AtomicU32::new(0)).collect();
        let base = memory.as_ptr() as usize;
        let packet = Ref::new(1, 40);

        assert!(RefQueue::claim_wake(base));
        assert_eq!(RefQueue::push(base, &packet), Ok(true));
        assert!(!RefQueue::claim_wake(base));
        RefQueue::rearm_wake(base);
        assert_eq!(RefQueue::pop(base), Ok(Some(packet)));
        assert_eq!(RefQueue::pop(base), Ok(None));
        assert_eq!(RefQueue::push_copy_at(base, 7, 10, || Ok(true)), Ok(true));
        assert!(RefQueue::claim_wake(base));
        let copy = RefQueue::pop(base).unwrap().unwrap();
        assert_eq!((copy.flags, copy.epoch, copy.meta), (FLAG_COPY, 7, 10));
    }

    #[test]
    fn copy_markers_share_reference_fifo_and_respect_backpressure() {
        let memory: Vec<AtomicU32> = (0..RQ_BYTES / 4).map(|_| AtomicU32::new(0)).collect();
        let base = memory.as_ptr() as usize;
        rq_head(base).store(u32::MAX - 1, Ordering::Relaxed);
        rq_tail(base).store(u32::MAX - 1, Ordering::Relaxed);
        let before = Ref::new(1, 40);
        let after = Ref::new(2, 41);
        assert_eq!(RefQueue::push(base, &before), Ok(true));
        assert_eq!(RefQueue::push_copy(base, || Ok(false)), Ok(false));
        assert_eq!(RefQueue::push_copy(base, || Ok(true)), Ok(true));
        assert_eq!(RefQueue::push(base, &after), Ok(true));
        for _ in 3..RQ_COUNT {
            assert_eq!(RefQueue::push(base, &after), Ok(true));
        }
        assert_eq!(RefQueue::push_copy(base, || panic!("copy on full reference queue")), Ok(false));
        assert_eq!(RefQueue::pop(base), Ok(Some(before)));
        assert_eq!(RefQueue::pop(base).unwrap().unwrap().flags, FLAG_COPY);
        assert_eq!(RefQueue::pop(base), Ok(Some(after)));
        assert!(RefQueue::claim_wake(base));
        assert!(!RefQueue::claim_wake(base));
        RefQueue::rearm_wake(base);
        assert!(RefQueue::claim_wake(base));
    }

    #[test]
    fn ingress_pair_publication_flushes_before_copy_and_releases_unqueued_buffers() {
        let meta = Meta::new();
        let memory: Vec<AtomicU32> = (0..RQ_BYTES / 4).map(|_| AtomicU32::new(0)).collect();
        let base = memory.as_ptr() as usize;
        let cpu = || {
            let packet = alloc(meta.base(), PARTITION_WAN).unwrap();
            publish(meta.base(), &packet).unwrap();
            packet
        };
        let mut ingress = IngressBatch::new(meta.base());
        let [a, b, c] = [cpu(), cpu(), cpu()];
        assert_eq!(ingress.offer(base, a, true, 0, |_, _| unreachable!()), 0);
        assert_eq!(RefQueue::peek(base), Ok(None));
        assert_eq!(ingress.offer(base, b, false, 1, |first, last| [first, last] == [a, b]), 0);
        assert_eq!(RefQueue::pop_pair_if(base, |_| true), Ok(Some([a, b])));
        assert_eq!(ingress.offer(base, c, true, 2, |_, _| unreachable!()), 0);
        assert_eq!(ingress.flush(), 0);
        assert_eq!(RefQueue::push_copy(base, || Ok(true)), Ok(true));
        assert_eq!(RefQueue::pop(base), Ok(Some(c)));
        assert_eq!(RefQueue::pop(base).unwrap().unwrap().flags, FLAG_COPY);

        let [d, e] = [cpu(), cpu()];
        assert_eq!(ingress.offer(base, d, true, 0, |_, _| unreachable!()), 0);
        assert_eq!(ingress.flush_expired(|| 1001), 0);
        assert_eq!(RefQueue::peek(base), Ok(Some(d)));
        assert_eq!(ingress.offer(base, e, false, 1001, |_, _| panic!("expired pair compared")), 0);
        assert_eq!(RefQueue::pop_pair_if(base, |_| true), Ok(Some([d, e])));
        let first_epoch = cpu();
        let next_epoch = Ref { epoch: 1, ..cpu() };
        ingress.offer(base, first_epoch, true, 0, |_, _| unreachable!());
        assert_eq!(ingress.offer(base, next_epoch, false, 1, |_, _| panic!("route revision changed")), 0);
        assert_eq!(RefQueue::pop(base), Ok(Some(first_epoch)));
        assert_eq!(RefQueue::pop(base), Ok(Some(next_epoch)));

        for _ in 1..RQ_COUNT {
            assert_eq!(RefQueue::push_copy(base, || Ok(true)), Ok(true));
        }
        let [first, last] = [cpu(), cpu()];
        assert_eq!(ingress.offer(base, first, true, 0, |_, _| unreachable!()), 0);
        assert_eq!(ingress.offer(base, last, false, 1, |_, _| true), 1);
        assert_eq!(status(meta.base(), first.id()).0, CPU);
        assert_eq!(status(meta.base(), last.id()).0, FREE);
        let pending = cpu();
        ingress.offer(base, pending, true, 2, |_, _| unreachable!());
        drop(ingress);
        assert_eq!(status(meta.base(), pending.id()).0, FREE);
    }

    #[test]
    fn adjacent_pair_wrap_and_copy_markers_preserve_fifo() {
        let memory: Vec<AtomicU32> = (0..RQ_BYTES / 4).map(|_| AtomicU32::new(0)).collect();
        let base = memory.as_ptr() as usize;
        rq_head(base).store(u32::MAX, Ordering::Relaxed);
        rq_tail(base).store(u32::MAX, Ordering::Relaxed);
        let pair = [Ref::new(4, 50), Ref::new(5, 51)];
        assert_eq!(RefQueue::push_batch(base, &pair), Ok(true));
        assert_eq!(RefQueue::pop_pair_if(base, |_| false), Ok(None));
        assert_eq!(RefQueue::consumed(base), u32::MAX);
        assert_eq!(RefQueue::pop_pair_if(base, |entries| entries == pair), Ok(Some(pair)));
        assert_eq!(RefQueue::consumed(base), 1);
        assert_eq!(RefQueue::push_copy(base, || Ok(true)), Ok(true));
        assert_eq!(RefQueue::push(base, &pair[0]), Ok(true));
        assert_eq!(RefQueue::pop_pair_if(base, |_| panic!("copy marker inspected")), Ok(None));
        assert_eq!(RefQueue::pop(base).unwrap().unwrap().flags, FLAG_COPY);
        assert_eq!(RefQueue::pop(base), Ok(Some(pair[0])));
    }

    #[test]
    fn ownership_transitions_follow_the_documented_state_machine() {
        let meta = Meta::new();
        let base = meta.base();
        let r = alloc(base, PARTITION_WAN).expect("free buffers");
        assert_eq!(status(base, r.id()), (RX_DMA, r.generation()));
        assert_eq!(publish(base, &r), Ok(()));
        assert_eq!(status(base, r.id()), (CPU, r.generation()));
        assert_eq!(submit(base, &r), Ok(()));
        assert_eq!(status(base, r.id()), (TX_DMA, r.generation()));
        assert_eq!(complete(base, &r), Ok(()));
        assert_eq!(status(base, r.id()), (FREE, r.generation()));
        assert_eq!(counter(base, C_OWNERSHIP_ERRORS).load(Ordering::Relaxed), 0);
    }

    #[test]
    fn invalid_transitions_are_refused_and_counted() {
        let meta = Meta::new();
        let base = meta.base();
        let r = alloc(base, PARTITION_LAN1).unwrap();
        // publish twice, submit before publish, complete before submit.
        assert_eq!(publish(base, &r), Ok(()));
        assert_eq!(publish(base, &r), Err(()));
        assert_eq!(complete(base, &r), Err(()));
        assert_eq!(submit(base, &r), Ok(()));
        assert_eq!(submit(base, &r), Err(()));
        assert_eq!(counter(base, C_OWNERSHIP_ERRORS).load(Ordering::Relaxed), 3);
    }

    #[test]
    fn double_free_and_stale_references_are_rejected() {
        let meta = Meta::new();
        let base = meta.base();
        let first = alloc(base, PARTITION_LAN2).unwrap();
        assert_eq!(publish(base, &first), Ok(()));
        assert_eq!(submit(base, &first), Ok(()));
        assert_eq!(complete(base, &first), Ok(()));
        // The generation advanced at the next allocation; the old handle can
        // never release the new life of this buffer.
        let second = alloc(base, PARTITION_LAN2).unwrap();
        assert_eq!(second.id(), first.id());
        assert_ne!(second.generation(), first.generation());
        assert_eq!(publish(base, &first), Err(()));
        assert_eq!(reject(base, &first), Err(()));
        assert_eq!(status(base, second.id()), (RX_DMA, second.generation()));
        assert_eq!(counter(base, C_OWNERSHIP_ERRORS).load(Ordering::Relaxed), 2);
    }

    #[test]
    fn allocator_partitions_quota_reuse_and_exhaustion() {
        let meta = Meta::new();
        let base = meta.base();
        let (first, end) = PARTITION_BOUNDS[PARTITION_WAN];
        let mut taken = Vec::new();
        for _ in first..end {
            taken.push(alloc(base, PARTITION_WAN).expect("quota"));
        }
        assert!(alloc(base, PARTITION_WAN).is_none());
        assert!(alloc(base, PARTITION_LAN1).is_some(), "other partition unaffected");
        let r = taken.pop().unwrap();
        assert_eq!(reject(base, &r), Err(()), "an unpublished RX buffer recycles, it does not reject");
        assert_eq!(recycle(base, &r), Ok(()));
        assert!(alloc(base, PARTITION_WAN).is_some(), "freed buffer is reused");
        let (used, free, _) = occupancy(base);
        assert_eq!(used, (end - first) as u32 + 1, "wan quota re-filled plus one LAN1 buffer");
        assert_eq!(free, BUFFERS as u32 - used);
    }

    #[test]
    fn recycle_returns_buffer_without_forwarding() {
        let meta = Meta::new();
        let base = meta.base();
        let r = alloc(base, PARTITION_LAN1).unwrap();
        assert_eq!(recycle(base, &r), Ok(()));
        assert_eq!(status(base, r.id()), (FREE, r.generation()));
        assert_eq!(counter(base, C_RX_RECYCLED).load(Ordering::Relaxed), 1);
        assert_eq!(counter(base, C_FORWARDED).load(Ordering::Relaxed), 0);
    }

    #[test]
    fn reference_queue_preserves_order_and_payload_across_wraparound() {
        let memory: Vec<AtomicU32> = (0..RQ_BYTES / 4).map(|_| AtomicU32::new(0)).collect();
        let base = memory.as_ptr() as usize;
        rq_head(base).store(u32::MAX - 2, Ordering::Relaxed);
        rq_tail(base).store(u32::MAX - 2, Ordering::Relaxed);
        for n in 0..RQ_COUNT {
            let r = Ref { handle: handle(n, n + 1), len: 60, flags: n as u16, epoch: 7, meta: 17 };
            assert_eq!(RefQueue::push(base, &r), Ok(true));
        }
        assert_eq!(RefQueue::push(base, &Ref::new(0, 0)), Ok(false), "backpressure");
        for n in 0..RQ_COUNT {
            let r = RefQueue::pop(base).unwrap().expect("entries in order");
            assert_eq!(r.handle, handle(n, n + 1));
            assert_eq!(r.len, 60);
            assert_eq!(r.flags, n as u16);
            assert_eq!(r.epoch, 7);
            assert_eq!(r.meta, 17);
        }
        assert_eq!(RefQueue::pop(base), Ok(None));
        assert_eq!(RefQueue::depth(base), Ok(0));
    }

    #[test]
    fn reference_queue_rejects_corrupt_cursors() {
        let memory: Vec<AtomicU32> = (0..RQ_BYTES / 4).map(|_| AtomicU32::new(0)).collect();
        let base = memory.as_ptr() as usize;
        rq_head(base).store(5, Ordering::Relaxed);
        rq_tail(base).store(5 + RQ_COUNT + 1, Ordering::Relaxed);
        assert_eq!(RefQueue::push(base, &Ref::new(1, 1)), Err(()));
        assert_eq!(RefQueue::pop(base), Err(()));
        assert_eq!(RefQueue::depth(base), Err(()));
    }

    #[test]
    fn dma_translation_enforces_windows_and_counts_failures() {
        let meta = Meta::new();
        let base = meta.base();
        let window = Window { physical: 0x2430_0000, bus: 0x8100_0000, size: 0x20_0000 };
        // Pool buffer 0 at CPU physical 0x2440_0000 is bus 0x8110_0000 here.
        assert_eq!(
            translate(&Addressing::Window(window), 0x2440_0000, 2048, base),
            Some(0x8110_0000)
        );
        // Exactly filling the window is valid; crossing its end is not.
        assert_eq!(
            translate(&Addressing::Window(window), 0x244f_f000, 2048, base),
            Some(0x811f_f000)
        );
        assert_eq!(translate(&Addressing::Window(window), 0x242f_f000, 2048, base), None);
        assert_eq!(translate(&Addressing::Window(window), 0x244f_f801, 2048, base), None);
        assert_eq!(translate(&Addressing::Window(window), 0x2440_0000, 0, base), None);
        // GMAC identity addressing is 32-bit; anything above 4 GiB fails.
        assert_eq!(translate(&Addressing::Identity32, 0x2440_0000, 2048, base), Some(0x2440_0000));
        assert_eq!(translate(&Addressing::Identity32, 0x1_0000_0000, 2048, base), None);
        assert_eq!(translate(&Addressing::Identity32, 0xffff_f800, 2048, base), Some(0xffff_f800));
        assert_eq!(translate(&Addressing::Identity32, 0xffff_f801, 2048, base), None);
        assert_eq!(counter(base, C_MAPPING_FAILURES).load(Ordering::Relaxed), 5);
    }

    #[test]
    fn buffer_addresses_require_aligned_pool_and_valid_ids() {
        assert_eq!(buffer_physical(0x2440_0000, 511), Some(0x2440_0000 + 511 * 2048));
        assert_eq!(buffer_physical(0x2440_0400, 0), None, "unaligned pool");
        assert_eq!(buffer_physical(0x2440_0000, BUFFERS as u32), None);
    }

    #[test]
    fn handle_packing_round_trips_and_masks_generations() {
        for &(id, generation) in &[(0, 0), (511, 1), (17, GEN_MASK), (3, GEN_MASK + 1)] {
            let h = handle(id, generation);
            assert_eq!(handle_id(h), id);
            assert_eq!(handle_generation(h), generation & GEN_MASK);
        }
    }

    /// The publish/consume protocol orders payload writes before the device
    /// may see them and completion reads after the device has finished. This
    /// models the NC-pool rule: no cache maintenance, only barriers.
    #[test]
    fn barriers_order_payload_before_device_publication() {
        let meta = Meta::new();
        let base = meta.base();
        let pool: Vec<AtomicU32> = (0..POOL_BYTES / 4).map(|_| AtomicU32::new(0)).collect();
        let payload = pool.as_ptr() as usize;
        let r = alloc(base, PARTITION_WAN).unwrap();
        let id = r.id() as usize;
        let device_viewed = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                // Device model: wait for TX_DMA, then read the payload.
                loop {
                    let (state, _) = status(base, handle_id(r.handle) as u32);
                    if state == TX_DMA {
                        barrier::dma_consume();
                        let seen = unsafe {
                            core::ptr::read_volatile((payload + id * BUFFER) as *const u32)
                        };
                        assert_eq!(seen, 0x5a5a_5a5a, "payload visible to the device");
                        device_viewed.store(true, Ordering::SeqCst);
                        break;
                    }
                    std::thread::yield_now();
                }
            });
            // CPU owner: write the payload, then publish and submit.
            unsafe {
                core::ptr::write_volatile((payload + id * BUFFER) as *mut u32, 0x5a5a_5a5a);
            }
            assert_eq!(publish(base, &r), Ok(()));
            barrier::dma_publish();
            assert_eq!(submit(base, &r), Ok(()));
            while !device_viewed.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
        });
        assert_eq!(counter(base, C_OWNERSHIP_ERRORS).load(Ordering::Relaxed), 0);
    }

    /// Three concurrent drivers allocate, forward and complete disjoint
    /// buffers while a fourth thread frees its own; no buffer is handed out
    /// twice and every reference releases exactly once.
    #[test]
    fn concurrent_owners_never_double_take_or_leak() {
        let meta = Meta::new();
        let base = meta.base();
        std::thread::scope(|scope| {
            for partition in 0..PARTITIONS {
                scope.spawn(move || {
                    for _ in 0..64 {
                        let Some(r) = alloc(base, partition) else { continue };
                        assert_eq!(status(base, r.id()), (RX_DMA, r.generation()));
                        assert_eq!(publish(base, &r), Ok(()));
                        assert_eq!(submit(base, &r), Ok(()));
                        assert_eq!(complete(base, &r), Ok(()));
                        // A stale duplicate must not release the next life.
                        assert_eq!(complete(base, &r), Err(()));
                    }
                });
            }
        });
        assert_eq!(counter(base, C_TX_COMPLETED).load(Ordering::Relaxed), 3 * 64);
        assert_eq!(counter(base, C_OWNERSHIP_ERRORS).load(Ordering::Relaxed), 3 * 64);
        let (used, free, _) = occupancy(base);
        assert_eq!(used, 0, "no buffer leaked");
        assert_eq!(free, BUFFERS as u32);
    }

    #[test]
    fn quiesce_after_stop_releases_every_buffer_and_invalidates_references() {
        let meta = Meta::new();
        let base = meta.base();
        let held: Vec<_> = (0..8).map(|_| alloc(base, PARTITION_LAN1).unwrap()).collect();
        for (i, r) in held.iter().enumerate() {
            match i % 3 {
                0 => {}
                1 => {
                    assert_eq!(publish(base, r), Ok(()));
                    assert_eq!(submit(base, r), Ok(()));
                }
                _ => assert_eq!(publish(base, r), Ok(())),
            }
        }
        // Safety: the test models a verified DMA stop; no device is running.
        unsafe { quiesce(base) };
        for r in &held {
            assert_eq!(status(base, r.id()).0, FREE);
            assert_eq!(complete(base, r), Err(()), "stale references are dead");
            assert_eq!(reject(base, r), Err(()));
        }
        let (used, free, _) = occupancy(base);
        assert_eq!(used, 0);
        assert_eq!(free, BUFFERS as u32);
    }

    #[test]
    fn concurrent_initializers_produce_one_consistent_free_list() {
        let words: Vec<AtomicU32> = (0..META_BYTES / 4).map(|_| AtomicU32::new(0)).collect();
        let base = words.as_ptr() as usize;
        assert!(!ready(base));
        assert!(alloc(base, PARTITION_WAN).is_none(), "no allocation before init");
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| init(base));
            }
        });
        assert!(ready(base));
        assert!(alloc(base, PARTITION_WAN).is_some());
    }

    #[test]
    fn metadata_layout_fits_the_region() {
        assert!(RQ_BYTES <= LINK_STRIDE);
        assert!(OWNERSHIP + BUFFERS * 4 <= FREE_BITS);
        assert_eq!(core::mem::size_of::<Ref>(), 16);
    }
}
