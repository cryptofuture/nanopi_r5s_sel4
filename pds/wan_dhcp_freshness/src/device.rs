use smoltcp::{phy::{self,Device,DeviceCapabilities,Medium},time::Instant};
use r5s_router_core::{*,arp};
use crate::queue;
pub const MAC:[u8;6]=[2,0x52,0x35,0x53,0,1];
#[cfg(feature="shared-wan")]
pub const INTERNET_TARGET: Ip = 0x08080808;
#[cfg(feature="shared-wan")]
pub const INTERNET_SEQ: u16 = 0x8000;
#[cfg(feature="shared-wan")]
pub const INTERNET_LIMIT: u16 = 3;
pub struct Net {
 #[cfg(feature="shared-wan")] pub internet_cookie: u64,
 #[cfg(feature="shared-wan")] pub internet_tx_mask: u8,
 #[cfg(feature="b51-udp")] pub bound_mask: Ip,
 #[cfg(feature="b51-udp")] next_report_ms: u64,
 #[cfg(feature="lan-status")] fallback: Option<arp::Acd>,
 #[cfg(feature="lan-status")] fallback_conflict: bool,
 #[cfg(feature="lan-status")] pub fallback_active: Ip,
 pub progress:crate::progress::Progress,pub epoch:u32,pub allow:bool,pub remaining:usize,pub candidate:Ip,pub active:Ip,pub gateway:Ip,pub gateway_seen:u64,pub conflict:bool,pub tx_drops:u32,pub rx_drops:u32,pub fault:bool}
impl Net {
 pub fn new()->Self {Self {
  #[cfg(feature="shared-wan")] internet_cookie: 0,
  #[cfg(feature="shared-wan")] internet_tx_mask: 0,
  #[cfg(feature="b51-udp")] bound_mask: 0,
  #[cfg(feature="b51-udp")] next_report_ms: 0,
  #[cfg(feature="lan-status")] fallback: None,
  #[cfg(feature="lan-status")] fallback_conflict: false,
  #[cfg(feature="lan-status")] fallback_active: 0,
  progress:Default::default(),epoch:0,allow:false,remaining:0,candidate:0,active:0,gateway:0,gateway_seen:0,conflict:false,tx_drops:0,rx_drops:0,fault:false}}
 #[cfg(feature="lan-status")]
 pub fn update_fallback(&mut self, now: u64, ip: Ip, phy: bool, acquisition: Option<u64>) {
    if !phy || self.active != 0 || self.candidate != 0 {
        self.fallback = None;
        self.fallback_active = 0;
        self.fallback_conflict = false;
    } else if self.allow && ip != 0 && !self.fallback_conflict
        && acquisition.is_some_and(|start| now.saturating_sub(start) >= 45000)
    {
        let acd = self.fallback.get_or_insert_with(|| arp::Acd::new(ip, now, now));
        let source = match acd.tick(now) {
            arp::Action::Probe(_) => 0,
            arp::Action::Announce(_) => ip,
            arp::Action::Bind(_) => { self.fallback_active = ip; return; }
            arp::Action::None => return,
        };
        let mut frame = [0; 60];
        if let Ok(size) = arp::request(MAC, source, ip, &mut frame) {
            if !self.send(&frame[..size]) {
                self.fallback = None;
                self.fallback_active = 0;
                self.fallback_conflict = true;
            }
        }
    }
 }

 #[cfg(feature="lan-status")]
 fn fallback_arp(&mut self, frame: &[u8]) -> bool {
    let Some(acd) = self.fallback.as_ref() else { return false };
    if acd.conflict(frame, MAC) {
        self.fallback = None;
        self.fallback_active = 0;
        self.fallback_conflict = true;
        return true;
    }
    if self.fallback_active == 0 || (frame[..6] != MAC && frame[..6] != [255; 6]) { return false; }
    if let Ok((1, peer, source, target)) = arp::parse(frame) {
        if target == self.fallback_active && source & 0xffffff00 == target & 0xffffff00 && peer != MAC {
            let mut reply = [0; 60];
            if arp::request(MAC, target, source, &mut reply).is_ok() {
                reply[..6].copy_from_slice(&peer);
                reply[32..38].copy_from_slice(&peer);
                set16(&mut reply, 20, 2);
                self.send(&reply);
                return true;
            }
        }
    }
    false
 }

 pub fn send(&mut self,frame:&[u8]) -> bool {
  #[cfg(feature="zc")]
  let queued={
   let queued=queue::push(crate::zc::WAN_CONTROL_TX,self.epoch,frame);
   if queued==Ok(true) && option_env!("R5S_GMAC_HYBRID_IRQ")==Some("1") {
    sel4_microkit::Channel::new(crate::zc::WAN_CONTROL_TX_CHANNEL).notify();
   }
   queued
  };
  #[cfg(not(feature="zc"))]
  let queued=queue::push(queue::BASE+queue::DIRECTION,self.epoch,frame);
  self.finish_send(frame,queued)
 }

