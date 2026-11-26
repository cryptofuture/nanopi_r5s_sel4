use crate::{ipat, u16at};

pub const WORKERS: usize = 4;
const BUCKETS: usize = 4096;
pub const BITMAP_WORDS: usize = BUCKETS / 32;

fn endpoint_hash(ip: u32, port: u16, protocol: u8) -> u32 {
    let key = ip.rotate_left(7) ^ u32::from(port) ^ (u32::from(protocol) << 16);
    let mixed = (key ^ (key >> 16)).wrapping_mul(0x7feb352d);
    let mixed = (mixed ^ (mixed >> 15)).wrapping_mul(0x846ca68b);
    mixed ^ (mixed >> 16)
}
pub(crate) fn bucket(ip: u32, port: u16, protocol: u8) -> usize {
    (endpoint_hash(ip,port,protocol)>>20) as usize
}

pub fn outbound_transport(ip: &[u8]) -> Option<usize> {
    if ip.len() < 28 || ip[0] != 0x45 || !matches!(ip[9], 6 | 17) { return None; }
    Some(((endpoint_hash(ipat(ip, 12), u16at(ip, 20), ip[9]) >> 20) as usize) % WORKERS)
}

pub fn transport_endpoint(ip: &[u8]) -> Option<(u64, u32)> {
    if ip.len() < 28 || ip[0] != 0x45 || !matches!(ip[9], 6 | 17) { return None; }
    let address = ipat(ip, 12);
    let port = u16at(ip, 20);
    let protocol = ip[9];
    let key = (u64::from(address) << 24) | (u64::from(port) << 8) | u64::from(protocol);
    Some((key, endpoint_hash(address, port, protocol)))
}

pub fn direct_lan_endpoint(frame: &[u8]) -> Option<(u64, u32)> {
    if !(42..=1514).contains(&frame.len()) || frame[..6] != crate::bridge::MAC
        || frame[6..12] == crate::bridge::MAC || frame[6..12] == [0; 6] || frame[6] & 1 != 0
        || frame[12..14] != [8, 0] || ipat(frame, 30) == crate::LAN[0]
        || (frame[23] == 17 && u16at(frame, 34) == 68 && u16at(frame, 36) == 67)
    {
        return None;
    }
    transport_endpoint(&frame[14..])
}

pub fn direct_lan_owner(frame: &[u8]) -> Option<usize> {
    direct_lan_endpoint(frame).map(|(_, hash)| ((hash >> 20) as usize) % WORKERS)
}

pub struct Steering([u8; BUCKETS]);
impl Steering {
    pub const fn new() -> Self { Self([0; BUCKETS]) }

    pub fn outbound(&mut self, ip: &[u8], load: impl Fn(usize) -> u32) -> Option<usize> {
        if ip.len() < 28 || ip[0] != 0x45 { return None; }
        let port = match ip[9] { 1 => u16at(ip,24), 6 | 17 => u16at(ip,20), _ => return None };
        let hash = endpoint_hash(ipat(ip,12),port,ip[9]);
        let bucket = (hash >> 20) as usize;
        let owner = &mut self.0[bucket];
        if *owner == 0 {
            let start = (hash as usize >> 16) % WORKERS;
            let (candidate, score) = (0..WORKERS).map(|n| (n+start)%WORKERS)
                .map(|candidate| (candidate,load(candidate))).min_by_key(|&(_,score)|score)?;
            if score==u32::MAX {return None;}
            *owner=candidate as u8+1;
        }
        Some(usize::from(*owner - 1))
    }

    pub fn release_idle(&mut self, worker: usize, active: &[u32; BITMAP_WORDS]) {
        for (index, owner) in self.0.iter_mut().enumerate() {
            if usize::from(*owner)==worker+1 && active[index/32] & (1<<(index%32))==0 {
                *owner=0;
            }
        }
    }
}

pub fn inbound(ip: &[u8]) -> Option<usize> {
    if ip.len() < 28 || ip[0] != 0x45 { return None; }
    let external = match ip[9] {
        6 | 17 => u16at(ip,22),
        1 if [3,11,12].contains(&ip[20]) => {
            let quoted=ip.get(28..56)?;
            match quoted[9] { 1 => u16at(quoted,24), 6 | 17 => u16at(quoted,20), _ => return None }
        }
        1 => u16at(ip,24),
        _ => return None,
    };
    (external>=49152).then_some(usize::from(external)%WORKERS)
}

#[cfg(test)]
#[test]
fn direct_lan_steers_by_the_private_endpoint_and_preserves_control_delivery() {
    let mut frame = [0; 60];
    frame[..6].copy_from_slice(&crate::bridge::MAC);
    frame[6] = 2;
    frame[11] = 9;
    frame[12..15].copy_from_slice(&[8, 0, 0x45]);
    frame[23] = 17;
    crate::setip(&mut frame, 26, crate::LAN[0] + 2);
    crate::setip(&mut frame, 30, 0x0808_0808);
    crate::set16(&mut frame, 34, 1234);
    crate::set16(&mut frame, 36, 53);
    let owner = direct_lan_owner(&frame).unwrap();
    assert_eq!(outbound_transport(&frame[14..]), Some(owner));
    crate::setip(&mut frame, 30, 0x0909_0909);
    crate::set16(&mut frame, 36, 123);
    assert_eq!(direct_lan_owner(&frame), Some(owner));
    crate::setip(&mut frame, 30, crate::LAN[0]);
    assert_eq!(direct_lan_owner(&frame), None);
    crate::setip(&mut frame, 30, u32::MAX);
    crate::set16(&mut frame, 34, 68);
    crate::set16(&mut frame, 36, 67);
    assert_eq!(direct_lan_owner(&frame), None);
    frame[..6].fill(255);
    assert_eq!(direct_lan_owner(&frame), None);
}
