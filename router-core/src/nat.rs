use crate::*;
const CAPACITY: usize = if cfg!(feature = "shared-nat") { 512 } else { 128 };
const SHARED_TCP_IDLE_MS: u64 = 600_000;
const SHARED_UDP_IDLE_MS: u64 = 30_000;
const SHARED_ICMP_IDLE_MS: u64 = 10_000;
const SHARED_TCP_MSL_MS: u64 = 15_000;
const SHARED_TCP_CLOSE_MS: u64 = 2 * SHARED_TCP_MSL_MS;

fn reuse_delay(shared_lan: bool, protocol: u8) -> u64 {
    if !shared_lan { return 120_000; }
    match protocol {
        6 => SHARED_TCP_CLOSE_MS,
        17 => SHARED_UDP_IDLE_MS,
        _ => SHARED_ICMP_IDLE_MS,
    }
}
#[derive(Clone,Copy)]
struct Flow {used:bool,realm:Port,proto:u8,private:Ip,local:u16,external:u16,remote:Ip,remote_port:u16,last:u64,created:u64,stage:u8,local_seq:u32,remote_seq:u32,local_next:u32,remote_next:u32,closed:u64}
const EMPTY:Flow=Flow {used:false,realm:Port::Lan1,proto:0,private:0,local:0,external:0,remote:0,remote_port:0,last:0,created:0,stage:0,local_seq:0,remote_seq:0,local_next:0,remote_next:0,closed:0};
const HASH_NAT: bool = match option_env!("R5S_NAT_HASH") {
    Some(value) => !matches!(value.as_bytes(), b"0"),
    None => true,
};
const PAIR_LOOKUP: bool = match option_env!("R5S_NAT_PAIR_LOOKUP") {
    Some(value) => !matches!(value.as_bytes(), b"0"),
    None => true,
};
const NO_SLOT: u16 = u16::MAX;
const OUTBOUND: usize = 0;
const EXTERNAL: usize = 1;
const ENDPOINT: usize = 2;

struct Index {
    heads: [[u16; CAPACITY]; 3],
    next: [[u16; CAPACITY]; 3],
}

fn bucket(a: u32, b: u32, c: u32) -> usize {
    let mut hash = a ^ b.wrapping_mul(0x9e37_79b1) ^ c.wrapping_mul(0x85eb_ca6b);
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x7feb_352d);
    (hash ^ (hash >> 15)) as usize & (CAPACITY - 1)
}

fn endpoint(port: Port, proto: u8, ip: Ip, local: u16) -> usize {
    bucket(ip, u32::from(local), (port as u32) << 8 | u32::from(proto))
}

fn outbound(port: Port, proto: u8, ip: Ip, local: u16, remote: Ip, remote_port: u16) -> usize {
    bucket(remote, u32::from(remote_port), endpoint(port, proto, ip, local) as u32)
}

fn external(proto: u8, port: u16) -> usize {
    bucket(u32::from(port), u32::from(proto), 0)
}

impl Index {
    const fn new() -> Self {
        Self { heads: [[NO_SLOT; CAPACITY]; 3], next: [[NO_SLOT; CAPACITY]; 3] }
    }

    fn keys(f: &Flow) -> [usize; 3] {
        [
            outbound(f.realm, f.proto, f.private, f.local, f.remote, f.remote_port),
            external(f.proto, f.external),
            endpoint(f.realm, f.proto, f.private, f.local),
        ]
    }

    fn replace(&mut self, slot: usize, old: &Flow, new: &Flow) {
        for (table, key) in Self::keys(new).into_iter().enumerate() {
            if old.proto != 0 {
                let successor = self.next[table][slot];
                let mut link = &mut self.heads[table][Self::keys(old)[table]];
                while *link != slot as u16 {
                    debug_assert_ne!(*link, NO_SLOT);
                    link = &mut self.next[table][usize::from(*link)];
                }
                *link = successor;
            }
            self.next[table][slot] = self.heads[table][key];
            self.heads[table][key] = slot as u16;
        }
    }
}

