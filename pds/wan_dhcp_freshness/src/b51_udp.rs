use r5s_router_core::{checksum, ipat, set16, setip, transport_checksum, u16at, unicast, validate_ip};

const PORT: u16 = 48125;
/// Status schema words. Schema 7 (TSO builds) appends per-port TSO
/// diagnostics after the schema 6 layout; schema 6 (zero-copy builds)
/// appends the shared-pool diagnostics after the schema 5 layout; earlier
/// builds keep 248 words.
#[allow(unexpected_cfgs)]
#[cfg(feature = "lan-tx-stats")]
pub const WORDS: usize = 368;
#[cfg(all(feature = "rss-stats", not(feature = "lan-tx-stats")))]
pub const WORDS: usize = 346;
#[cfg(all(feature = "sched-stats", not(any(feature = "rss-stats", feature = "lan-tx-stats"))))]
pub const WORDS: usize = 336;
#[cfg(all(feature = "tso", not(any(feature = "sched-stats", feature = "rss-stats", feature = "lan-tx-stats"))))]
pub const WORDS: usize = 304;
#[cfg(all(feature = "zc", not(any(feature = "tso", feature = "sched-stats", feature = "rss-stats", feature = "lan-tx-stats"))))]
pub const WORDS: usize = 268;
#[cfg(not(any(feature = "zc", feature = "sched-stats", feature = "rss-stats", feature = "lan-tx-stats")))]
pub const WORDS: usize = 248;
pub fn frame_length(frame: &[u8]) -> usize {
    14 + usize::from(u16at(frame, 16))
}

