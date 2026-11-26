//! Host tests for the RTL8125 LAN TSO PD glue (`b51.rs`, contract
//! docs/tso-design.md §4-§7): a FLAG_TSO reference group drains to
//! `Ring::send_tso` with GiantSend descriptors, malformed groups free every
//! reference, the software fallback emits MTU-sized frames through
//! `Ring::send`, and buffers are released only when `retired_tso` reports
//! the request id.
#[path = "../pds/gmac_dma_corrected/src/tso.rs"]
mod tso;
#[path = "../pds/gmac_dma_corrected/src/zc.rs"]
mod zc;
#[path = "../pds/gmac_dma_corrected/src/tso_testgen.rs"]
mod tso_testgen;
#[path = "../drivers/rtl8125/pcie.rs"]
mod pcie;
#[path = "../drivers/rtl8125/mmio.rs"]
mod mmio;
#[path = "../drivers/rtl8125/mac.rs"]
mod mac;
#[path = "../drivers/rtl8125/ring.rs"]
mod ring;
#[path = "../drivers/rtl8125/dma.rs"]
mod dma;
#[path = "../drivers/rtl8125/provider.rs"]
mod provider;
#[path = "../pds/pcie_provider/src/arp.rs"]
mod arp;
#[path = "../pds/pcie_provider/src/b51.rs"]
mod b51;

fn progress(_: u32) {}
fn ticks() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CLOCK: AtomicU64 = AtomicU64::new(240_000);
    CLOCK.fetch_add(240_000, Ordering::Relaxed)
}

use std::collections::BTreeMap;
use std::ptr::NonNull;
use std::sync::atomic::AtomicU32;

const BAR: u64 = 0x1000;
const BUS: u64 = 0x8000_0000;
const SIZE: usize = 0x30_0000;
const POOL_OFF: usize = 0x20_0000;
const POOL_PA: u64 = 0x2440_0000;
const OWN: u32 = 1 << 31;
const FIRST: u32 = 1 << 29;
const LAST: u32 = 1 << 28;

struct Model {
    pool: NonNull<u8>,
    regs: BTreeMap<u64, u32>,
}

impl Model {
    fn new() -> Self {
        let layout = std::alloc::Layout::from_size_align(SIZE, 4096).unwrap();
        let pool = NonNull::new(unsafe { std::alloc::alloc_zeroed(layout) }).unwrap();
        Self { pool, regs: BTreeMap::new() }
    }
    fn ptr(&self, bus: u64, size: usize) -> *mut u8 {
        let offset = bus.checked_sub(BUS).unwrap() as usize;
        assert!(offset + size <= SIZE);
        unsafe { self.pool.as_ptr().add(offset) }
    }
}

impl Drop for Model {
    fn drop(&mut self) {
        unsafe {
            std::alloc::dealloc(
                self.pool.as_ptr(),
                std::alloc::Layout::from_size_align(SIZE, 4096).unwrap(),
            )
        }
    }
}

impl pcie::ConfigIo for Model {
    type Error = ();
    fn read32(&mut self, address: u64) -> Result<u32, ()> {
        Ok(self.regs.get(&address).copied().unwrap_or(0))
    }
    fn write32(&mut self, address: u64, value: u32) -> Result<(), ()> {
        self.regs.insert(address, value);
        Ok(())
    }
    fn delay_us(&mut self, _: u32) {}
}

impl mac::Registers for Model {
    fn read8(&mut self, a: u64) -> Result<u8, ()> {
        Ok(self.regs.get(&a).copied().unwrap_or(0) as u8)
    }
    fn read16(&mut self, a: u64) -> Result<u16, ()> {
        Ok(self.regs.get(&a).copied().unwrap_or(0) as u16)
    }
    fn write8(&mut self, a: u64, v: u8) -> Result<(), ()> {
        self.regs.insert(a, u32::from(v));
        Ok(())
    }
    fn write16(&mut self, a: u64, v: u16) -> Result<(), ()> {
        self.regs.insert(a, u32::from(v));
        Ok(())
    }
}

