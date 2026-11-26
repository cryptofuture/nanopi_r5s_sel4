use crate::*;
pub fn parse(frame:&[u8])->Result<(u16,[u8;6],Ip,Ip),Drop> {
 if frame.len()<42 || frame[12..14]!=[8,6] || frame[14..20]!=[0,1,8,0,6,4] {return Err(Drop::Malformed);}
 let op=u16at(frame,20);if op!=1 && op!=2 {return Err(Drop::Malformed);}
 let mac:[u8;6]=frame[22..28].try_into().unwrap();if mac[0]&1!=0 || mac==[0;6] || frame[6..12]!=mac {return Err(Drop::Spoof);}
 Ok((op,mac,ipat(frame,28),ipat(frame,38)))
}
pub fn request(mac:[u8;6],source:Ip,target:Ip,out:&mut[u8])->Result<usize,Drop> {
 if out.len()<60 {return Err(Drop::Malformed);}out[..60].fill(0);out[..6].fill(255);out[6..12].copy_from_slice(&mac);out[12..20].copy_from_slice(&[8,6,0,1,8,0,6,4]);set16(out,20,1);out[22..28].copy_from_slice(&mac);setip(out,28,source);setip(out,38,target);Ok(60)
}
#[derive(Debug,PartialEq)]
pub enum Action {None,Probe(Ip),Bind(Ip),Announce(Ip)}
pub struct Acd {pub address:Ip,next:u64,count:u8,bound:bool,announcements:u8}
impl Acd {
 pub fn new(address:Ip,now:u64,seed:u64)->Self {Self {address,next:now+seed%1000,count:0,bound:false,announcements:0}}
 pub fn tick(&mut self,now:u64)->Action {
  if now<self.next {return Action::None;}
  if self.count<3 {self.count+=1;self.next=now+if self.count==3 {2000}else{1000};return Action::Probe(self.address);}
  if !self.bound {self.bound=true;self.next=now;return Action::Bind(self.address);}
  if self.announcements<2 {self.announcements+=1;self.next=now+2000;return Action::Announce(self.address);}
  Action::None
 }
 pub fn conflict(&self,frame:&[u8],own:[u8;6])->bool {
  parse(frame).is_ok_and(|(_,mac,source,target)|mac!=own && (source==self.address || source==0 && target==self.address))
 }
}
