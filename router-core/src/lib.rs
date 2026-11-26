#![no_std]
pub mod nat;
pub mod steering;
pub mod dhcp;
pub mod arp;
pub mod ethernet;
pub mod bridge;
pub mod service;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
#[repr(u8)]
pub enum Port {Wan=0,Lan1=1,Lan2=2}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum Drop {Malformed,Checksum,Unsupported,Spoof,Policy,NoWan,Full,Ttl,Mtu,State,Overlap,NatClientLimit,NatSlots,NatIds,NeighborBusy}
pub type Ip=u32;
#[cfg(not(feature = "integrated-lan"))]
pub const LAN:[Ip;2]=[0x0a2c0101,0x0a2c0201];
#[cfg(feature = "integrated-lan")]
pub const LAN: [Ip; 2] = [0xc0a80a01, 0xc0a81401];
pub fn unicast(ip:Ip)->bool {ip!=0 && ip!=u32::MAX && ip>>24!=127 && ip>>24!=0 && ip>>28<14}
pub fn internal(ip:Ip)->bool {LAN.iter().any(|net|ip&0xffffff00==net&0xffffff00)}
pub fn checksum(bytes:&[u8])->u16 {
 let mut sum=0u32;let mut chunks=bytes.chunks_exact(2);
 for b in &mut chunks {sum+=u16::from_be_bytes([b[0],b[1]]) as u32;}
 if let Some(&last)=chunks.remainder().first() {sum+=(last as u32)<<8;}
 while sum>>16!=0 {sum=(sum&0xffff)+(sum>>16);}!(sum as u16)
}
pub fn u16at(p:&[u8],i:usize)->u16 {u16::from_be_bytes([p[i],p[i+1]])}
pub fn ipat(p:&[u8],i:usize)->Ip {Ip::from_be_bytes(p[i..i+4].try_into().unwrap())}
pub fn set16(p:&mut[u8],i:usize,v:u16) {p[i..i+2].copy_from_slice(&v.to_be_bytes());}
pub fn setip(p:&mut[u8],i:usize,v:Ip) {p[i..i+4].copy_from_slice(&v.to_be_bytes());}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub struct Wan {pub address:Ip,pub mask:Ip,pub gateway:Ip,pub dns:[Ip;3],pub dns_count:usize,pub expires:u64}
impl Wan {
 pub fn validate(&self,now:u64)->Result<(),Drop> {
  let prefix=self.mask.leading_ones();
  if !(1..=30).contains(&prefix) || self.mask!=u32::MAX<<(32-prefix) || !unicast(self.address) || !unicast(self.gateway) || self.address==self.gateway || self.address&self.mask!=self.gateway&self.mask || self.address&!self.mask==0 || self.address&!self.mask==!self.mask || self.gateway&!self.mask==0 || self.gateway&!self.mask==!self.mask || self.expires<=now || !(1..=3).contains(&self.dns_count) {return Err(Drop::Malformed);}
  for lan in LAN {let common=self.mask&0xffffff00;if (lan^self.address)&common==0 {return Err(Drop::Overlap);}}
  for &dns in &self.dns[..self.dns_count] {if !unicast(dns) || internal(dns) || dns==self.address || (dns&self.mask==self.address&self.mask && (dns&!self.mask==0 || dns&!self.mask==!self.mask)) {return Err(Drop::Malformed);}}
  Ok(())
 }
 pub fn next_hop(&self,destination:Ip)->Ip {if destination&self.mask==self.address&self.mask {destination}else{self.gateway}}
}
pub fn validate_ip(packet:&[u8])->Result<usize,Drop> {
 if packet.len()<20 || packet[0]!=0x45 {return Err(Drop::Unsupported);}
 let length=u16at(packet,2) as usize;
 if length<20 || length>packet.len() {return Err(Drop::Malformed);}
 if checksum(&packet[..20])!=0 {return Err(Drop::Checksum);}
 if u16at(packet,6)&0xbfff!=0 {return Err(Drop::Unsupported);}
 Ok(length)
}
pub fn transport_checksum(packet:&[u8],length:usize)->u16 {
 let mut sum=0u32;
 for i in (12..20).step_by(2) {sum+=u16at(packet,i) as u32;}
 sum+=packet[9] as u32+(length-20) as u32;
 for chunk in packet[20..length].chunks(2) {sum+=if chunk.len()==2 {u16::from_be_bytes([chunk[0],chunk[1]]) as u32}else{(chunk[0] as u32)<<8};}
 while sum>>16!=0 {sum=(sum&0xffff)+(sum>>16);}!(sum as u16)
}
