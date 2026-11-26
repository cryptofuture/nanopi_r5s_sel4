use smoltcp::{phy::{self,Device,DeviceCapabilities,Medium},time::Instant};
use r5s_router_core::{*,arp};
use crate::queue;
pub const MAC:[u8;6]=[2,0x52,0x35,0x53,0,1];
pub struct Net {pub epoch:u32,pub allow:bool,pub remaining:usize,pub candidate:Ip,pub active:Ip,pub gateway:Ip,pub gateway_seen:u64,pub conflict:bool,pub tx_drops:u32,pub rx_drops:u32,pub fault:bool}
impl Net {
 pub fn new()->Self {Self {epoch:0,allow:false,remaining:0,candidate:0,active:0,gateway:0,gateway_seen:0,conflict:false,tx_drops:0,rx_drops:0,fault:false}}
 pub fn send(&mut self,frame:&[u8]) {
  match queue::push(queue::BASE+queue::DIRECTION,self.epoch,frame) {Ok(true)=>{},Ok(false)=>self.tx_drops=self.tx_drops.saturating_add(1),Err(())=>self.fault=true}
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
  if ip[9]==17 && len>=268 && u16at(ip,20)==67 && u16at(ip,22)==68 && u16at(ip,24) as usize==len-20 {
   let data=&ip[28..len];return data[28..34]==MAC && dhcp::reply_options(data).is_ok();
  }
  ip[9]==1 && len>=28 && ip[20]==0 && ip[21]==0 && u16at(ip,24)==0x5242 && ipat(ip,12)==self.gateway && ipat(ip,16)==self.active
 }
}
pub struct Rx {data:[u8;1514],length:usize}
impl phy::RxToken for Rx {fn consume<R,F>(self,f:F)->R where F:FnOnce(&[u8])->R {f(&self.data[..self.length])}}
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
   let length=match queue::pop(queue::BASE,self.epoch,&mut data) {Ok(n)=>n,Err(())=>{self.fault=true;return None}};
   if length==0 {return None;}
   if self.accept(&data[..length],time.total_millis() as u64) {return Some((Rx {data,length},Tx(self)));}
   self.rx_drops=self.rx_drops.saturating_add(1);
  }
  None
 }
 fn transmit(&mut self,_time:Instant)->Option<Tx<'_>> {self.allow.then_some(Tx(self))}
 fn capabilities(&self)->DeviceCapabilities {let mut c=DeviceCapabilities::default();c.medium=Medium::Ethernet;c.max_transmission_unit=1514;c.max_burst_size=Some(8);c}
}
