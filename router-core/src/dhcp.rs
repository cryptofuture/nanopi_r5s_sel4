use crate::*;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub struct Options<'a> {pub kind:u8,pub server:Option<Ip>,pub requested:Option<Ip>,pub client:&'a [u8],pub lease:Option<u32>,pub renew:Option<u32>,pub rebind:Option<u32>}
pub fn options(packet:&[u8])->Result<Options<'_>,Drop> {parse_options(packet,1)}
pub fn reply_options(packet:&[u8])->Result<Options<'_>,Drop> {parse_options(packet,2)}
fn parse_options(packet:&[u8],op:u8)->Result<Options<'_>,Drop> {
 if packet.len()<240 || packet.len()>1500 || packet[0]!=op || packet[1]!=1 || packet[2]!=6 || packet[3]!=0 || packet[236..240]!=[99,130,83,99] || ipat(packet,24)!=0 {return Err(Drop::Malformed);}
 let mut result=Options {kind:0,server:None,requested:None,client:&packet[28..34],lease:None,renew:None,rebind:None};let mut seen=[false;256];let mut i=240;let mut ended=false;
 while i<packet.len() {
  let code=packet[i];i+=1;if code==0 {continue;}if code==255 {ended=true;break;}
  if i>=packet.len() {return Err(Drop::Malformed);}let size=packet[i] as usize;i+=1;if i+size>packet.len() || seen[code as usize] {return Err(Drop::Malformed);}seen[code as usize]=true;
  let value=&packet[i..i+size];i+=size;
  match code {
   53 if size==1=>result.kind=value[0],54 if size==4=>result.server=Some(ipat(value,0)),50 if size==4=>result.requested=Some(ipat(value,0)),
   61 if (2..=64).contains(&size)=>result.client=value,51 if size==4=>result.lease=Some(ipat(value,0)),58 if size==4=>result.renew=Some(ipat(value,0)),59 if size==4=>result.rebind=Some(ipat(value,0)),
   1|3 if size==4=>{},6 if size>=4 && size<=12 && size%4==0=>{},
   1|3|6|53|54|50|61|51|52|58|59=>return Err(Drop::Malformed),_=>{}
  }
 }
 if !ended || !(1..=8).contains(&result.kind) {return Err(Drop::Malformed);}
 if op==2 && result.kind==5 {
  let lease=result.lease.ok_or(Drop::Malformed)?;
  let renew=result.renew.unwrap_or(lease/2);let rebind=result.rebind.unwrap_or(((lease as u64)*7/8) as u32);
  if !(4..=604800).contains(&lease) || renew==0 || renew>=rebind || rebind>=lease || result.server.is_none() {return Err(Drop::Malformed);}
 }
 Ok(result)
}
#[derive(Clone,Copy)]
struct Lease {xid:u32,state:u8,id:[u8;64],size:usize,mac:[u8;6],until:u64}
const EMPTY:Lease=Lease {xid:0,state:0,id:[0;64],size:0,mac:[0;6],until:0};
pub const LEASE_SECONDS:u32=12*60*60;
const POOL_SIZE: usize = if cfg!(feature = "integrated-lan") {101} else {100};
pub struct Server {port:Port,leases:[Lease;POOL_SIZE],next_expiry:u64,expiry_cursor:usize,expired:bool,pub generation:u64}
#[derive(Debug,PartialEq)]
pub enum Reply {None,Probe(Ip),Offer(Ip),Ack(Ip),Nak,Released(Ip)}
impl Server {
 pub fn new(port:Port)->Result<Self,Drop> {if port==Port::Wan {return Err(Drop::Policy);}Ok(Self {port,leases:[EMPTY;POOL_SIZE],next_expiry:u64::MAX,expiry_cursor:0,expired:false,generation:0})}
 pub fn used(&self)->usize {self.leases.iter().filter(|l|l.state!=0).count()}
 pub fn address(&self,index:usize)->Ip {(LAN[self.port as usize-1]&0xffffff00)+100+index as u32}
 pub fn expire(&mut self,now:u64,router:&mut crate::nat::Router) {
  if self.expiry_cursor==0 && now<self.next_expiry {return;}
  self.expiry_cursor=0;
  self.next_expiry=u64::MAX;
  self.expired |= self.expire_range(0,POOL_SIZE,now,router);
 }
 pub(crate) fn maintain(&mut self,now:u64,router:&mut crate::nat::Router)->bool {
  let expired=core::mem::take(&mut self.expired);
  if self.expiry_cursor==0 {
   if now<self.next_expiry {return expired;}
   self.next_expiry=u64::MAX;
  }
  let end=(self.expiry_cursor+16).min(POOL_SIZE);
  let changed=self.expire_range(self.expiry_cursor,end,now,router);
  self.expiry_cursor=if end==POOL_SIZE {0}else{end};
  changed || expired
 }
 fn expire_range(&mut self,start:usize,end:usize,now:u64,router:&mut crate::nat::Router)->bool {
  let mut changed=false;
  for i in start..end {
   let lease=self.leases[i];
   if lease.state==0 {continue;}
   if lease.until<=now {
    router.release_client(self.port,self.address(i),now);
    self.leases[i]=EMPTY;
    changed=true;
   } else {
    self.next_expiry=self.next_expiry.min(lease.until);
   }
  }
  changed
 }
 fn set_deadline(&mut self,index:usize,until:u64) {
  self.leases[index].until=until;
  self.next_expiry=self.next_expiry.min(until);
 }
 pub(crate) fn cancel_probe(&mut self,address:Ip) {
  if let Some(i)=(0..POOL_SIZE).find(|&i|self.address(i)==address) {
   if self.leases[i].state==1 {self.leases[i]=EMPTY;}
  }
 }
 pub fn probe_result(&mut self,address:Ip,conflict:bool,now:u64)->Result<Reply,Drop> {
  let Some(i)=(0..POOL_SIZE).find(|&i|self.address(i)==address) else {return Err(Drop::Malformed)};
  if self.leases[i].state!=1 || self.leases[i].until<=now {return Err(Drop::State);}
  self.leases[i].state=if conflict {4}else{2};self.set_deadline(i,now.saturating_add(if conflict {600000}else{30000}));
  Ok(if conflict {Reply::None}else{Reply::Offer(address)})
 }
 pub fn receive(&mut self,ingress:Port,p:&[u8],now:u64,router:&mut crate::nat::Router)->Result<Reply,Drop> {
  if ingress!=self.port {return Err(Drop::Policy);}let opt=options(p)?;
  let mac:[u8;6]=p[28..34].try_into().unwrap();if mac[0]&1!=0 || mac==[0;6] {return Err(Drop::Malformed);}
  self.expire(now,router);
  let server=LAN[self.port as usize-1];
  if opt.server.is_some_and(|s|s!=server) {return Ok(Reply::None);}
  let existing=self.leases.iter().position(|l|l.state!=0 && l.state!=4 && l.size==opt.client.len() && l.id[..l.size]==*opt.client && l.mac==mac);
  match opt.kind {
   1=>{
    let i=if let Some(i)=existing {i}else{self.leases.iter().position(|l|l.state==0).ok_or(Drop::Full)?};
    self.leases[i].xid=ipat(p,4);
    if self.leases[i].state>=2 {return Ok(Reply::Offer(self.address(i)));}
    if self.leases[i].state==1 {return Ok(Reply::Probe(self.address(i)));}
    router.release_client(self.port,self.address(i),now);
    let mut id=[0;64];id[..opt.client.len()].copy_from_slice(opt.client);
    self.leases[i]=Lease {xid:ipat(p,4),state:1,id,size:opt.client.len(),mac,until:0};
    self.set_deadline(i,now.saturating_add(30000));Ok(Reply::Probe(self.address(i)))
   }
   3=>{
    let ciaddr=ipat(p,12);
    if (ciaddr!=0 && opt.requested.is_some()) || (opt.server.is_some() && opt.requested.is_none()) {return Err(Drop::Malformed);}
    let requested=opt.requested.unwrap_or(ciaddr);
    if let Some(i)=existing {
     if opt.server.is_some() && self.leases[i].xid!=ipat(p,4) {return Ok(Reply::None);}
     if self.address(i)==requested && (self.leases[i].state==2 || self.leases[i].state==3) {
      self.leases[i].state=3;self.set_deadline(i,now.saturating_add(u64::from(LEASE_SECONDS)*1000));return Ok(Reply::Ack(requested));
     }
    }
    if existing.is_none() && opt.server.is_none() && ciaddr==0 && requested&0xffffff00==server&0xffffff00 {return Ok(Reply::None);}
    Ok(Reply::Nak)
   }
   4|7=>{
    if let Some(i)=existing {
     let address=self.address(i);let expected=if opt.kind==4 {opt.requested.unwrap_or(0)}else{ipat(p,12)};
     if address!=expected {return Ok(Reply::None);}
     if opt.kind==4 {self.leases[i].state=4;self.set_deadline(i,now.saturating_add(600000));}else{self.leases[i]=EMPTY;}
     router.release_client(self.port,address,now);self.generation=self.generation.wrapping_add(1);Ok(Reply::Released(address))
    } else {Ok(Reply::None)}
   }
   8=>{let address=ipat(p,12);if address&0xffffff00!=server&0xffffff00 || address&255==0 || address&255==255 || address==server {return Err(Drop::Malformed);}Ok(Reply::Ack(address))},_=>Ok(Reply::None)
  }
 }
 pub fn response(&self,request:&[u8],kind:u8,address:Ip,dns:&[Ip],out:&mut[u8])->Result<usize,Drop> {
  self.response_with_routes(request,kind,address,dns,true,out)
 }
 pub(crate) fn response_with_routes(&self,request:&[u8],kind:u8,address:Ip,dns:&[Ip],advertise_router:bool,out:&mut[u8])->Result<usize,Drop> {
  if request.len()<240 || out.len()<300 || dns.len()>3 || dns.iter().any(|&ip|!unicast(ip)) || ![2,5,6].contains(&kind) {return Err(Drop::Malformed);}
  let inform=options(request)?.kind==8;
  out[..300].fill(0);out[..44].copy_from_slice(&request[..44]);out[0]=2;setip(out,16,if kind==6 || inform {0}else{address});setip(out,20,LAN[self.port as usize-1]);out[236..240].copy_from_slice(&[99,130,83,99]);if kind==6 {setip(out,12,0);set16(out,10,0x8000);}
  let mut i=240;let mut option=|code:u8,value:&[u8]|{out[i]=code;out[i+1]=value.len() as u8;out[i+2..i+2+value.len()].copy_from_slice(value);i+=2+value.len();};
  option(53,&[kind]);option(54,&LAN[self.port as usize-1].to_be_bytes());
  if kind!=6 {
   option(1,&0xffffff00u32.to_be_bytes());if advertise_router {option(3,&LAN[self.port as usize-1].to_be_bytes());}let mut values=[0;12];for (n,ip) in dns.iter().enumerate() {values[n*4..n*4+4].copy_from_slice(&ip.to_be_bytes());}if !dns.is_empty() {option(6,&values[..dns.len()*4]);}
   if !inform {option(51,&LEASE_SECONDS.to_be_bytes());option(58,&(LEASE_SECONDS/2).to_be_bytes());option(59,&(LEASE_SECONDS*7/8).to_be_bytes());}
  }
  out[i]=255;Ok(300.max(i+1))
 }
}
pub fn decline(mac:[u8;6],address:Ip,server:Ip,xid:u32,out:&mut[u8])->Result<usize,Drop> {
 if out.len()<342 || !unicast(address) || !unicast(server) {return Err(Drop::Malformed);}
 out[..342].fill(0);out[..6].fill(255);out[6..12].copy_from_slice(&mac);out[12..14].copy_from_slice(&[8,0]);
 let ip=&mut out[14..342];ip[0]=0x45;set16(ip,2,328);ip[8]=64;ip[9]=17;setip(ip,16,u32::MAX);set16(ip,20,68);set16(ip,22,67);set16(ip,24,308);
 let d=&mut ip[28..];d[0]=1;d[1]=1;d[2]=6;setip(d,4,xid);set16(d,10,0x8000);d[28..34].copy_from_slice(&mac);d[236..240].copy_from_slice(&[99,130,83,99]);
 d[240..245].copy_from_slice(&[53,1,4,50,4]);setip(d,245,address);d[249..251].copy_from_slice(&[54,4]);setip(d,251,server);d[255..258].copy_from_slice(&[61,7,1]);d[258..264].copy_from_slice(&mac);d[264]=255;
 let c=checksum(&ip[..20]);set16(ip,10,c);let c=transport_checksum(ip,328);set16(ip,26,if c==0 {65535}else{c});Ok(342)
}

#[cfg(test)]
#[test]
fn expiry_slice_and_dhcp_request_keep_lease_reuse_exact() {
    let mut router = crate::nat::Router::new(1);
    let mut server = Server::new(Port::Lan1).unwrap();
    server.leases[0].state = 3;
    server.set_deadline(0, 50);
    server.leases[POOL_SIZE - 1].state = 3;
    server.set_deadline(POOL_SIZE - 1, 50);
    assert!(!server.maintain(49, &mut router));
    assert!(server.maintain(50, &mut router));
    assert_eq!(server.used(), 1);
    server.expire(50, &mut router);
    assert_eq!(server.used(), 0);
    assert!(server.maintain(50, &mut router));
    assert!(!server.maintain(50, &mut router));
}