pub fn reply(frame: &mut [u8], mac: [u8; 6], active: u32, mask: u32, snapshot: impl FnOnce() -> Option<[u32; WORDS]>) -> bool {
    if frame.len() < 58 { return false; }
    let length = frame_length(frame);
    let udp_length = length.saturating_sub(34);
    let Some(body) = udp_length.checked_sub(24) else { return false };
    let words_on_wire = body / 4;
    if body % 4 != 0 || (words_on_wire != WORDS
        && !(cfg!(feature = "lan-tx-stats") && matches!(words_on_wire, 248 | 268 | 304 | 336 | 346 | 348 | 352 | 358 | 360 | 364))
        && !(cfg!(feature = "rss-stats") && matches!(words_on_wire, 248 | 268 | 304))
        && !(cfg!(feature = "sched-stats") && matches!(words_on_wire, 248 | 268 | 336)))
    {
        return false;
    }
    let Some(frame) = frame.get_mut(..length) else { return false };
    if active == 0 || mask == 0
        || frame[..6] != mac || frame[6] & 1 != 0 || frame[6..12] == mac
        || frame[6..12] == [0; 6] || frame[12..14] != [8, 0]
    {
        return false;
    }
    let (eth, ip) = frame.split_at_mut(14);
    let source = ipat(ip, 12);
    let port = u16at(ip, 20);
    if ip[9] != 17 || u16at(ip, 22) != PORT || ipat(ip, 16) != active || port == 0
        || source == active || !unicast(source) || source & mask != active & mask
        || source & !mask == 0 || source & !mask == !mask || ip[8] == 0
        || u16at(ip, 24) != udp_length as u16 || validate_ip(ip) != Ok(ip.len())
        || (u16at(ip, 26) != 0 && transport_checksum(ip, ip.len()) != 0)
        || &ip[28..36] != b"B51Q\x00\x01\x00\x00" || ip[44..].iter().any(|&v| v != 0)
    {
        return false;
    }

    let Some(mut words) = snapshot() else { return false };
    if words_on_wire != WORDS {
        words[25] = match words_on_wire { 268 => 6, 304 => 7, 336 => 8, 346 => 9, 348 => 10, 358 => 11, _ => 5 };
    }
    #[cfg(all(feature = "zc", feature = "lan-tx-stats"))]
    if words_on_wire==352 {
        for (i,out) in words[304..344].iter_mut().enumerate() {
            let counter=unsafe {&*((0x41200000 as *const core::sync::atomic::AtomicU32).add(160+i))};
            *out=counter.load(core::sync::atomic::Ordering::Relaxed);
        }
        words[25]=14;
        words[344] = 0x5a435231;
        for (i, out) in words[345..348].iter_mut().enumerate() {
            *out = crate::zc::counter(crate::zc::META_VA, crate::zc::C_CPU_RELEASE + i)
                .load(core::sync::atomic::Ordering::Relaxed);
        }
    }
    #[cfg(all(feature = "zc", feature = "lan-tx-stats"))]
    if words_on_wire >= 360 {
        use crate::zc;
        words[25] = if words_on_wire >= 368 {15} else if words_on_wire == 364 {13} else {12};
        if words_on_wire >= 364 {
            for (index, word) in words[360..363].iter_mut().enumerate() {
                *word = zc::counter(zc::META_VA, zc::C_PORT_BORROW + index)
                    .load(core::sync::atomic::Ordering::Relaxed);
            }
            words[363] = zc::counter(zc::META_VA, zc::C_GRO_SEGMENTS)
                .load(core::sync::atomic::Ordering::Relaxed);
            let read = |index| zc::counter(zc::META_VA, index).load(core::sync::atomic::Ordering::Relaxed);
            words[325] = read(zc::C_GMAC_ROUTE_BLOCKED);
            words[326] = read(zc::C_GMAC_STALE);
            words[327] = read(zc::C_GMAC_GROUP_DEFERRED);
            words[358] = (1 << 31) | read(zc::C_GMAC_TX_PEAK);
            words[359] = read(zc::C_GMAC_TX_PEAK + 1);
        }
        if words_on_wire >= 368 {
            /* Schema 15: payload cache-maintenance calls (the NC payload
             * policy never issues them) and the requested GMAC PBL kept
             * beside the effective value in word 5. */
            words[364] = zc::counter(zc::META_VA, zc::C_CACHE_MAINTENANCE)
                .load(core::sync::atomic::Ordering::Relaxed);
            words[365] = unsafe {
                &*((0x40002000 as *const core::sync::atomic::AtomicU32).add(200))
            }.load(core::sync::atomic::Ordering::Relaxed);
        }
        if zc::WORKER_TX_EVENT_DRIVEN {
            words[313] = 0x57545831;
            for (index, word) in words[314..320].iter_mut().enumerate() {
                *word = zc::counter(zc::META_VA, zc::WORKER_TX_WAKEUPS + index)
                    .load(core::sync::atomic::Ordering::Relaxed);
            }
        }
        words[322] = 0x4c524e31;
        for (index, word) in words[323..325].iter_mut().enumerate() {
            *word = zc::counter(zc::META_VA, zc::C_LEARN_SUBMITTED + index)
                .load(core::sync::atomic::Ordering::Relaxed);
        }
        if option_env!("R5S_GMAC_COPIED_CHECKSUM") == Some("1") {
            words[320] = 0x43534d31;
            words[321] = zc::counter(zc::META_VA, zc::C_NAT_COPY_CHECKSUM_BYPASS)
                .load(core::sync::atomic::Ordering::Relaxed);
        }
        for (index, word) in words[304..313].iter_mut().enumerate() {
            *word = zc::counter(zc::META_VA, zc::C_PORT_ALLOC_FAILED + index)
                .load(core::sync::atomic::Ordering::Relaxed);
        }
    }
    let state = match words[7] { 0 => 0, 0xff => 2, _ => 1 };
    ip[28..32].copy_from_slice(b"B51R");
    set16(ip, 34, state);
    for (out, word) in ip[44..].chunks_exact_mut(4).zip(words) {
        out.copy_from_slice(&word.to_be_bytes());
    }

    eth.copy_within(6..12, 0);
    eth[6..12].copy_from_slice(&mac);
    ip[1] = 0;
    set16(ip, 4, 0);
    set16(ip, 6, 0x4000);
    ip[8] = 1;
    setip(ip, 12, active);
    setip(ip, 16, source);
    set16(ip, 10, 0);
    let header_sum = checksum(&ip[..20]);
    set16(ip, 10, header_sum);
    set16(ip, 20, PORT);
    set16(ip, 22, port);
    set16(ip, 26, 0);
    let sum = transport_checksum(ip, ip.len());
    set16(ip, 26, if sum == 0 { 0xffff } else { sum });
    true
}