 #[cfg(all(feature="zc", feature="shared-wan"))]
 pub fn send_forwarded(&mut self,frame:&[u8]) -> bool {
  use crate::zc;
  let queued=zc::RefQueue::push_copy(zc::link_queue(zc::LINKS_VA,zc::Q_APP_TX),
   ||queue::push(queue::BASE+queue::DIRECTION,self.epoch,frame));
  self.finish_send(frame,queued)
 }

 fn finish_send(&mut self,frame:&[u8],queued:Result<bool,()>) -> bool {
  match queued {
   Ok(true)=>{
    self.progress.observe(frame,true,MAC);
    #[cfg(feature="shared-wan")]
    if let Some(seq) = self.internet_echo(frame, 8) { self.internet_tx_mask |= 1 << seq; }
   }
   Ok(false)=>self.tx_drops=self.tx_drops.saturating_add(1),Err(())=>self.fault=true
  }
  queued==Ok(true)
 }

 /// Zero-copy WAN ingress. Forwardable unicast IPv4 to the lease address is
 /// relayed to the logical LAN by reference; ARP, DHCP, status requests and
 /// everything else is copied for the local smoltcp path, which keeps the
 /// copy-path relay behavior exactly.
 #[cfg(feature = "zc")]
 fn zc_take(&mut self, data: &mut [u8; 1514]) -> ZcTake {
  use crate::zc;
  let Ok(Some(r)) = zc::RefQueue::pop(zc::link_queue(zc::LINKS_VA, zc::Q_GMAC_RX)) else {
   return ZcTake::Empty;
  };
  if r.flags & zc::FLAG_COPY != 0 {
   return ZcTake::Marker;
  }
  if r.epoch != self.epoch {
   let _ = zc::reject(zc::META_VA, &r);
   return ZcTake::Handled;
  }
  let length = r.len as usize;
  if !(14..=zc::BUFFER).contains(&length) {
   let _ = zc::reject(zc::META_VA, &r);
   return ZcTake::Empty;
  }
  let buffer = unsafe {
   core::slice::from_raw_parts((zc::POOL_VA + r.id() as usize * zc::BUFFER) as *const u8, zc::BUFFER)
  };
  let frame = &buffer[..length];
  let status_request = length >= 50 && frame[23] == 17 && u16at(frame, 36) == 48125
   && frame[42..50] == *b"B51Q\x00\x01\x00\x00";
  let forward = !status_request && self.active != 0 && frame[..6] == MAC
   && length >= 34 && frame[12..14] == [8, 0] && ipat(frame, 30) == self.active
   && self.internet_echo(frame, 0).is_none();
  if forward {
   if zc::RefQueue::push(zc::link_queue(zc::LINKS_VA, zc::Q_APP_FWD), &r) == Ok(true) {
    return ZcTake::Handled;
   }
   zc::count(zc::META_VA, zc::C_FALLBACK_QUEUE);
  }
  data[..length].copy_from_slice(frame);
  zc::count(zc::META_VA, zc::C_COPIED_PACKETS);
  zc::add_copied_bytes(zc::META_VA, length as u32);
  let _ = zc::release_cpu(zc::META_VA, &r);
  ZcTake::Copied(length)
 }
 #[cfg(feature="shared-wan")]
 fn internet_echo(&self, frame: &[u8], kind: u8) -> Option<u16> {
    if self.active == 0 || frame.get(12..14) != Some(&[8, 0]) { return None; }
    let ip = &frame[14..];
    if ip.len() < 44 || ip[9] != 1 || ip[20..22] != [kind, 0] || u16at(ip, 24) != 0x5242
        || validate_ip(ip) != Ok(44) || checksum(&ip[20..44]) != 0
        || ip[28..36] != self.internet_cookie.to_be_bytes() { return None; }
    let (source, destination) = if kind == 8 { (self.active, INTERNET_TARGET) } else { (INTERNET_TARGET, self.active) };
    let sequence = u16at(ip, 26);
    let index = sequence.wrapping_sub(INTERNET_SEQ);
    (ipat(ip, 12) == source && ipat(ip, 16) == destination && index < INTERNET_LIMIT
        && ip[36..44] == u64::from(sequence).to_be_bytes()).then_some(index)
 }
 fn accept(&mut self,frame:&[u8],now:u64)->bool {
  if frame.len()<14 || (frame[..6]!=MAC && frame[..6]!=[255;6]) || frame[6]&1!=0 || frame[6..12]==MAC || frame[6..12]==[0;6] {return false;}
  if frame[12..14]==[8,6] {
   if let Ok((op,_,source,target))=arp::parse(frame) {
    if (self.candidate!=0 && (source==self.candidate || source==0 && target==self.candidate)) || (self.active!=0 && source==self.active) {self.conflict=true;}
    if op==2 && source==self.gateway && target==self.active && self.active!=0 && frame[32..38]==MAC {self.gateway_seen=now;}
    return true;
   }
   return false;
  }
  if frame[12..14]!=[8,0] {return false;}
  let ip=&frame[14..];let Ok(len)=validate_ip(ip) else {return false};
  #[cfg(feature="shared-wan")]
  if self.internet_echo(frame, 0).is_some() { return true; }
  if ip[9]==17 && len>=268 && u16at(ip,20)==67 && u16at(ip,22)==68 && u16at(ip,24) as usize==len-20 {
   let data=&ip[28..len];
   let valid=data[28..34]==MAC && dhcp::reply_options(data).is_ok_and(|opt| self.progress.accepts(data,&opt)) &&
    (u16at(ip,26)==0 || transport_checksum(ip,len)==0);
   if valid {self.progress.observe(frame,false,MAC);}
   return valid;
  }
  ip[9]==1 && len>=28 && ip[20]==0 && ip[21]==0 && u16at(ip,24)==0x5242 && ipat(ip,12)==self.gateway && ipat(ip,16)==self.active
 }
}
pub struct Rx {data:[u8;1514],length:usize}
impl phy::RxToken for Rx {fn consume<R,F>(self,f:F)->R where F:FnOnce(&[u8])->R {f(&self.data[..self.length])}}
#[cfg(feature = "zc")]
enum ZcTake { Empty, Marker, Handled, Copied(usize) }
pub struct Tx<'a>(&'a mut Net);
impl phy::TxToken for Tx<'_> {
 fn consume<R,F>(self,len:usize,f:F)->R where F:FnOnce(&mut[u8])->R {
  assert!(len<=1514);let mut data=[0;1514];let result=f(&mut data[..len]);self.0.send(&data[..len]);result
 }
}
impl Device for Net {
 type RxToken<'a>=Rx;type TxToken<'a>=Tx<'a>;
 fn receive(&mut self,time:Instant)->Option<(Rx,Tx<'_>)> {
  if !self.allow {return None;}
  while self.remaining!=0 {
   self.remaining-=1;let mut data=[0;1514];
   #[cfg(feature = "zc")]
   let (length,_verified)=match self.zc_take(&mut data) {
    ZcTake::Handled => continue,
    ZcTake::Copied(length) => (length,0u32),
    ZcTake::Empty => return None,
    ZcTake::Marker => match queue::pop_meta(queue::BASE,self.epoch,&mut data) {Ok(n)=>n,Err(())=>{self.fault=true;return None}},
   };
   #[cfg(not(feature = "zc"))]
   let (length,_verified)=match queue::pop_meta(queue::BASE,self.epoch,&mut data) {Ok(n)=>n,Err(())=>{self.fault=true;return None}};
   if length==0 {return None;}
   #[cfg(feature="lan-status")]
   if self.fallback_arp(&data[..length]) { continue; }
   #[cfg(feature="b51-udp")]
   {
    let now = time.total_millis() as u64;
    let (address, mask) = (self.active, self.bound_mask);
    #[cfg(feature="lan-status")]
    let (address, mask) = if address == 0 { (self.fallback_active, 0xffffff00) } else { (address, mask) };
     if now >= self.next_report_ms && crate::b51_udp::reply(&mut data[..length], MAC, address, mask, crate::b51_snapshot) {
     self.next_report_ms = now.saturating_add(250);
     self.send(&data[..crate::b51_udp::frame_length(&data)]);
     continue;
    }
   }
   #[cfg(feature="shared-wan")]
   if self.active != 0 && data[..6] == MAC {
    let frame = &data[..length];
    if arp::parse(frame).is_ok_and(|(op, _, _, target)| op == 2 && target == self.active)
        || (length >= 34 && frame[12..14] == [8, 0]
            && ipat(frame, 30) == self.active && self.internet_echo(frame, 0).is_none())
    {
     let copy = || queue::push_meta(0x64000000, self.epoch, frame,_verified);
     #[cfg(feature="zc")]
     let _ = crate::zc::RefQueue::push_copy(crate::zc::link_queue(crate::zc::LINKS_VA,crate::zc::Q_APP_FWD),copy);
     #[cfg(not(feature="zc"))]
     let _ = copy();
    }
   }
   if self.accept(&data[..length],time.total_millis() as u64) {return Some((Rx {data,length},Tx(self)));}
   #[cfg(feature="lan-gated")]
   if self.active!=0 && length>=34 && data[..6]==MAC && data[12..14]==[8,0] && ipat(&data,30)==self.active {
    // Only the WAN PD produces these copies. Backpressure never stalls local DHCP or ARP.
    let _ = queue::push(0x62000000,self.epoch,&data[..length]);
   }
   self.rx_drops=self.rx_drops.saturating_add(1);
  }
  None
 }
 fn transmit(&mut self,_time:Instant)->Option<Tx<'_>> {self.allow.then_some(Tx(self))}
 fn capabilities(&self)->DeviceCapabilities {let mut c=DeviceCapabilities::default();c.medium=Medium::Ethernet;c.max_transmission_unit=1514;c.max_burst_size=Some(8);c}
}
