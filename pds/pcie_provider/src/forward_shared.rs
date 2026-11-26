use core::sync::atomic::{AtomicU32, Ordering::{Acquire, Relaxed, Release, AcqRel}};
use r5s_router_core::Wan;

pub const WORKERS: usize = r5s_router_core::steering::WORKERS;
pub const CONTROL: usize = 0x65000000;
const QUEUES: usize = 0x65100000;
const CLIENTS: usize = 256;
pub const INPUT_WAN: u32 = 1;
pub const TTL: u32 = 2;
#[cfg(feature = "zc")]
pub const LAN_RETRY: usize = 10;
#[cfg(feature = "zc")]
pub const WORKER_DIAG: usize = 11;
#[cfg(feature = "zc")]
pub const WORKER_DIAG_WORDS: usize = 10;
pub const LAN_TAG_SHIFT: u32 = 2;
pub const LAN_TAG_MASK: u32 = 3 << LAN_TAG_SHIFT;
pub const TAG_SHIFT: u32 = crate::queue::FORWARD_TAG_SHIFT;

pub const fn residency(worker: usize) -> usize { base(worker) + 0x3c00 }
pub const fn base(worker: usize) -> usize { QUEUES + worker * 0x8000 }
/// Reference-queue pair of one worker: zero-copy requests and results live
/// beside the frame queues kept for the copy fallback.
#[cfg(feature = "zc")]
pub const fn request_ref(worker: usize) -> usize { base(worker) + 0x3100 }
#[cfg(feature = "zc")]
pub const fn result_ref(worker: usize) -> usize { base(worker) + 0x7100 }
pub fn word(base: usize, index: usize) -> &'static AtomicU32 {
    unsafe { &*((base as *const AtomicU32).add(index)) }
}
pub fn counter(worker: usize, index: usize) -> &'static AtomicU32 {
    word(base(worker) + 0x3f00, index)
}

pub struct Control {
    pub revision: u32,
    pub wan_generation: u32,
    pub wan: Option<Wan>,
    link_epoch: Option<u32>,
    pub clients: [u32; CLIENTS],
}
impl Control {
    pub const fn new() -> Self {
        Self { revision: 1, wan_generation: 0, wan: None, link_epoch: None, clients: [0; CLIENTS] }
    }
    pub fn publish(&mut self, generation: u32, wan: Option<Wan>, link_epoch: Option<u32>, released: [u32;8]) {
        if self.wan_generation == generation && self.wan == wan && self.link_epoch == link_epoch && released == [0;8] { return; }
        let revision = self.wan_generation != generation || self.link_epoch != link_epoch || released != [0;8];
        if revision {
            self.revision = (self.revision + 1) & (u32::MAX >> TAG_SHIFT);
        }
        self.wan_generation = generation;
        self.wan = wan;
        self.link_epoch = link_epoch;
        word(CONTROL,0).fetch_add(1, AcqRel);
        for (i, bits) in released.into_iter().enumerate() {
            for bit in 0..32 {
                if bits & (1 << bit) == 0 { continue; }
                let host = i*32+bit;
                self.clients[host] = self.clients[host].wrapping_add(1);
                word(CONTROL,16+host).store(self.clients[host], Relaxed);
            }
        }
        let mut values = [self.revision,generation,0,0,0,0,0,0,0,0,0];
        if let Some(wan) = wan {
            values[2..].copy_from_slice(&[1, wan.address, wan.mask, wan.gateway,
                wan.dns[0], wan.dns[1], wan.dns[2], wan.dns_count as u32, wan.expires as u32]);
        }
        word(CONTROL,13).store(link_epoch.unwrap_or(0), Relaxed);
        word(CONTROL,12).store(wan.map_or(0, |w| (w.expires >> 32) as u32), Relaxed);
        for (i, value) in values.into_iter().enumerate() { word(CONTROL,i+1).store(value,Relaxed); }
        word(CONTROL,0).fetch_add(1,Release);
    }
    pub fn snapshot() -> Option<Self> {
        let w = crate::shared::read::<272>(CONTROL)?;
        let wan = (w[3] != 0).then(|| Wan {
            address:w[4], mask:w[5], gateway:w[6], dns:[w[7],w[8],w[9]],
            dns_count:w[10] as usize, expires:u64::from(w[11]) | (u64::from(w[12]) << 32),
        });
        Some(Self { revision:w[1], wan_generation:w[2], wan, link_epoch:wan.map(|_|w[13]), clients:w[16..].try_into().unwrap() })
    }
}

pub fn changed(sequence: u32) -> Option<u32> {
    let value = word(CONTROL,0).load(Acquire);
    (value != sequence && value & 1 == 0).then_some(value)
}