type TestDma = dma::PackedDma<Model, 64, true>;

struct Fixture {
    dma: TestDma,
    ring: ring::Ring<64>,
    meta: Vec<AtomicU32>,
    queue: Vec<AtomicU32>,
    state: b51::TsoState,
    submitted_at: [u64; 64],
    pool_va: usize,
}

impl Fixture {
    fn new() -> Self {
        let model = Model::new();
        let pool = model.pool;
        let memory = |offset: usize, size| dma::DmaMemory {
            cpu: NonNull::new(unsafe { pool.as_ptr().add(offset) }).unwrap(),
            physical: 0x2420_0000 + offset as u64,
            size,
        };
        let window = dma::InboundWindow { physical: 0x2420_0000, bus: BUS, size: SIZE as u64 };
        let mut dma = unsafe {
            TestDma::from_verified_memory(
                model,
                BAR,
                memory(0, TestDma::DESCRIPTOR_BYTES),
                memory(4096, 64 * 4096),
                &window,
            )
        }
        .unwrap();
        let (tx, rx, _) = dma.bus_addresses();
        dma.registers().regs.extend([(BAR + 0x20, tx as u32), (BAR + 0xe4, rx as u32)]);
        let mut ring = ring::Ring::<64>::new(BUS + 4096, 64 * 4096).unwrap();
        ring.initialize(&mut dma).unwrap();
        dma.enable_no_close().unwrap();
        let meta: Vec<AtomicU32> = (0..zc::META_BYTES / 4).map(|_| AtomicU32::new(0)).collect();
        let meta_base = meta.as_ptr() as usize;
        zc::init(meta_base);
        let queue: Vec<AtomicU32> = (0..zc::RQ_BYTES / 4).map(|_| AtomicU32::new(0)).collect();
        let pool_va = pool.as_ptr() as usize + POOL_OFF;
        let state = b51::TsoState::new(meta_base, pool_va, POOL_PA);
        Self { dma, ring, meta, queue, state, submitted_at: [0; 64], pool_va }
    }

    fn meta_base(&self) -> usize {
        self.meta.as_ptr() as usize
    }
    fn queue_base(&self) -> usize {
        self.queue.as_ptr() as usize
    }
    fn bus_of(&self, id: u32) -> u64 {
        BUS + POOL_OFF as u64 + u64::from(id) * 2048
    }
    fn cpu_slice(&self, id: u32) -> &'static mut [u8] {
        unsafe {
            core::slice::from_raw_parts_mut((self.pool_va + id as usize * zc::BUFFER) as *mut u8, zc::BUFFER)
        }
    }
    /// Allocate + publish one CPU-owned reference carrying group metadata.
    fn make_ref(&self, flags: u16, meta: u32, len: u16) -> zc::Ref {
        let r = zc::alloc(self.meta_base(), zc::PARTITION_LAN1).expect("pool buffer");
        assert_eq!(zc::publish(self.meta_base(), &r), Ok(()));
        zc::Ref { flags, len, meta, ..r }
    }
    fn state_of(&self, id: u32) -> u32 {
        zc::status(self.meta_base(), id).0
    }
}

fn group_refs(f: &Fixture) -> [zc::Ref; 3] {
    let meta = tso::pack_first_meta(1460, false, 20);
    [
        f.make_ref(tso::FLAG_TSO_FIRST, meta, 54 + 100),
        f.make_ref(tso::FLAG_TSO_CONT, 0, 1460),
        f.make_ref(tso::FLAG_TSO_CONT | tso::FLAG_TSO_LAST, 0, 1460),
    ]
}

fn push_all(f: &Fixture, refs: &[zc::Ref]) {
    for r in refs {
        assert_eq!(zc::RefQueue::push(f.queue_base(), r), Ok(true));
    }
}

