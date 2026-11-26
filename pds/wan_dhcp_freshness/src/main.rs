#![no_std]
#![no_main]
#[cfg(feature="b51-udp")]
mod b51_udp;
#[cfg(all(feature="b51-udp", not(feature="lan-status")))]
fn b51_snapshot() -> Option<[u32; b51_udp::WORDS]> {
    let word = |index: usize| unsafe { &*((0x41000000 as *const AtomicU32).add(index)) };
    let sequence = word(256).load(Ordering::Acquire);
    if sequence == 0 { return Some([0; b51_udp::WORDS]); }
    if sequence & 1 != 0 { return None; }
    let snapshot = core::array::from_fn(|i| word(257 + i).load(Ordering::Relaxed));
    core::sync::atomic::fence(Ordering::Acquire);
    (word(256).load(Ordering::Relaxed) == sequence).then_some(snapshot)
}
#[cfg(feature="lan-status")]
include!("wan_status_profile.rs");
#[cfg(feature="lan-status")]
mod network_snapshot;

#[cfg(feature="lan-status")]
fn b51_snapshot() -> Option<[u32; b51_udp::WORDS]> {
    network_snapshot::read(PORT_MASK, LOGICAL)
}
mod progress;mod queue;mod shared;mod device;mod config;mod link;use link::observed_link;
#[cfg(feature = "zc")]
mod zc;
use core::sync::atomic::{AtomicU32,Ordering};
use sel4_microkit::{protection_domain,Handler,Infallible};
use smoltcp::{iface::{Config,Interface,SocketSet,SocketStorage},socket::{dhcpv4,icmp},time::Instant,wire::{EthernetAddress,IpCidr,Ipv4Cidr,Ipv4Address,IpAddress,DhcpRepr}};
use r5s_router_core::{*,arp::{Acd,Action}};
fn word(offset:usize)->&'static AtomicU32 {unsafe {&*((0x40004000+offset) as *const AtomicU32)}}
fn interface(device:&mut device::Net,now:u64,seed:u64)->Interface {let mut config=Config::new(EthernetAddress(device::MAC).into());config.random_seed=seed;Interface::new(config,device,Instant::from_millis(now as i64))}
fn echo_request(sequence: u16, cookie: u64) -> [u8; 24] {
    let mut packet = [0; 24];
    packet[0] = 8;
    set16(&mut packet, 4, 0x5242);
    set16(&mut packet, 6, sequence);
    packet[8..16].copy_from_slice(&cookie.to_be_bytes());
    packet[16..24].copy_from_slice(&u64::from(sequence).to_be_bytes());
    let sum = checksum(&packet);
    set16(&mut packet, 2, sum);
    packet
}
#[protection_domain]
fn init()->impl Handler {
 while unsafe {&*(0x40001008 as *const AtomicU32)}.load(Ordering::Acquire)!=1 {core::hint::spin_loop();}
 let boot=shared::millis();let ticks:u64;unsafe {core::arch::asm!("isb","mrs {},cntvct_el0",out(reg)ticks,options(nostack));}let seed=ticks.rotate_left(23)^0x52533557414e4450;
 let mut net=device::Net::new();let mut iface=interface(&mut net,boot,seed);
 let mut dhcp_packet=[0u8;1500];let mut dhcp=dhcpv4::Socket::new();dhcp.set_receive_packet_buffer(&mut dhcp_packet);
 let mut rx_meta=[icmp::PacketMetadata::EMPTY;4];let mut rx_data=[0;512];let mut tx_meta=[icmp::PacketMetadata::EMPTY;4];let mut tx_data=[0;512];
 let mut ping=icmp::Socket::new(icmp::PacketBuffer::new(&mut rx_meta[..],&mut rx_data[..]),icmp::PacketBuffer::new(&mut tx_meta[..],&mut tx_data[..]));ping.bind(icmp::Endpoint::Ident(0x5242)).unwrap();
 let mut storage=[SocketStorage::EMPTY,SocketStorage::EMPTY];let mut sockets=SocketSet::new(&mut storage[..]);let dhcp_handle=sockets.add(dhcp);let ping_handle=sockets.add(ping);
 // The new socket announces its initial unconfigured state once.
 let _ = sockets.get_mut::<dhcpv4::Socket>(dhcp_handle).poll();
 let mut active:Option<Wan>=None;let mut candidate:Option<Wan>=None;let mut cidr:Option<Ipv4Cidr>=None;let mut acd:Option<Acd>=None;
 let mut cfg_req=0u32;let mut awaiting=false;let mut old_link=false;let mut old_epoch=0;let mut backoff=0;let mut last_status=u64::MAX;let mut reject=0;
 #[cfg(feature="lan-status")] let mut dhcp_history = (0, 0);
 #[cfg(feature="shared-wan")] let mut internet_sent = 0u16;
 #[cfg(feature="shared-wan")] let mut internet_replies = 0u8;
 #[cfg(feature="shared-wan")] let mut internet_at = 0u64;
 let mut acquisition=None;let mut bound_at=0;let mut arp_at=0;let mut ping_at=0;let mut ping_seq=0u16;let mut replies=0u32;let mut server=0;let mut decline_pending=None;let mut ping_mask=0u8;
 loop {
  let now=shared::millis();let status=shared::read::<20>(0x40002000);
  let phy=observed_link(status.as_ref(),old_link);let epoch=status.map_or(old_epoch,|v|v[11]);
  let reset=(!phy && old_link) || (!awaiting && epoch!=old_epoch && (active.is_some() || candidate.is_some())) || active.is_some_and(|w|now>=w.expires) || candidate.is_some_and(|w|now>=w.expires) || net.conflict;
  if reset {
   net.progress=Default::default();acquisition=None;replies=0;ping_seq=0;ping_mask=0;
   let conflict=net.conflict;
   if conflict {if let Some(value)=active.or(candidate) {decline_pending=Some((value.address,server));}}

   active=None;candidate=None;acd=None;cidr=None;net.active=0;net.candidate=0;net.conflict=false;net.gateway_seen=0;
   iface=interface(&mut net,now,seed^now);sockets.get_mut::<dhcpv4::Socket>(dhcp_handle).reset();cfg_req=cfg_req.wrapping_add(1);awaiting=true;backoff=if conflict {now}else{now+10000};
  }
  old_link=phy;old_epoch=epoch;net.epoch=epoch;
  let ack=status.is_some_and(|v|v[19]==cfg_req);if awaiting && ack {awaiting=false;}
  net.allow=phy && ack && status.is_some_and(|v|v[14]==1) && now>=backoff;
  if net.allow && acquisition.is_none() {acquisition=Some(now);}
  #[cfg(feature="b51-udp")]
  { net.bound_mask = active.map_or(0, |lease| lease.mask); }
  #[cfg(feature="lan-status")]
  net.update_fallback(now, FALLBACK, phy, acquisition);
  net.remaining=queue::batch(option_env!("R5S_DATAPATH_BATCH"), 8) as usize;
  if net.allow {
   if let Some((address,server))=decline_pending.take() {let mut frame=[0;342];if let Ok(length)=dhcp::decline(device::MAC,address,server,(seed^now) as u32,&mut frame) {net.send(&frame[..length]);}backoff=now+10000;net.allow=false;}
  }
  if net.allow {iface.poll(Instant::from_millis(now as i64),&mut net,&mut sockets);}
  #[cfg(feature="lan-status")]
  {
   if dhcp_history.0 != epoch { dhcp_history = (epoch, 0); }
   dhcp_history.1 |= net.progress.bits;
  }
  let mut configured=None;let mut lost=false;
  match sockets.get_mut::<dhcpv4::Socket>(dhcp_handle).poll() {
   Some(dhcpv4::Event::Configured(config))=>{
    let duration=config.packet.as_ref().and_then(|p|DhcpRepr::parse(p).ok()).and_then(|p|p.lease_duration).unwrap_or(0);
    let mut dns=[0;3];for (i,ip) in config.dns_servers.iter().take(3).enumerate() {dns[i]=ip.to_bits();}
    if crate::config::DNS_COUNT!=0 {dns=crate::config::DNS;}
    let prefix=config.address.prefix_len();let mask=if prefix==0 {0}else{u32::MAX<<(32-prefix)};
    let value=Wan {address:config.address.address().to_bits(),mask,gateway:config.router.map_or(0,|v|v.to_bits()),dns,dns_count:if crate::config::DNS_COUNT!=0 {crate::config::DNS_COUNT}else{config.dns_servers.len().min(3)},expires:now.saturating_add(duration as u64*1000)};
    if value.validate(now).is_ok() && unicast(config.server.identifier.to_bits()) && unicast(config.server.address.to_bits()) && duration>=4 && duration<=604800 {configured=Some((value,config.address,config.server.identifier.to_bits()));}else {reject=1;lost=true;}
   }
   Some(dhcpv4::Event::Deconfigured)=>lost=true,None=>{}
  }
  if lost {
   net.progress=Default::default();acquisition=None;replies=0;ping_seq=0;ping_mask=0;
   active=None;candidate=None;acd=None;net.active=0;net.candidate=0;net.gateway_seen=0;iface=interface(&mut net,now,seed^now);cfg_req=cfg_req.wrapping_add(1);awaiting=true;backoff=now+10000;sockets.get_mut::<dhcpv4::Socket>(dhcp_handle).reset();
  }
  if let Some((value,address,server_id))=configured {
   let same=active.is_some_and(|mut old|{old.expires=value.expires;old==value});
   if same {active=Some(value);}else {
    active=None;candidate=Some(value);cidr=Some(address);acd=Some(Acd::new(value.address,now,seed^now));server=server_id;
    net.candidate=value.address;net.active=0;net.gateway=0;net.gateway_seen=0;iface=interface(&mut net,now,seed^now);cfg_req=cfg_req.wrapping_add(1);awaiting=true;
    #[cfg(not(feature="quiet"))] sel4_microkit::debug_println!("WAN candidate={:#x} gateway={:#x} DNS={:#x} lease_s={}",value.address,value.gateway,value.dns[0],value.expires.saturating_sub(now)/1000);
   }
  }
  if !awaiting && net.allow {
   if let Some(probe)=acd.as_mut() {
    match probe.tick(now) {
     Action::Probe(ip)|Action::Announce(ip)=>{let mut frame=[0;60];let source=if active.is_some(){ip}else{0};arp::request(device::MAC,source,ip,&mut frame).unwrap();net.send(&frame);}
     Action::Bind(ip)=>{
      if let (Some(value),Some(address))=(candidate.take(),cidr) {
       iface.update_ip_addrs(|list|{list.clear();list.push(IpCidr::Ipv4(address)).unwrap();});iface.routes_mut().add_default_ipv4_route(Ipv4Address::from_bits(value.gateway)).unwrap();
       active=Some(value);net.active=ip;net.candidate=0;net.gateway=value.gateway;ping_seq=0;ping_mask=0;replies=0;arp_at=now;ping_at=now;bound_at=now;
       #[cfg(feature="shared-wan")]
       {
        internet_sent = 0;
        internet_replies = 0;
        internet_at = now;
        net.internet_tx_mask = 0;
        net.internet_cookie = seed ^ now ^ u64::from(cfg_req);
       }
      }
     },Action::None=>{}
    }
   }
   if let Some(value)=active {
    if now>=arp_at {let mut frame=[0;60];arp::request(device::MAC,value.address,value.gateway,&mut frame).unwrap();net.send(&frame);arp_at=now+5000;}
    if net.gateway_seen!=0 && ping_seq<5 && now>=ping_at {
     let packet=echo_request(ping_seq, seed ^ u64::from(cfg_req));
     if sockets.get_mut::<icmp::Socket>(ping_handle).send_slice(&packet,IpAddress::Ipv4(Ipv4Address::from_bits(value.gateway))).is_ok() {ping_seq+=1;ping_at=now+1000;}
    }
    #[cfg(feature="shared-wan")]
    if net.gateway_seen != 0 && internet_sent < device::INTERNET_LIMIT && now >= internet_at {
     let packet = echo_request(device::INTERNET_SEQ + internet_sent, net.internet_cookie);
     let target = IpAddress::Ipv4(Ipv4Address::from_bits(device::INTERNET_TARGET));
     if sockets.get_mut::<icmp::Socket>(ping_handle).send_slice(&packet, target).is_ok() {
      internet_sent += 1;
      internet_at = now.saturating_add(1000);
     }
    }
   }
  }
  if let Ok((data,address))=sockets.get_mut::<icmp::Socket>(ping_handle).recv() {
   if data.len()==24 && data[0]==0 && data[1]==0 && checksum(data)==0 && u16at(data,4)==0x5242 && u16at(data,6)<ping_seq && ping_mask&(1<<u16at(data,6))==0 && data[8..16]==(seed^cfg_req as u64).to_be_bytes() && data[16..24]==(u16at(data,6) as u64).to_be_bytes() && active.is_some_and(|w|address==IpAddress::Ipv4(Ipv4Address::from_bits(w.gateway))) {ping_mask|=1<<u16at(data,6);replies=replies.saturating_add(1);#[cfg(not(feature="quiet"))] sel4_microkit::debug_println!("WAN gateway IPv4 reply count={}",replies);}
   #[cfg(feature="shared-wan")]
   if data.len() == 24 && active.is_some() && now < internet_at.saturating_add(4000)
       && address == IpAddress::Ipv4(Ipv4Address::from_bits(device::INTERNET_TARGET))
   {
    let index = u16at(data, 6).wrapping_sub(device::INTERNET_SEQ);
    if index < internet_sent && net.internet_tx_mask & (1 << index) != 0 {
     internet_replies |= 1 << index;
    }
   }
  }
  #[cfg(feature="shared-wan")]
  if net.allow && active.is_some() && net.active != 0 {
   for _ in 0..queue::batch(option_env!("R5S_DATAPATH_BATCH"), 4) {
    match queue::producer_has_space(queue::BASE + queue::DIRECTION) {
     Ok(true) => {}
     Ok(false) => break,
     Err(()) => { net.fault = true; break; }
    }
    #[cfg(feature = "zc")]
    {
     let input = zc::link_queue(zc::LINKS_VA, zc::Q_WAN_EG);
     let output = zc::link_queue(zc::LINKS_VA, zc::Q_APP_TX);
     if !zc::RefQueue::depth(output).is_ok_and(|depth| depth < zc::RQ_COUNT) { break; }
     let Ok(Some(r)) = zc::RefQueue::pop(input) else { break };
     if r.flags & zc::FLAG_COPY == 0 {
      if r.epoch != net.epoch {
       let _ = zc::reject(zc::META_VA, &r);
       continue;
      }
      let length = r.len as usize;
      if !(14..=zc::BUFFER).contains(&length) {
       let _ = zc::reject(zc::META_VA, &r);
       continue;
      }
      let buffer = unsafe {
       core::slice::from_raw_parts((zc::POOL_VA + r.id() as usize * zc::BUFFER) as *const u8, zc::BUFFER)
      };
      let frame = &buffer[..length];
      net.progress.observe(frame, true, device::MAC);
      if length >= 42 && frame[12..14]==[8,0] && frame[14]==0x45 && frame[23]==1
          && frame[34]==8 && ipat(&frame,30)==0x08080808 {
       word(128).fetch_add(1,Ordering::AcqRel);
       word(132).fetch_add(1,Ordering::Relaxed);
       word(136).store(ipat(&frame,38),Ordering::Relaxed);
       word(140).store(1,Ordering::Relaxed);
       word(128).fetch_add(1,Ordering::Release);
      }
      if zc::RefQueue::push(zc::link_queue(zc::LINKS_VA, zc::Q_APP_TX), &r) != Ok(true) {
       zc::count(zc::META_VA, zc::C_FALLBACK_QUEUE);
       let _ = zc::reject(zc::META_VA, &r);
      }
      continue;
     }
    }
    let result=unsafe {queue::consume_in_place(0x64000000 + queue::DIRECTION, |frame,tag| {
      if tag!=net.epoch {return false;}
      let length=frame.len();
      #[cfg(feature="zc")]
      let queued=net.send_forwarded(frame);
      #[cfg(not(feature="zc"))]
      let queued=net.send(frame);
      if length >= 42 && frame[12..14]==[8,0] && frame[14]==0x45 && frame[23]==1
          && frame[34]==8 && ipat(&frame,30)==0x08080808 {
       word(128).fetch_add(1,Ordering::AcqRel);
       word(132).fetch_add(1,Ordering::Relaxed);
       word(136).store(ipat(&frame,38),Ordering::Relaxed);
       word(140).store(u32::from(queued),Ordering::Relaxed);
       word(128).fetch_add(1,Ordering::Release);
      }
      true
    })};
    match result {
     Ok(Some(true)) => {}
     Ok(_) => break,
     Err(()) => { net.fault = true; break; }
    }
   }
  }
  if now!=last_status {
   last_status=now;let config=active.or(candidate);let ready=active.is_some() && net.gateway_seen!=0 && now.saturating_sub(net.gateway_seen)<=60000 && net.allow;
   let mut progress=net.progress.bits;
   if active.is_some() {progress|=8;}
   if ready {progress|=16;}
   if ready && replies!=0 {progress|=32;}
   if active.is_none() && acquisition.is_some_and(|t|now.saturating_sub(t)>=45000) {progress|=64;}
   if active.is_some() && replies==0 && now.saturating_sub(bound_at)>=20000 {progress|=128;}
   word(0).fetch_add(1,Ordering::AcqRel);
   word(96).store(progress & 0xff,Ordering::Relaxed);
   #[cfg(feature="lan-status")]
   {
    word(100).store(net.fallback_active, Ordering::Relaxed);
    word(104).store(dhcp_history.1, Ordering::Relaxed);
   }
   #[cfg(feature="shared-wan")]
   {
    let counts = if active.is_some() {
     [u32::from(internet_sent), net.internet_tx_mask.count_ones(), internet_replies.count_ones(),
      u32::from(internet_sent == device::INTERNET_LIMIT && now >= internet_at.saturating_add(4000))]
    } else { [0; 4] };
    word(108).store(1, Ordering::Relaxed);
    for (i, count) in counts.into_iter().enumerate() { word(112 + i * 4).store(count, Ordering::Relaxed); }
   }
   for (offset,value) in [(4,1),(8,now as u32),(12,0),(16,0),(20,epoch),(24,0),(28,u32::from(net.fault)),(32,cfg_req),(36,u32::from(ready)),(40,config.map_or(0,|w|w.address)),(44,config.map_or(0,|w|w.mask)),(48,config.map_or(0,|w|w.gateway)),(52,config.map_or(0,|w|w.dns[0])),(56,config.map_or(0,|w|w.dns[1])),(60,config.map_or(0,|w|w.dns[2])),(64,config.map_or(0,|w|w.dns_count as u32)),(68,config.map_or(0,|w|w.expires.saturating_sub(now) as u32)),(72,net.gateway_seen as u32),(76,replies),(80,net.tx_drops),(84,net.rx_drops),(88,reject),(92,server)] {word(offset).store(value,Ordering::Relaxed);}
   word(0).fetch_add(1,Ordering::Release);
  }
  #[cfg(feature="zc")]
  zc::flush_counts(zc::META_VA);
  core::hint::spin_loop();
 }
 #[allow(unreachable_code)] Idle
}
struct Idle;impl Handler for Idle {type Error=Infallible;}
