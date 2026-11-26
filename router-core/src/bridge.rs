pub const MAC: [u8; 6] = [2, 0x52, 0x35, 0x53, 0, 2];
const LOCAL_HASH: bool = match option_env!("R5S_BRIDGE_LOCAL_HASH") {
    Some(value) => matches!(value.as_bytes(), b"1"),
    None => false,
};

#[derive(Clone, Copy)]
struct Entry { mac: [u8; 6], port: usize, until: u64 }

pub struct Bridge {
    learned: [Option<Entry>; 64],
    last_source: [usize; 2],
    recent: [(u64, u64); 32],
    next: usize,
}

impl Bridge {
    pub const fn new() -> Self {
        Self { learned: [None; 64], last_source: [0; 2], recent: [(0, 0); 32], next: 0 }
    }

    pub fn destination(&self, mac: &[u8], links: u8, now: u64) -> u8 {
        self.learned_destination(mac, links, now).map_or(links, |(port, _)| 1 << port)
    }

    pub fn learned_destination(&self, mac: &[u8], links: u8, now: u64) -> Option<(usize, u64)> {
        if mac.first()? & 1 != 0 { return None; }
        self.learned.iter().flatten()
            .find(|entry| entry.mac == mac && entry.until > now && links & (1 << entry.port) != 0)
            .map(|entry| (entry.port, entry.until))
    }

    pub fn receive(&mut self, port: usize, frame: &[u8], links: u8, now: u64) -> Option<(bool, u8)> {
        if port > 1 || links & (1 << port) == 0 || !(14..=1514).contains(&frame.len()) { return None; }
        let dst = &frame[..6];
        let src: [u8; 6] = frame[6..12].try_into().unwrap();
        if src == MAC || src == [0; 6] || src[0] & 1 != 0 || dst[..5] == [1, 0x80, 0xc2, 0, 0] {
            return None;
        }
        let egress = if dst == MAC || (dst[0] & 1 != 0 && dst != [255; 6]) { 0 }
                     else { self.destination(dst, links, now) & !(1 << port) };
        // Router-addressed frames are never entered into the reflection history.
        if links == 3 && (LOCAL_HASH || dst != MAC) {
            let hash = frame.iter().fold(0xcbf29ce484222325u64,
                |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3));
            if self.recent.iter().any(|&(previous, until)| previous == hash && until > now) {
                return None;
            }
            if egress != 0 {
                self.recent[self.next] = (hash, now.saturating_add(50));
                self.next = (self.next + 1) % self.recent.len();
            }
        }
        self.learn_source(port, src, links, now);
        Some((dst == MAC || dst == [255; 6], egress))
    }

    pub fn learn_source(&mut self, port: usize, src: [u8; 6], links: u8, now: u64) {
        if port > 1 || links & (1 << port) == 0 || src == MAC || src == [0; 6] || src[0] & 1 != 0 {
            return;
        }
        let last = self.last_source[port];
        let slot = if !LOCAL_HASH && self.learned[last].is_some_and(|entry| entry.mac == src) {
            last
        } else {
            self.learned.iter().position(|e| e.is_some_and(|e| e.mac == src))
                .or_else(|| self.learned.iter().position(|e| e.is_none_or(|e| e.until <= now)))
                .unwrap_or_else(|| self.learned.iter().enumerate().min_by_key(|(_, e)| e.unwrap().until).unwrap().0)
        };
        self.learned[slot] = Some(Entry { mac: src, port, until: now.saturating_add(300_000) });
        self.last_source[port] = slot;
    }

    pub fn link_down(&mut self, port: usize) {
        for entry in &mut self.learned {
            if entry.is_some_and(|value| value.port == port) { *entry = None; }
        }
    }
}

#[cfg(test)]
#[test]
fn learns_moves_ages_and_filters_reflections() {
    let mut bridge = Bridge::new();
    let mut frame = [0; 60];
    frame[..6].fill(255);
    frame[6..12].copy_from_slice(&[2, 3, 4, 5, 6, 7]);
    assert_eq!(bridge.receive(0, &frame, 3, 10), Some((true, 2)));
    assert_eq!(bridge.receive(1, &frame, 3, 11), None);
    assert_eq!(bridge.destination(&frame[6..12], 3, 11), 1);
    frame[..6].copy_from_slice(&MAC);
    assert_eq!(bridge.receive(1, &frame, 3, 100), Some((true, 0)));
    assert_eq!(bridge.destination(&frame[6..12], 3, 100), 2);
    assert_eq!(bridge.destination(&frame[6..12], 3, 300_101), 3);
    bridge.link_down(1);
    assert_eq!(bridge.destination(&frame[6..12], 3, 101), 3);
}

#[cfg(test)]
#[test]
fn local_unicast_relearns_after_cache_replacement_and_mac_move() {
    let mut bridge = Bridge::new();
    let mut frame = [0; 1514];
    frame[..6].copy_from_slice(&MAC);
    let source = [2, 1, 2, 3, 4, 5];
    frame[6..12].copy_from_slice(&source);
    for now in 0..2 {
        assert_eq!(bridge.receive(0, &frame, 3, now), Some((true, 0)));
    }
    assert_eq!(bridge.destination(&source, 3, 2), 1);
    for n in 1..=64 {
        frame[11] = n + 5;
        assert_eq!(bridge.receive(1, &frame, 3, u64::from(n) + 2), Some((true, 0)));
    }
    assert_eq!(bridge.destination(&source, 3, 67), 3);
    frame[6..12].copy_from_slice(&source);
    assert_eq!(bridge.receive(1, &frame, 3, 68), Some((true, 0)));
    assert_eq!(bridge.destination(&source, 3, 68), 2);
}