fn drain_first(f: &mut Fixture, now: u64) {
    let first = zc::RefQueue::pop(f.queue_base()).unwrap().expect("queued ref");
    let queue = f.queue_base();
    b51::tso_group(&mut f.state, &mut f.dma, &mut f.ring, queue, first, &mut f.submitted_at, now);
}

#[test]
fn flag_tso_group_drains_to_giant_send_descriptors() {
    let mut f = Fixture::new();
    let refs = group_refs(&f);
    push_all(&f, &refs);
    drain_first(&mut f, 42);
    // One request accepted by hardware; ceil(3020/1460) = 3 segments.
    assert_eq!((f.state.requests, f.state.hw_frames, f.state.hw_segments_est), (1, 1, 3));
    assert_eq!((f.state.invalid, f.state.desc_errors, f.state.queue_drops), (0, 0, 0));
    assert_eq!(f.dma.tx_inflight(), 3);
    for (i, r) in refs.iter().enumerate() {
        let words = f.dma.tx_descriptor_words(i).unwrap();
        let len = [154u32, 1460, 1460][i];
        let mut flags = OWN | ring::TSO_GIANT_SEND_V4 | (34 << ring::TSO_GTTCPHO_SHIFT) | len;
        if i == 0 {
            flags |= FIRST;
        }
        if i == 2 {
            flags |= LAST;
        }
        assert_eq!(words[0], flags, "descriptor {} opts1", i);
        assert_eq!(words[1], 1460 << ring::TSO_MSS_SHIFT, "descriptor {} opts2", i);
        assert_eq!(u64::from(words[2]) | u64::from(words[3]) << 32, f.bus_of(r.id()));
        assert_eq!(f.state_of(r.id()), zc::TX_DMA, "frag {} is DMA-owned", i);
        assert_eq!(f.submitted_at[i], 42);
    }
    assert_eq!(zc::RefQueue::depth(f.queue_base()), Ok(0));
    let pending = f.state.pending[0].expect("outstanding request");
    assert_eq!(pending.nfrags, 3);
    assert_eq!(pending.refs.iter().flatten().count(), 3);
}

#[test]
fn malformed_groups_reject_refs_and_count_invalid() {
    let meta = tso::pack_first_meta(1460, false, 20);
    // A non-TSO reference mid-group terminates the group as malformed.
    let mut f = Fixture::new();
    let first = f.make_ref(tso::FLAG_TSO_FIRST, meta, 154);
    let stray = f.make_ref(0, 0, 100);
    push_all(&f, &[first, stray]);
    drain_first(&mut f, 1);
    assert_eq!(f.state.invalid, 1);
    assert_eq!(f.state_of(first.id()), zc::FREE);
    assert_eq!(f.state_of(stray.id()), zc::FREE);
    assert_eq!((f.state.requests, f.dma.tx_inflight()), (0, 0));

    // Missing LAST: the queue runs dry mid-group.
    let mut f = Fixture::new();
    let first = f.make_ref(tso::FLAG_TSO_FIRST, meta, 154);
    let cont = f.make_ref(tso::FLAG_TSO_CONT, 0, 1460);
    push_all(&f, &[first, cont]);
    drain_first(&mut f, 1);
    assert_eq!(f.state.invalid, 1);
    assert_eq!(f.state_of(first.id()), zc::FREE);
    assert_eq!(f.state_of(cont.id()), zc::FREE);

    // Bad first-ref metadata is rejected with the group.
    let mut f = Fixture::new();
    let bad = f.make_ref(tso::FLAG_TSO_FIRST, tso::pack_first_meta(63, false, 20), 154);
    push_all(&f, &[bad]);
    drain_first(&mut f, 1);
    assert_eq!((f.state.invalid, f.state_of(bad.id())), (1, zc::FREE));

    // More than TSO_MAX_FRAGS fragments: everything collected is freed and
    // the overflow fragment stays queued for the next drain.
    let mut f = Fixture::new();
    let first = f.make_ref(tso::FLAG_TSO_FIRST, meta, 154);
    let conts: Vec<_> = (0..8).map(|_| f.make_ref(tso::FLAG_TSO_CONT, 0, 1460)).collect();
    push_all(&f, std::slice::from_ref(&first));
    push_all(&f, &conts);
    drain_first(&mut f, 1);
    assert_eq!(f.state.invalid, 1);
    assert_eq!(f.state_of(first.id()), zc::FREE);
    for r in &conts[..7] {
        assert_eq!(f.state_of(r.id()), zc::FREE);
    }
    assert_eq!(zc::RefQueue::depth(f.queue_base()), Ok(1));
    assert_eq!(f.state_of(conts[7].id()), zc::CPU, "overflow fragment still queued");
    for _ in 0..2 {
        let leftover = zc::RefQueue::pop(f.queue_base()).unwrap();
        if let Some(r) = leftover {
            assert_eq!(zc::reject(f.meta_base(), &r), Ok(()));
        }
    }
}