fn flow_deadline(f:&Flow,shared_lan:bool)->u64 {
 let timeout=match (f.proto,f.stage,shared_lan) {
    (6,0|1,false) => 30_000,
    (6,0..=2,true) => SHARED_TCP_IDLE_MS,
    (6,2,false) => 7_200_000,
    (6,_,_) | (17,_,true) | (1,_,true) => reuse_delay(shared_lan,f.proto),
    (17,_,false) => 120_000,
    _ => 60_000,
   };
 let mut deadline=f.last.saturating_add(timeout);
 if f.proto==6 {
  if f.stage==3 {deadline=deadline.min(f.closed);}
  if !shared_lan && f.stage<2 {deadline=deadline.min(f.created.saturating_add(30_000));}
 }
 deadline
}
pub struct Router {pub wan:Option<Wan>,pub generation:u64,flows:[Flow;CAPACITY],index:Index,pub drops:u64,pub forwarded:u64,shared_lan:bool,shard:u8,next_expiry:u64,expiry_cursor:usize,as_of:u64,notify_releases:bool,releases:[u32;8],slot_cursor:u32,port_cursor:u32,tcp_port_cursor:u32,icmp_port_cursor:u32}
impl Router {
 pub fn new(seed:u32)->Self {Self {wan:None,generation:0,flows:[EMPTY;CAPACITY],index:Index::new(),drops:0,forwarded:0,shared_lan:false,shard:0,next_expiry:0,expiry_cursor:0,as_of:0,notify_releases:false,releases:[0;8],slot_cursor:0,port_cursor:seed,tcp_port_cursor:seed.rotate_left(11),icmp_port_cursor:seed.rotate_left(21)}}
 pub(crate) fn enable_shared_lan(&mut self) { self.shared_lan=true; }
 pub fn sharded(seed:u32, worker:u8)->Self {
  assert!(worker<4);
  let mut router=Self::new(seed);
  router.enable_shared_lan();
  router.shard=worker+1;
  router
 }
 fn limit(&self)->usize {if self.shard==0 {CAPACITY} else {CAPACITY/4}}
 fn find(&self, table: usize, key: usize, matches: impl Fn(&Flow) -> bool) -> Option<usize> {
    if !HASH_NAT {
        return self.flows[..self.limit()].iter().position(matches);
    }
    let mut slot = self.index.heads[table][key];
    while slot != NO_SLOT {
        let index = usize::from(slot);
        if matches(&self.flows[index]) {
            return Some(index);
        }
        slot = self.index.next[table][index];
    }
    None
 }
 fn live(&self, flow: &Flow, now: u64) -> bool {
    flow.used && (!self.shared_lan || flow_deadline(flow, true) > now)
 }
 /// Reusable slot: unused and past its reuse delay. Scans forward from the
 /// per-worker cursor so steady-state allocation is O(1) with slack instead of
 /// always restarting at slot zero. Full table remains the bound.
 fn alloc_slot(&mut self, now: u64) -> Option<usize> {
    let limit = self.limit();
    let start = self.slot_cursor as usize % limit;
    for offset in 0..limit {
        let slot = (start + offset) % limit;
        let f = &self.flows[slot];
        if !self.live(f, now) && f.closed <= now {
            self.slot_cursor = (slot + 1) as u32 % limit as u32;
            return Some(slot);
        }
    }
    None
 }
 /// Collision-safe public port reservation. Probes forward from the per-worker
 /// cursor within the shard's stride-4 lane; a candidate is refused while any
 /// live or retired (reuse-delay) mapping still claims it.
 fn alloc_port(&mut self, protocol: u8, now: u64) -> Result<u16, Drop> {
    let (first, count) = if self.shared_lan {(49152, 16384)} else {(40000, 20000)};
    let stride = if self.shard == 0 {1} else {4};
    let lane = u32::from(self.shard.saturating_sub(1));
    let count = count / stride;
    // Separate protocol cursors preserve the shard lane and avoid coupling TCP
    // port selection to bursts of UDP or ICMP allocations.
    let cursor = match protocol {6 => self.tcp_port_cursor, 1 => self.icmp_port_cursor, _ => self.port_cursor};
    let mut index = cursor % count;
    for _ in 0..=self.limit() {
        let candidate = (first + stride * index + lane) as u16;
        index += 1;
        if index == count {index = 0;}
        if self.find(EXTERNAL, external(protocol, candidate), |f| f.proto == protocol
             && f.external == candidate && (self.live(f, now) || f.closed > now)).is_none() {
            match protocol {6 => self.tcp_port_cursor=index, 1 => self.icmp_port_cursor=index, _ => self.port_cursor=index}
            return Ok(candidate);
        }
    }
    Err(if self.shared_lan {Drop::NatIds} else {Drop::Full})
 }
 fn reverse(&self, proto: u8, port: u16, remote: Ip, peer: u16, now: u64) -> Option<usize> {
    self.find(EXTERNAL, external(proto, port), |f| {
        f.proto == proto && f.external == port && f.remote == remote && f.remote_port == peer && self.live(f, now)
    })
 }
 pub fn configure(&mut self,wan:Option<Wan>,now:u64)->Result<(),Drop> {
  if let Some(w)=wan {w.validate(now)?;}
  let same=match (self.wan,wan) {(Some(mut a),Some(mut b))=>{a.expires=0;b.expires=0;a==b},(None,None)=>true,_=>false};
  if !same {self.generation=self.generation.wrapping_add(1);for f in &mut self.flows {if f.used {f.used=false;f.closed=now.saturating_add(reuse_delay(self.shared_lan,f.proto));}}}
  self.wan=wan;Ok(())
 }
 pub fn expire(&mut self,now:u64) {
  self.as_of=now;
  if self.wan.is_some_and(|w|w.expires<=now) {let _=self.configure(None,now);}
  self.next_expiry=u64::MAX;
  self.expiry_cursor=0;
  self.expire_range(0,self.limit(),now);
 }
 fn expire_range(&mut self, start:usize, end:usize, now:u64) {
  for f in &mut self.flows[start..end] {
   if !f.used {continue;}
   let deadline=flow_deadline(f,self.shared_lan);
   if deadline<=now {
    f.used=false;
    f.closed=if self.shared_lan {now} else {now.saturating_add(120_000)};
   } else {self.next_expiry=self.next_expiry.min(deadline);}
  }
 }
 pub fn reset_lan(&mut self,port:Port,now:u64) {for f in &mut self.flows {if f.realm==port && f.used {f.used=false;f.closed=now.saturating_add(reuse_delay(self.shared_lan,f.proto));}}}
 pub fn track_releases(&mut self) {self.notify_releases=true;}
 pub fn take_releases(&mut self)->[u32;8] {core::mem::take(&mut self.releases)}
 pub fn release_client(&mut self,port:Port,client:Ip,now:u64) {
 if self.notify_releases && port==Port::Lan1 && (client^LAN[0])&0xffffff00==0 {
  let host=client as u8 as usize;
  self.releases[host/32]|=1<<(host%32);
 }
 for f in &mut self.flows {if f.realm==port && f.private==client && f.used {f.used=false;f.closed=now.saturating_add(reuse_delay(self.shared_lan,f.proto));}}}
 pub fn active(&self)->usize {self.flows[..self.limit()].iter().filter(|f|self.live(f,self.as_of)).count()}
 pub fn slot_usage(&self, now: u64) -> (u32, u32, u32) {
    let mut active = 0;
    let mut retired = 0;
    for flow in &self.flows[..self.limit()] {
        active += u32::from(self.live(flow, now));
        retired += u32::from(!flow.used && flow.closed > now);
    }
    (self.limit() as u32, active, retired)
 }
 pub fn active_endpoints(&self, now: u64) -> [u32; steering::BITMAP_WORDS] {
  let mut active=[0;steering::BITMAP_WORDS];
  for flow in self.flows[..self.limit()].iter().filter(|f|self.live(f,now)) {
   let bucket=steering::bucket(flow.private,flow.local,flow.proto);
   active[bucket/32] |= 1<<(bucket%32);
  }
  active
 }
 pub fn retains_endpoint(&self, address: Ip, local: u16, protocol: u8, now: u64) -> bool {
    self.flows[..self.limit()].iter().any(|flow| flow.private == address && flow.local == local
        && flow.proto == protocol && (self.live(flow, now) || (!flow.used && flow.closed > now)))
 }
 pub fn maintain(&mut self, now:u64) {
  self.as_of=now;
  if self.wan.is_some_and(|wan|wan.expires<=now) {
   let _=self.configure(None,now);
   self.next_expiry=u64::MAX;
   self.expiry_cursor=0;
   return;
  }
  if self.expiry_cursor==0 && now<self.next_expiry {return;}
  if !self.shared_lan {
   self.expire(now);
   return;
  }
  if self.expiry_cursor==0 {self.next_expiry=u64::MAX;}
  let limit=self.limit();
  let end=(self.expiry_cursor+8).min(limit);
  self.expire_range(self.expiry_cursor,end,now);
  self.expiry_cursor=if end==limit {0}else{end};
 }
 pub fn forward(&mut self,port:Port,packet:&mut[u8],now:u64,mtu:usize)->Result<Port,Drop> {
  self.forward_with_rx_checksum(port,packet,now,mtu,0)
 }
 /// Supply a transport protocol only from a trusted RX completion for this packet.
 pub fn forward_with_rx_checksum(&mut self,port:Port,packet:&mut[u8],now:u64,mtu:usize,verified_protocol:u32)->Result<Port,Drop> {
  self.maintain(now);
  self.forward_packet::<false>(port,packet,now,mtu,verified_protocol,&mut None)
 }
 pub fn forward_pair(&mut self, port: Port, packets: [&mut [u8]; 2], now: u64, mtu: usize,
                     verified: [u32; 2]) -> [Result<Port, Drop>; 2] {
  self.maintain(now);
  let [first, last] = packets;
  let mut slot = None;
  [self.forward_packet::<PAIR_LOOKUP>(port, first, now, mtu, verified[0], &mut slot),
   self.forward_packet::<PAIR_LOOKUP>(port, last, now, mtu, verified[1], &mut slot)]
 }
 pub fn forward_burst<'a>(&mut self, port: Port, packets: impl Iterator<Item = (&'a mut [u8], u32)>,
                          now: u64, mtu: usize, mut result: impl FnMut(Result<Port, Drop>)) {
  self.maintain(now);
  let mut slot = None;
  for (packet, verified) in packets {
   result(self.forward_packet::<PAIR_LOOKUP>(port, packet, now, mtu, verified, &mut slot));
  }
 }
 fn forward_packet<const PAIRED: bool>(&mut self,port:Port,packet:&mut[u8],now:u64,mtu:usize,verified_protocol:u32,
                                       slot: &mut Option<usize>)->Result<Port,Drop> {
  let result=self.process::<PAIRED>(port,packet,now,mtu,verified_protocol,slot);if result.is_err(){self.drops=self.drops.saturating_add(1);}else{self.forwarded=self.forwarded.saturating_add(1);}result
 }
 fn process<const PAIRED: bool>(&mut self,port:Port,p:&mut[u8],now:u64,mtu:usize,verified_protocol:u32,
                                slot: &mut Option<usize>)->Result<Port,Drop> {
  let w=self.wan.ok_or(Drop::NoWan)?;let len=validate_ip(p)?;
  let src=ipat(p,12);let dst=ipat(p,16);
  if !unicast(src) || !unicast(dst) {return Err(Drop::Spoof);}
  if port!=Port::Wan {
   let lan=LAN[port as usize-1];if src&0xffffff00!=lan&0xffffff00 || src&255==0 || src&255==255 || src==lan {return Err(Drop::Spoof);}
   if (dst&w.mask==w.address&w.mask && (dst&!w.mask==0 || dst&!w.mask==!w.mask)) || internal(dst) || dst==w.address {return Err(Drop::Policy);}
  } else if internal(src) || dst!=w.address {return Err(Drop::Spoof);}
  if p[8]<=1 {return Err(Drop::Ttl);}if len>mtu {return Err(Drop::Mtu);}
  let protocol=p[9];
  if port==Port::Wan && protocol==1 && len>=28 && [3,11,12].contains(&p[20]) {return self.related(p,len,now,w);}
  let minimum=match protocol {6=>40,17=>28,1=>28,_=>return Err(Drop::Unsupported)};
  if len<minimum {return Err(Drop::Malformed);}
  let zero_udp=protocol==17 && u16at(p,26)==0;
  if protocol==17 && u16at(p,24) as usize!=len-20 {return Err(Drop::Malformed);}
  if protocol==6 && ((p[32]>>4) as usize*4<20 || (p[32]>>4) as usize*4>len-20) {return Err(Drop::Malformed);}
  if protocol==1 {if checksum(&p[20..len])!=0 || p[21]!=0 || p[20]!=if port==Port::Wan {0}else{8} {return Err(Drop::Checksum);}}
  else if !zero_udp && verified_protocol!=u32::from(protocol) && transport_checksum(p,len)!=0 {return Err(Drop::Checksum);}
  let (local,remote_port)=if protocol==1 {(u16at(p,24),0)}else{(u16at(p,20),u16at(p,22))};
  let previous = if PAIRED && protocol == 6 { *slot } else { None };
  let index;
  if port==Port::Wan {
   let external=if protocol==1 {local}else{remote_port};
   let peer_port=if protocol==1 {0}else{local};
   let reused = previous.filter(|&i| {
    let flow = &self.flows[i];
    flow.proto == protocol && flow.external == external && flow.remote == src
        && flow.remote_port == peer_port && self.live(flow, now)
   });
   index=reused.or_else(|| self.reverse(protocol,external,src,peer_port,now)).ok_or(Drop::Policy)?;
  } else {
   if protocol!=1 && (local==0 || remote_port==0) {return Err(Drop::Malformed);}
   let matches = |flow: &Flow| self.live(flow, now) && flow.realm == port && flow.proto == protocol
       && flow.private == src && flow.local == local && flow.remote == dst && flow.remote_port == remote_port;
   let reused = previous.filter(|&i| matches(&self.flows[i]));
   if let Some(i)=reused.or_else(|| self.find(OUTBOUND, outbound(port,protocol,src,local,dst,remote_port), matches)) {index=i;}
   else {
    if protocol==6 && (p[33]&0x17!=2 || len!=20+(p[32]>>4) as usize*4) {return Err(Drop::State);}
    if !self.shared_lan && self.flows[..self.limit()].iter().filter(|f| f.used && f.realm==port && f.private==src).count()>=16 {
     return Err(Drop::Full);
    }
    index=self.alloc_slot(now)
        .ok_or(if self.shared_lan {Drop::NatSlots} else {Drop::Full})?;
    let shared=self.find(ENDPOINT, endpoint(port,protocol,src,local), |f| self.live(f,now) && f.realm==port && f.proto==protocol && f.private==src && f.local==local).map(|slot| self.flows[slot].external);
    let external=if let Some(v)=shared {v}else{self.alloc_port(protocol,now)?};
    let flow=Flow {used:true,realm:port,proto:protocol,private:src,local,external,remote:dst,remote_port,last:now,created:now,local_seq:if protocol==6 {ipat(p,24)}else{0},..EMPTY};
    if HASH_NAT {self.index.replace(index, &self.flows[index], &flow);}
    self.flows[index]=flow;
   }
  }
  let f=&mut self.flows[index];
  if protocol==6 {
   let seq=ipat(p,24);let ack=ipat(p,28);let flags=p[33];let data=len-20-(p[32]>>4) as usize*4;
   if f.stage==0 {
    if port==Port::Wan {
     if ack!=f.local_seq.wrapping_add(1) {return Err(Drop::State);}
     if flags&0x17==0x14 {f.stage=3;f.closed=now.saturating_add(reuse_delay(self.shared_lan,6));}
     else if flags&0x17==0x12 {f.remote_seq=seq;f.local_next=f.local_seq.wrapping_add(1);f.remote_next=seq.wrapping_add(1);f.stage=1;}
     else {return Err(Drop::State);}
    } else if seq!=f.local_seq || flags&0x17!=2 {return Err(Drop::State);}
   } else if f.stage==1 {
    if port==Port::Wan {if flags&0x17!=0x12 || seq!=f.remote_seq || ack!=f.local_seq.wrapping_add(1) {return Err(Drop::State);}}
    else {if flags&0x12!=0x10 || seq!=f.local_next || ack!=f.remote_next {return Err(Drop::State);}f.stage=2;}
   } else {
    if flags&0x14==0 || flags&2!=0 {return Err(Drop::State);}
    let (next,other)=if port==Port::Wan {(f.remote_next,f.local_next)}else{(f.local_next,f.remote_next)};
    if seq.wrapping_sub(next) as i32>1048576 || (seq.wrapping_sub(next) as i32)< -1048576 || (flags&0x10!=0 && ((ack.wrapping_sub(other) as i32)>0 || (ack.wrapping_sub(other) as i32)< -1048576)) {return Err(Drop::State);}
   }
   if f.stage>=2 {
    let next=seq.wrapping_add(data as u32).wrapping_add(u32::from(flags&1!=0));
    let target=if port==Port::Wan {&mut f.remote_next}else{&mut f.local_next};
    if (next.wrapping_sub(*target) as i32)>0 {*target=next;}
    if flags&5!=0 && f.stage!=3 {f.stage=3;f.closed=now.saturating_add(reuse_delay(self.shared_lan,6));}
   }
   if self.shared_lan && f.stage==3 {
    f.closed=if flags&4!=0 {now} else {now.saturating_add(SHARED_TCP_CLOSE_MS)};
   }
  }
  f.last=now;
  self.next_expiry=self.next_expiry.min(flow_deadline(f,self.shared_lan));
  let (ip_offset, transport_offset, old_ip, new_ip, old_id, new_id, egress) =
   if port==Port::Wan {
    (16, if protocol==1 {24}else{22}, dst, f.private, f.external, f.local, f.realm)
   } else {
    (12, if protocol==1 {24}else{20}, src, w.address, f.local, f.external, Port::Wan)
   };
  let old_ttl_protocol=u16at(p,8);
  let mut ip_checksum=adjust_address(u16at(p,10),old_ip,new_ip);
  p[8]-=1;
  ip_checksum=adjust(ip_checksum,old_ttl_protocol,u16at(p,8));
  set16(p,10,ip_checksum);
  setip(p,ip_offset,new_ip);
  set16(p,transport_offset,new_id);
  if !zero_udp {
   let offset=match protocol {6=>36,17=>26,_=>22};
   let mut updated=adjust(u16at(p,offset),old_id,new_id);
   if protocol!=1 {updated=adjust_address(updated,old_ip,new_ip);}
   set16(p,offset,if protocol==17 && updated==0 {0xffff}else{updated});
  }
  if PAIRED && protocol == 6 { *slot = Some(index); }
  Ok(egress)
 }
 fn related(&mut self,p:&mut[u8],len:usize,now:u64,w:Wan)->Result<Port,Drop> {
  if len<56 || checksum(&p[20..len])!=0 || (p[20]==11 && p[21]>1) || (p[20]==3 && p[21]>15) || (p[20]==12 && p[21]>2) {return Err(Drop::Malformed);}
  let q=&p[28..len];if q[0]!=0x45 || checksum(&q[..20])!=0 || ipat(q,12)!=w.address || u16at(q,6)&0xbfff!=0 {return Err(Drop::State);}
  let proto=q[9];let (external,remote_port)=match proto {6|17=>(u16at(q,20),u16at(q,22)),1 if q[20]==8=>(u16at(q,24),0),_=>return Err(Drop::Unsupported)};
  let remote=ipat(q,16);let f=&self.flows[self.reverse(proto,external,remote,remote_port,now).ok_or(Drop::Policy)?];
  let realm=f.realm;let private=f.private;let local=f.local;
  let q=&mut p[28..len];
  if u16at(q,2)<if proto==6 {40}else{28} {return Err(Drop::Malformed);}
  if proto==17 && u16at(q,24) as usize!=u16at(q,2) as usize-20 {return Err(Drop::Malformed);}
  if proto==6 && ((ipat(q,24).wrapping_sub(f.local_next) as i32)>1048576 || (ipat(q,24).wrapping_sub(f.local_next) as i32)< -1048576) {return Err(Drop::State);}
  let checksum_offset=match proto {6=>36,17=>26,_=>22};
  if q.len()>=checksum_offset+2 {
   let old=u16at(q,checksum_offset);
   if proto!=17 || old!=0 {
    let mut value=old;
    if proto!=1 {value=adjust(value,(w.address>>16) as u16,(private>>16) as u16);value=adjust(value,w.address as u16,private as u16);}
    value=adjust(value,external,local);set16(q,checksum_offset,if proto==17 && value==0 {0xffff}else{value});
   }
  }
  setip(q,12,private);set16(q,if proto==1 {24}else{20},local);set16(q,10,0);set16(q,10,checksum(&q[..20]));
  setip(p,16,private);repair(p,len,false);Ok(realm)

 }
}
fn repair(p:&mut[u8],len:usize,zero_udp:bool) {
 p[8]-=1;set16(p,10,0);set16(p,10,checksum(&p[..20]));
 let offset=match p[9] {6=>36,17=>26,_=>22};set16(p,offset,0);
 if p[9]==1 {set16(p,offset,checksum(&p[20..len]));}
 else if !zero_udp {let value=transport_checksum(p,len);set16(p,offset,if value==0 {0xffff}else{value});}
}

