use core::sync::atomic::{AtomicU32, Ordering};
use crate::b51_udp::WORDS;

pub fn read(port_mask: u32, logical: bool) -> Option<[u32; WORDS]> {
    let mut words = [0; WORDS];
    let read = |base: usize, index: usize| {
        unsafe { &*((base as *const AtomicU32).add(index)) }.load(Ordering::Acquire)
    };
    for port in 0..2 {
        if port_mask & (1 << port) == 0 { continue; }
        let base = 0x41000000 + port * 0x100000;
        let start = 32 + 32 * port;
        if logical && cfg!(feature = "shared-wan") {
            for i in 0..6 { words[8 + port * 6 + i] = read(base, 256 + i); }
        }
        if logical {
            words[start + 9] = read(base,262);
            words[start + 10] = read(base,263);
            #[cfg(feature = "lan-tx-stats")]
            {
                words[346 + port] = read(base,284);
                for i in 0..3 { words[348 + port * 3 + i] = read(base,285 + i); }
            }
            let ring_entries = match read(base, 276) {
                256 => 1,
                512 => 2,
                value => value & 255,
            };
            words[6] |= ring_entries << (port * 8);
            words[27 + port * 2] = read(base,277);
            words[28 + port * 2] = read(base,278);
            #[cfg(feature = "rss-stats")]
            for i in 0..5 { words[336 + port * 5 + i] = read(base, 279 + i); }
        }
        for i in 0..9 { words[start + i] = read(base, i); }
        if logical { for i in 0..12 { words[start + 12 + i] = read(base, 32 + i); } }
        for (i, index) in [27, 135, 143, 15, 16, 21, 22].into_iter().enumerate() {
            words[start + 24 + i] = read(base, index);
        }
    }
    if logical {
        for i in 0..21 { words[96 + i] = read(0x41200000, i); }
        if cfg!(feature = "forward-workers") {
            for (i, target) in (20..25).chain(117..128).enumerate() {
                words[target] = read(0x41200000,65+i);
            }
            words[247] = read(0x41200000,64);
            words[31] = read(0x41200000,81);
        }
        words[246] = read(0x41200000, 50);
        if let Some(trace) = crate::shared::read::<29>(0x41200000 + 21 * 4) {
            words[213..242].copy_from_slice(&trace);
        }
    }
    if let Some(app) = crate::shared::read::<32>(0x40004000) {
        words[128..160].copy_from_slice(&app);
    }
    if let Some(tx) = crate::shared::read::<4>(0x40004080) {
        words[242..246].copy_from_slice(&tx);
    }
    if let Some(link) = crate::shared::read::<20>(0x40002000) {
        words[160..180].copy_from_slice(&link);
    }
    if read(0x40002000, 31) == 1 {
        words[208] = 1;
        for i in 0..4 { words[209 + i] = read(0x40002000, 32 + i); }
    }
    let supervisor = 0x40001000;
    words[180] = 1;
    words[181] = read(supervisor, 5);
    words[182] = read(supervisor, 31);
    if words[182] != 0 {
        words[183] = read(supervisor, 7);
        words[184] = read(supervisor, 8);
        words[185] = read(supervisor, 9);
        words[186] = read(supervisor, 32);
        for i in 0..21 { words[187 + i] = read(supervisor, 10 + i); }
    }
    for (out, index) in words[..6].iter_mut().zip(176..182) {
        *out = read(0x40002000, index);
    }
    let axi = read(0x40002000, 187);
    if axi != 0 {
        words[6] |= (1 << 31) | ((axi >> 16 & 15) << 16) | ((axi >> 24 & 15) << 20)
            | ((axi >> 14 & 1) << 24);
        // EAME (DMA_SysBus_Mode bit 11) rides in the pbl word's high bit;
        // dma_pbl values fit in 16 bits.
        if axi & (1 << 11) != 0 {
            words[5] |= 1 << 31;
        }
    }
    if option_env!("R5S_GMAC_HYBRID_IRQ")==Some("1") {
        words[6] |= (1 << 27) | (u32::from(read(0x40002000,188)!=0) << 25)
            | (u32::from(read(0x40002000,189)!=0) << 26);
    }
    if option_env!("R5S_GMAC_TX_CHANNELS")==Some("2") {
        words[6] |= (1 << 28) | (u32::from(read(0x40002000,190)==2) << 29);
    }
    if option_env!("R5S_GMAC_RX_CHANNELS")==Some("2") && read(0x40002000,191)==2 {
        words[6] |= 1 << 30;
    }
    words[43] = read(0x40002000,36);
    words[75] = read(0x40002000,37);
    #[cfg(feature = "zc")]
    {
        // Schema 6: shared zero-copy pool counters (zc.rs C_* indices).
        let meta = 0x66800000usize;
        let counter = |index: usize| crate::zc::counter(meta, index).load(Ordering::Relaxed);
        for (out, index) in words[248..266].iter_mut().zip(0..18) {
            *out = counter(index);
        }
        let (used, free, _) = crate::zc::occupancy(meta);
        words[266] = used;
        words[267] = free;
        words[63] = counter(18);
        words[95] = counter(19);
        words[131] = counter(20);
        words[132] = (counter(24).min(0xffff) << 16) | counter(21).min(0xffff);
        words[134] = (counter(22).min(0xffff) << 16) | counter(23).min(0xffff);
        #[cfg(feature = "sched-stats")]
        {
            if option_env!("R5S_WORKER_GRO") == Some("1") {
                words[333] = counter(crate::zc::C_GRO_WORKER_PAIRS);
                words[334] |= 1;
            }
            if option_env!("R5S_DRIVER_GRO") == Some("1") {
                words[335] = counter(crate::zc::C_GRO_DRIVER_PAIRS);
                words[334] |= 2;
            }
        }
    }
    #[cfg(feature = "tso")]
    {
        // Schema 7: per-port TSO diagnostics (12 words per port).
        // WAN from the GMAC status page, LAN1/LAN2 from the device pages.
        for (out, index) in words[268..280].iter_mut().zip(152..164) {
            *out = read(0x40002000, index);
        }
        for (out, index) in words[280..292].iter_mut().zip(264..276) {
            *out = read(0x41000000, index);
        }
        for (out, index) in words[292..304].iter_mut().zip(264..276) {
            *out = read(0x41100000, index);
        }
    }
    #[cfg(feature = "sched-stats")]
    {
        for (out, index) in words[304..328].iter_mut().zip(84..108) {
            *out = read(0x41200000, index);
        }
        for (out, index) in words[328..333].iter_mut().zip(182..187) {
            *out = read(0x40002000, index);
        }
        words[334] |= (read(0x40002000, 196) & 31) << 8;
    }
    #[cfg(feature = "lan-tx-stats")]
    for (i, word) in words[354..358].iter_mut().enumerate() {
        *word = read(0x40002000, 192 + i);
    }
    words[25] = if logical && cfg!(feature = "lan-tx-stats") { 11 }
        else if logical && cfg!(feature = "rss-stats") { 9 }
        else if logical && cfg!(feature = "sched-stats") { 8 }
        else if logical && cfg!(feature = "tso") { 7 }
        else if logical && cfg!(feature = "zc") { 6 }
        else if logical && cfg!(feature = "forward-workers") { 5 }
        else if logical && cfg!(feature = "shared-wan") { 4 }
        else { 3 };
    words[26] = port_mask;
    words[7] = if logical { 6 } else { words[71] };
    Some(words)
}