#[test]
fn fallback_segments_through_ring_send_and_frees_refs() {
    let mut f = Fixture::new();
    let mut refs = [zc::Ref::new(0, 0); tso::TSO_MAX_FRAGS];
    for slot in &mut refs {
        *slot = f.make_ref(0, 0, 0);
    }
    // 64-byte aligned but not 2048-byte aligned bus addresses: the shared
    // validator accepts the request, the hardware path refuses it, so the
    // glue must take the software fallback.
    let mut addrs = [0u64; tso::TSO_MAX_FRAGS];
    let mut slices: [&mut [u8]; tso::TSO_MAX_FRAGS] =
        core::array::from_fn(|i| f.cpu_slice(refs[i].id()));
    for (i, r) in refs.iter().enumerate() {
        addrs[i] = f.bus_of(r.id()) + 0x40;
    }
    let spec = tso_testgen::TestSpec::default_ipv4(7);
    let request = tso_testgen::build(&spec, &mut slices, &addrs).unwrap();
    f.state.submit(&mut f.dma, &mut f.ring, request, &refs, &mut f.submitted_at, 99);
    assert_eq!(f.state.hw_frames, 0, "hardware path must refuse");
    assert_eq!((f.state.requests, f.state.fallback_frames, f.state.fallback_segments), (1, 1, 11));
    assert_eq!((f.state.desc_errors, f.state.queue_drops, f.state.invalid), (0, 0, 0));
    // ceil(16000/1460) = 11 frames went through `ring.send`, MTU-sized.
    assert_eq!(f.dma.tx_inflight(), 11);
    let mut payload_total = 0usize;
    for slot in 0..11 {
        let words = f.dma.tx_descriptor_words(slot).unwrap();
        assert_eq!(words[0] & OWN, OWN, "frame {} published", slot);
        assert_eq!(words[0] & (FIRST | LAST), FIRST | LAST, "single-descriptor frame");
        let padded = (words[0] & 0xffff) as usize;
        assert!((60..=1514).contains(&padded), "frame {} padded {}", slot, padded);
        let bus = u64::from(words[2]) | u64::from(words[3]) << 32;
        let frame = unsafe { core::slice::from_raw_parts(f.dma.registers().ptr(bus, padded), padded) };
        let ip_len = usize::from(u16::from_be_bytes([frame[16], frame[17]]));
        payload_total += ip_len - 40;
        assert!(14 + ip_len <= 1514, "frame {} respects the MTU", slot);
    }
    assert_eq!(payload_total, 16000);
    for r in &refs {
        assert_eq!(f.state_of(r.id()), zc::FREE, "source refs returned to the pool");
    }
}

