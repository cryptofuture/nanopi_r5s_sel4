#![no_std]
#![no_main]
mod queue;mod shared;mod device;mod config;
use core::sync::atomic::{AtomicU32,Ordering};
use sel4_microkit::{protection_domain,Handler,Infallible};
use smoltcp::{iface::{Config,Interface,SocketSet,SocketStorage},socket::{dhcpv4,icmp},time::Instant,wire::{EthernetAddress,IpCidr,Ipv4Cidr,Ipv4Address,IpAddress,DhcpRepr}};
use r5s_router_core::{*,arp::{Acd,Action}};
fn word(offset:usize)->&'static AtomicU32 {unsafe {&*((0x40004000+offset) as *const AtomicU32)}}
fn interface(device:&mut device::Net,now:u64,seed:u64)->Interface {let mut config=Config::new(EthernetAddress(device::MAC).into());config.random_seed=seed;Interface::new(config,device,Instant::from_millis(now as i64))}
#[protection_domain]
fn init()->impl Handler {
 while unsafe {&*(0x40001008 as *const AtomicU32)}.load(Ordering::Acquire)!=1 {core::hint::spin_loop();}
 let boot=shared::millis();let ticks:u64;unsafe {core::arch::asm!("isb","mrs {},cntvct_el0",out(reg)ticks,options(nostack));}let seed=ticks.rotate_left(23)^0x52533557414e4450;
 let mut net=device::Net::new();let mut iface=interface(&mut net,boot,seed);
 let mut dhcp_packet=[0u8;1500];let mut dhcp=dhcpv4::Socket::new();dhcp.set_receive_packet_buffer(&mut dhcp_packet);
 let mut rx_meta=[icmp::PacketMetadata::EMPTY;4];let mut rx_data=[0;512];let mut tx_meta=[icmp::PacketMetadata::EMPTY;4];let mut tx_data=[0;512];
 let mut ping=icmp::Socket::new(icmp::PacketBuffer::new(&mut rx_meta[..],&mut rx_data[..]),icmp::PacketBuffer::new(&mut tx_meta[..],&mut tx_data[..]));ping.bind(icmp::Endpoint::Ident(0x5242)).unwrap();
 let mut storage=[SocketStorage::EMPTY,SocketStorage::EMPTY];let mut sockets=SocketSet::new(&mut storage[..]);let dhcp_handle=sockets.add(dhcp);let ping_handle=sockets.add(ping);
 let mut active:Option<Wan>=None;let mut candidate:Option<Wan>=None;let mut cidr:Option<Ipv4Cidr>=None;let mut acd:Option<Acd>=None;
 let mut cfg_req=0u32;let mut awaiting=false;let mut old_link=false;let mut old_epoch=0;let mut backoff=0;let mut last_status=u64::MAX;let mut reject=0;
 let mut arp_at=0;let mut ping_at=0;let mut ping_seq=0u16;let mut replies=0u32;let mut server=0;let mut decline_pending=None;let mut ping_mask=0u8;
 loop {
  let now=shared::millis();let status=shared::read::<20>(0x40002000);
  let phy=status.is_some_and(|v|v[2]==2);let epoch=status.map_or(old_epoch,|v|v[11]);
  let reset=(!phy && old_link) || (!awaiting && epoch!=old_epoch && (active.is_some() || candidate.is_some())) || active.is_some_and(|w|now>=w.expires) || candidate.is_some_and(|w|now>=w.expires) || net.conflict;
  if reset {
   let conflict=net.conflict;
   if conflict {if let Some(value)=active.or(candidate) {decline_pending=Some((value.address,server));}}

   active=None;candidate=None;acd=None;cidr=None;net.active=0;net.candidate=0;net.conflict=false;net.gateway_seen=0;
   iface=interface(&mut net,now,seed^now);sockets.get_mut::<dhcpv4::Socket>(dhcp_handle).reset();cfg_req=cfg_req.wrapping_add(1);awaiting=true;backoff=if conflict {now}else{now+10000};
  }
  old_link=phy;old_epoch=epoch;net.epoch=epoch;
  let ack=status.is_some_and(|v|v[19]==cfg_req);if awaiting && ack {awaiting=false;}
  net.allow=phy && ack && status.is_some_and(|v|v[14]==1) && now>=backoff;
  net.remaining=8;
  if net.allow {
   if let Some((address,server))=decline_pending.take() {let mut frame=[0;342];if let Ok(length)=dhcp::decline(device::MAC,address,server,(seed^now) as u32,&mut frame) {net.send(&frame[..length]);}backoff=now+10000;net.allow=false;}
  }
  if net.allow {iface.poll(Instant::from_millis(now as i64),&mut net,&mut sockets);}
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
   active=None;candidate=None;acd=None;net.active=0;net.candidate=0;net.gateway_seen=0;iface=interface(&mut net,now,seed^now);cfg_req=cfg_req.wrapping_add(1);awaiting=true;backoff=now+10000;sockets.get_mut::<dhcpv4::Socket>(dhcp_handle).reset();
  }
  if let Some((value,address,server_id))=configured {
   let same=active.is_some_and(|mut old|{old.expires=value.expires;old==value});
   if same {active=Some(value);}else {
    active=None;candidate=Some(value);cidr=Some(address);acd=Some(Acd::new(value.address,now,seed^now));server=server_id;
    net.candidate=value.address;net.active=0;net.gateway=0;net.gateway_seen=0;iface=interface(&mut net,now,seed^now);cfg_req=cfg_req.wrapping_add(1);awaiting=true;
    sel4_microkit::debug_println!("WAN candidate={:#x} gateway={:#x} DNS={:#x} lease_s={}",value.address,value.gateway,value.dns[0],value.expires.saturating_sub(now)/1000);
   }
  }
  if !awaiting && net.allow {
   if let Some(probe)=acd.as_mut() {
    match probe.tick(now) {
     Action::Probe(ip)|Action::Announce(ip)=>{let mut frame=[0;60];let source=if active.is_some(){ip}else{0};arp::request(device::MAC,source,ip,&mut frame).unwrap();net.send(&frame);}
     Action::Bind(ip)=>{
      if let (Some(value),Some(address))=(candidate.take(),cidr) {
       iface.update_ip_addrs(|list|{list.clear();list.push(IpCidr::Ipv4(address)).unwrap();});iface.routes_mut().add_default_ipv4_route(Ipv4Address::from_bits(value.gateway)).unwrap();
       active=Some(value);net.active=ip;net.candidate=0;net.gateway=value.gateway;ping_seq=0;ping_mask=0;replies=0;arp_at=now;ping_at=now;
      }
     },Action::None=>{}
    }
   }
   if let Some(value)=active {
    if now>=arp_at {let mut frame=[0;60];arp::request(device::MAC,value.address,value.gateway,&mut frame).unwrap();net.send(&frame);arp_at=now+5000;}
    if net.gateway_seen!=0 && ping_seq<5 && now>=ping_at {
     let mut packet=[0;24];packet[0]=8;set16(&mut packet,4,0x5242);set16(&mut packet,6,ping_seq);packet[8..16].copy_from_slice(&(seed^cfg_req as u64).to_be_bytes());packet[16..24].copy_from_slice(&(ping_seq as u64).to_be_bytes());let check=checksum(&packet);set16(&mut packet,2,check);
     if sockets.get_mut::<icmp::Socket>(ping_handle).send_slice(&packet,IpAddress::Ipv4(Ipv4Address::from_bits(value.gateway))).is_ok() {ping_seq+=1;ping_at=now+1000;}
    }
   }
  }
  if let Ok((data,address))=sockets.get_mut::<icmp::Socket>(ping_handle).recv() {
   if data.len()==24 && data[0]==0 && checksum(data)==0 && u16at(data,4)==0x5242 && u16at(data,6)<ping_seq && ping_mask&(1<<u16at(data,6))==0 && data[8..16]==(seed^cfg_req as u64).to_be_bytes() && active.is_some_and(|w|address==IpAddress::Ipv4(Ipv4Address::from_bits(w.gateway))) {ping_mask|=1<<u16at(data,6);replies=replies.saturating_add(1);sel4_microkit::debug_println!("WAN gateway IPv4 reply count={}",replies);}
  }
  if now!=last_status {
   last_status=now;let config=active.or(candidate);let ready=active.is_some() && net.gateway_seen!=0 && now.saturating_sub(net.gateway_seen)<=60000 && net.allow;
   word(0).fetch_add(1,Ordering::AcqRel);
   for (offset,value) in [(4,1),(8,now as u32),(12,0),(16,0),(20,epoch),(24,0),(28,u32::from(net.fault)),(32,cfg_req),(36,u32::from(ready)),(40,config.map_or(0,|w|w.address)),(44,config.map_or(0,|w|w.mask)),(48,config.map_or(0,|w|w.gateway)),(52,config.map_or(0,|w|w.dns[0])),(56,config.map_or(0,|w|w.dns[1])),(60,config.map_or(0,|w|w.dns[2])),(64,config.map_or(0,|w|w.dns_count as u32)),(68,config.map_or(0,|w|w.expires.saturating_sub(now) as u32)),(72,net.gateway_seen as u32),(76,replies),(80,net.tx_drops),(84,net.rx_drops),(88,reject),(92,server)] {word(offset).store(value,Ordering::Relaxed);}
   word(0).fetch_add(1,Ordering::Release);
  }
  core::hint::spin_loop();
 }
 #[allow(unreachable_code)] Idle
}
struct Idle;impl Handler for Idle {type Error=Infallible;}