fn adjust_address(check:u16,old:Ip,new:Ip)->u16 {
 adjust(adjust(check,(old>>16) as u16,(new>>16) as u16),old as u16,new as u16)
}

fn adjust(check:u16,old:u16,new:u16)->u16 {let mut sum=(!check as u32)+(!old as u32)+(new as u32);while sum>>16!=0 {sum=(sum&65535)+(sum>>16);}!(sum as u16)}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_tcp_lookup_checks_the_live_directional_tuple() {
        if !HASH_NAT { return; }
        let mut router = Router::sharded(1, 0);
        let wan = Wan { address: 0x0a000132, mask: 0xffffff00, gateway: 0x0a000101,
            dns: [0x0a000101, 0, 0], dns_count: 1, expires: 10_000_000 };
        router.configure(Some(wan), 0).unwrap();
        let private = LAN[0] + 2;
        let remote = 0x08080808;
        router.flows[0] = Flow { used: true, realm: Port::Lan1, proto: 6, private, remote,
            local: 1234, external: 49152, remote_port: 443, stage: 2, ..EMPTY };
        let tcp = |src, dst, sport, dport| {
            let mut ip = [0; 40];
            ip[0] = 0x45;
            ip[8..10].copy_from_slice(&[64, 6]);
            ip[32..34].copy_from_slice(&[0x50, 0x10]);
            set16(&mut ip, 2, 40);
            setip(&mut ip, 12, src);
            setip(&mut ip, 16, dst);
            set16(&mut ip, 20, sport);
            set16(&mut ip, 22, dport);
            let sum = checksum(&ip[..20]);
            set16(&mut ip, 10, sum);
            let sum = transport_checksum(&ip, ip.len());
            set16(&mut ip, 36, sum);
            ip
        };
        // The fixture has no index entry, so only a validated cached slot can succeed.
        let mut outbound = tcp(private, remote, 1234, 443);
        assert_eq!(router.forward_packet::<false>(Port::Lan1, &mut outbound, 1, 1500, 0, &mut None),
                   Err(Drop::State));
        assert_eq!(router.forward_packet::<true>(Port::Lan1, &mut outbound, 1, 1500, 0, &mut Some(0)),
                   Ok(Port::Wan));
        let mut inbound = tcp(remote, wan.address, 443, 49152);
        assert_eq!(router.forward_packet::<true>(Port::Wan, &mut inbound, 2, 1500, 0, &mut Some(0)),
                   Ok(Port::Lan1));
        assert_eq!(ipat(&inbound, 16), private);
        let mut wrong_peer = tcp(remote + 1, wan.address, 443, 49152);
        assert_eq!(router.forward_packet::<true>(Port::Wan, &mut wrong_peer, 3, 1500, 0, &mut Some(0)),
                   Err(Drop::Policy));
        let mut expired = tcp(remote, wan.address, 443, 49152);
        assert_eq!(router.forward_packet::<true>(
            Port::Wan, &mut expired, SHARED_TCP_IDLE_MS + 2, 1500, 0, &mut Some(0)), Err(Drop::Policy));
    }

    #[test]
    fn shared_lan_expires_flows_at_genode_deadlines() {
        for (proto, stage, timeout) in [(1, 0, 10_000), (17, 0, 30_000), (6, 0, 600_000),
                                         (6, 2, 600_000), (6, 3, 30_000)] {
            let mut router = Router::new(1);
            router.enable_shared_lan();
            router.flows[0] = Flow { used: true, proto, stage, closed: timeout, ..EMPTY };
            router.expire(timeout - 1);
            assert_eq!(router.active(), 1);
            router.expire(timeout);
            assert_eq!(router.slot_usage(timeout), (CAPACITY as u32, 0, 0));
        }
    }

    #[test]
    fn slot_allocation_cursor_skips_busy_slots_and_respects_reuse_delay() {
        let mut router = Router::new(1);
        // Occupy the first slots so the cursor has to walk past them.
        for slot in 0..3 {
            router.flows[slot] = Flow { used: true, proto: 17, ..EMPTY };
        }
        assert_eq!(router.alloc_slot(0), Some(3));
        // Slot 1 is freed but still inside its reuse delay; with every other
        // slot busy the scan must refuse it until the delay passes.
        router.flows[3] = Flow { used: true, proto: 17, ..EMPTY };
        router.flows[1] = Flow { used: false, closed: 100, ..EMPTY };
        for slot in 4..CAPACITY {
            router.flows[slot] = Flow { used: true, proto: 17, ..EMPTY };
        }
        assert_eq!(router.alloc_slot(50), None);
        assert_eq!(router.alloc_slot(100), Some(1));
    }

    #[test]
    fn port_allocation_uses_the_shard_lane_and_refuses_retired_ports() {
        let mut worker = Router::sharded(7, 1); // shard 2 -> lane 1, stride 4
        let first = worker.alloc_port(17, 0).unwrap();
        assert_eq!((first as u32 - 49152) % 4, 1);
        let second = worker.alloc_port(17, 0).unwrap();
        assert_eq!((second as u32 - 49152) % 4, 1);
        assert_ne!(first, second);
        // Claim the next lane port as a retired mapping (closed in the future),
        // recorded through the index exactly like the forwarding path does.
        let retired = (second as u32 + 4) as u16;
        let retired_flow = Flow { used: false, proto: 17, external: retired, closed: 1000, ..EMPTY };
        worker.index.replace(0, &EMPTY, &retired_flow);
        worker.flows[0] = retired_flow;
        // The cursor alone would offer `retired` next; the reservation check
        // must refuse it until the reuse delay passes.
        let third = worker.alloc_port(17, 500).unwrap();
        assert_ne!(third, retired);
        // Restart the probe at the retired port's lane slot: once the reuse
        // delay has passed the reservation check must accept it again.
        worker.port_cursor = (retired as u32 - 49152 - 1) / 4;
        assert_eq!(worker.alloc_port(17, 1000), Ok(retired));
    }

    #[test]
    fn port_allocation_lanes_are_disjoint_across_workers() {
        let mut lanes = [0u32; 4];
        for worker in 0..4u8 {
            let mut router = Router::sharded(3, worker);
            router.enable_shared_lan();
            let port = router.alloc_port(6, 0).unwrap() as u32;
            lanes[worker as usize] = (port - 49152) % 4;
        }
        lanes.sort_unstable();
        assert_eq!(lanes, [0, 1, 2, 3]);
    }
}