#[test]
fn completion_releases_refs_only_when_retired_tso_reports() {
    let mut f = Fixture::new();
    let meta = tso::pack_first_meta(1460, false, 20);
    let refs = [
        f.make_ref(tso::FLAG_TSO_FIRST, meta, 154),
        f.make_ref(tso::FLAG_TSO_CONT | tso::FLAG_TSO_LAST, 0, 1460),
    ];
    push_all(&f, &refs);
    drain_first(&mut f, 5);
    assert_eq!(f.state.hw_frames, 1);
    let id = f.state.pending[0].expect("request").tso_id;
    for r in &refs {
        assert_eq!(f.state_of(r.id()), zc::TX_DMA);
    }
    // The first descriptor retires without the LAST one: buffers stay held.
    f.dma.registers().regs.insert(BAR + dma::TX_CLOSE, 1);
    let oldest = f.dma.oldest_tx_slot();
    let completed = f.dma.reclaim_tx().unwrap();
    assert_eq!((oldest, completed), (0, 1));
    f.state.reclaim(&mut f.ring, oldest, completed);
    for r in &refs {
        assert_eq!(f.state_of(r.id()), zc::TX_DMA, "held before the report");
    }
    assert!(f.state.pending[0].is_some());
    assert_eq!(f.state.completion_failures, 0);
    // The LAST slot retires: `retired_tso` reports the id and all buffers
    // of the request are released at once.
    f.dma.registers().regs.insert(BAR + dma::TX_CLOSE, 2);
    let oldest = f.dma.oldest_tx_slot();
    let completed = f.dma.reclaim_tx().unwrap();
    assert_eq!((oldest, completed), (1, 1));
    f.state.reclaim(&mut f.ring, oldest, completed);
    for r in &refs {
        assert_eq!(f.state_of(r.id()), zc::FREE, "released on report");
    }
    assert!(f.state.pending[0].is_none());
    assert_eq!(f.state.completion_failures, 0);
    // A report for an untracked request is an anomaly and frees nothing.
    let refs = [
        f.make_ref(tso::FLAG_TSO_FIRST, meta, 154),
        f.make_ref(tso::FLAG_TSO_CONT | tso::FLAG_TSO_LAST, 0, 1460),
    ];
    push_all(&f, &refs);
    drain_first(&mut f, 6);
    let stolen = f.state.pending.iter_mut().find(|p| p.is_some()).unwrap().take().unwrap();
    f.dma.registers().regs.insert(BAR + dma::TX_CLOSE, 4);
    let oldest = f.dma.oldest_tx_slot();
    let completed = f.dma.reclaim_tx().unwrap();
    assert_eq!((oldest, completed), (2, 2));
    f.state.reclaim(&mut f.ring, oldest, completed);
    assert_eq!(f.state.completion_failures, 1);
    for r in stolen.refs.iter().flatten() {
        assert_eq!(f.state_of(r.id()), zc::TX_DMA, "untracked refs stay held");
        assert_eq!(zc::complete(f.meta_base(), r), Ok(()));
    }
    assert_ne!(id, 0);
}

#[test]
fn test_generator_submits_three_requestsWith_ids_one_to_three() {
    let mut f = Fixture::new();
    b51::tso_testgen_run(
        &mut f.state,
        &mut f.dma,
        &mut f.ring,
        zc::PARTITION_LAN1,
        &mut f.submitted_at,
        7,
    );
    assert_eq!((f.state.requests, f.state.hw_frames), (3, 3));
    assert_eq!(f.state.hw_segments_est, 33, "3 x ceil(16000/1460)");
    let mut ids: Vec<u32> = f.state.pending.iter().flatten().map(|p| p.tso_id).collect();
    ids.sort_unstable();
    assert_eq!(ids, [1, 2, 3], "the generator is the first id consumer");
    assert_eq!(f.dma.tx_inflight(), 24, "8 descriptors per request");
    for entry in f.state.pending.iter().flatten() {
        for r in entry.refs.iter().flatten() {
            assert_eq!(f.state_of(r.id()), zc::TX_DMA);
        }
    }
    let words = f.dma.tx_descriptor_words(0).unwrap();
    assert_eq!(words[0] & ring::TSO_GIANT_SEND_V4, ring::TSO_GIANT_SEND_V4);
    assert_eq!(words[1], 1460 << ring::TSO_MSS_SHIFT);
}
