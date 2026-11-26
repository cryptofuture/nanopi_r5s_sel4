#[path="../pds/wan_dhcp_freshness/src/device.rs"] mod device;
#[path="../pds/wan_dhcp_freshness/src/progress.rs"] mod progress;
use r5s_router_core::{arp,set16,setip,checksum};
use smoltcp::{iface::{Config,Interface,SocketSet},phy::Device,time::Instant,wire::{EthernetAddress,IpCidr,Ipv4Address,Ipv4Cidr}};
mod queue {
    use std::{cell::RefCell,collections::VecDeque};
    pub const BASE:usize=0;
    pub const DIRECTION:usize=1;
    thread_local! {
        pub static RX:RefCell<VecDeque<Vec<u8>>>=RefCell::default();
        pub static SENT:RefCell<Vec<(usize,u32,Vec<u8>)>>=RefCell::default();
    }
    pub fn pop(_:usize,_:u32,out:&mut [u8])->Result<usize,()> {
        let Some(v)=RX.with_borrow_mut(|q|q.pop_front()) else {return Ok(0)};
        out[..v.len()].copy_from_slice(&v);Ok(v.len())
    }
    pub fn pop_meta(base:usize,epoch:u32,out:&mut [u8])->Result<(usize,u32),()> {
        pop(base,epoch,out).map(|n|(n,0))
    }
    pub fn push_meta(base:usize,epoch:u32,frame:&[u8],_:u32)->Result<bool,()> {
        push(base,epoch,frame)
    }
    pub fn push(base:usize,epoch:u32,frame:&[u8])->Result<bool,()> {
        SENT.with_borrow_mut(|q|q.push((base,epoch,frame.to_vec())));Ok(true)
    }
}

#[cfg(feature = "lan-gated")]
#[test]
fn bound_wan_answers_on_link_arp_and_sends_unsolicited_ip_only_to_policy_queue() {
    let peer=[2,1,2,3,4,5];let gateway=0x0a000101;let assigned=0x0a00015b;
    let mut net=device::Net::new();net.active=assigned;net.gateway=gateway;
    net.epoch=14;net.remaining=8;net.allow=true;
    let now=Instant::from_millis(1);
    let mut iface=Interface::new(Config::new(EthernetAddress(device::MAC).into()),&mut net,now);
    iface.update_ip_addrs(|list|list.push(IpCidr::Ipv4(Ipv4Cidr::new(Ipv4Address::from_bits(assigned),24))).unwrap());
    let mut sockets=SocketSet::new(&mut [][..]);
    let mut frame=[0;60];arp::request(peer,gateway,assigned,&mut frame).unwrap();
    queue::RX.with_borrow_mut(|q|q.push_back(frame.to_vec()));
    iface.poll(now,&mut net,&mut sockets);
    queue::SENT.with_borrow_mut(|sent| {
        assert_eq!(sent.len(),1);
        let (base,epoch,reply)=sent.pop().unwrap();
        assert_eq!((base,epoch),(queue::DIRECTION,14));
        assert_eq!(arp::parse(&reply).unwrap(),(2,device::MAC,assigned,gateway));
        assert_eq!(&reply[..6],&peer);
    });
    let mut frame=[0;42];
    frame[..6].copy_from_slice(&device::MAC);frame[6..12].copy_from_slice(&peer);set16(&mut frame,12,0x0800);
    let ip=&mut frame[14..];ip[0]=0x45;set16(ip,2,28);ip[8]=64;ip[9]=17;
    setip(ip,12,gateway);setip(ip,16,assigned);set16(ip,20,1234);set16(ip,22,4321);set16(ip,24,8);
    let c=checksum(&ip[..20]);set16(ip,10,c);
    queue::RX.with_borrow_mut(|q|q.push_back(frame.to_vec()));
    assert!(net.receive(now).is_none());
    queue::SENT.with_borrow(|sent|assert_eq!(sent,&[(0x62000000,14,frame.to_vec())]));
}

#[cfg(feature="b51-udp")]
#[path="../pds/wan_dhcp_freshness/src/b51_udp.rs"]
mod b51_udp;
#[cfg(feature="b51-udp")]
fn b51_word(index: usize) -> u32 {
    match index { 7 => 0xff, 25 if cfg!(feature="lan-tx-stats") => 11, 25 => 1, _ => 0x81250000 + index as u32 }
}

#[cfg(feature="b51-udp")]
fn b51_snapshot() -> Option<[u32; b51_udp::WORDS]> { Some(core::array::from_fn(b51_word)) }

#[cfg(feature="b51-udp")]
#[test]
fn status_query_through_wan_queues_returns_all_words_and_checksums() {
    use r5s_router_core::{ipat, transport_checksum, u16at, validate_ip};
    const LOCAL: u32 = 0x0a00015b;
    const PEER: u32 = 0x0a000102;
    const MASK: u32 = 0xffffff00;
    let mut net = device::Net::new();
    net.allow = true;
    net.remaining = 8;
    net.active = LOCAL;
    net.bound_mask = MASK;
    net.epoch = 7;
    let mut request = vec![0; 14 + 20 + 8 + 16 + b51_udp::WORDS * 4];
    assert!(request.len() - 14 <= 1500);
    request[..6].copy_from_slice(&device::MAC);
    let mac = [2,1,2,3,4,5];
    request[6..12].copy_from_slice(&mac);
    set16(&mut request, 12, 0x0800);
    let ip = &mut request[14..];
    let len = ip.len();
    ip[0] = 0x45;
    ip[8] = 1;
    ip[9] = 17;
    set16(ip, 2, len as u16);
    setip(ip, 12, PEER);
    setip(ip, 16, LOCAL);
    set16(ip, 20, 53000);
    set16(ip, 22, 48125);
    set16(ip, 24, (len - 20) as u16);
    ip[28..44].copy_from_slice(b"B51Q\0\x01\0\0abcdefgh");
    let fix_sums = |ip: &mut [u8]| {
        set16(ip, 10, 0);
        let c = checksum(&ip[..20]); set16(ip, 10, c);
        set16(ip, 26, 0);
        let c = transport_checksum(ip, ip.len()); set16(ip, 26, c);
    };
    fix_sums(ip);
    let mut query = |ms| {
        let mut rx = request.clone();
        rx.extend_from_slice(&[0xa5; 4]);
        queue::RX.with_borrow_mut(|q|q.push_back(rx));
        assert!(net.receive(Instant::from_millis(ms)).is_none());
        queue::SENT.with_borrow_mut(|out|std::mem::take(out))
    };
    let sent = query(500);
    assert_eq!(sent.len(), 1);
    let (base, epoch, frame) = &sent[0];
    assert_eq!((*base,*epoch), (queue::DIRECTION, 7));
    assert_eq!(frame.len(), request.len());
    assert_eq!(&frame[..6], &mac);
    assert_eq!(&frame[6..12], &device::MAC);
    let ip = &frame[14..];
    assert_eq!(validate_ip(ip), Ok(len));
    assert_eq!(transport_checksum(ip,len), 0);
    assert_eq!((ipat(ip,12),ipat(ip,16),u16at(ip,20),u16at(ip,22)),(LOCAL,PEER,48125,53000));
    assert_eq!(&ip[28..44], b"B51R\0\x01\0\x02abcdefgh");
    for i in 0..b51_udp::WORDS { assert_eq!(ipat(ip,44+4*i), b51_word(i)); }
    assert!(query(501).iter().all(|(base,_,_)| *base != queue::DIRECTION));
    assert_eq!(query(750).len(), 1);

    #[cfg(feature="lan-tx-stats")]
    {
        let mut legacy = request[..14 + 20 + 8 + 16 + 348 * 4].to_vec();
        let ip = &mut legacy[14..];
        let len = ip.len();
        set16(ip, 2, len as u16);
        set16(ip, 24, (len - 20) as u16);
        fix_sums(ip);
        assert!(b51_udp::reply(&mut legacy, device::MAC, LOCAL, MASK, b51_snapshot));
        assert_eq!(ipat(&legacy[14..], 44+25*4), 10);
        assert_eq!(transport_checksum(&legacy[14..], len), 0);
    }

    for (offset,value) in [(12,0x0b),(16,0x0b),(6,0x20),(28,b'X'),(44,1),(8,0)] {
        let mut invalid = request.clone();
        invalid[14+offset] = value;
        fix_sums(&mut invalid[14..]);
        assert!(!b51_udp::reply(&mut invalid,device::MAC,LOCAL,MASK, || panic!("invalid query touched status")));
    }
    assert!(!b51_udp::reply(&mut request.clone(),device::MAC,LOCAL,MASK, || None));
    let mut corrupted = request.clone();
    corrupted[14+26] ^= 1;
    assert!(!b51_udp::reply(&mut corrupted,device::MAC,LOCAL,MASK,b51_snapshot));
    assert!(!b51_udp::reply(&mut request,device::MAC,0,0,b51_snapshot));
    assert!(!b51_udp::reply(&mut request[..100],device::MAC,LOCAL,MASK,b51_snapshot));

    let lan = r5s_router_core::LAN[0];
    let lan_mac = r5s_router_core::bridge::MAC;
    request[..6].copy_from_slice(&lan_mac);
    setip(&mut request, 30, lan);
    fix_sums(&mut request[14..]);
    assert!(!b51_udp::reply(&mut request,lan_mac,lan,MASK, || panic!("off-subnet LAN query touched status")));
    setip(&mut request, 26, lan + 1);
    fix_sums(&mut request[14..]);
    let mut bridge = r5s_router_core::bridge::Bridge::new();
    for port in 0..2 {
        let mut query = request.clone();
        assert_eq!(bridge.receive(port, &query, 3, 500), Some((true, 0)));
        assert!(b51_udp::reply(&mut query,lan_mac,lan,MASK,b51_snapshot));
        assert_eq!(&query[..6], &mac);
        assert_eq!(&query[6..12], &lan_mac);
        assert_eq!((ipat(&query,26), ipat(&query,30)), (lan, lan + 1));
        assert_eq!(transport_checksum(&query[14..],len), 0);
    }
}

#[cfg(feature = "lan-status")]
#[test]
fn fallback_waits_for_acd_and_withdraws_on_conflict_or_lease() {
    let fallback = 0xa9fe4d01;
    let mut net = device::Net::new();
    net.allow = true;
    net.update_fallback(44_999, fallback, true, Some(0));
    assert_eq!(net.fallback_active, 0);
    net.update_fallback(45_000, fallback, true, Some(0));
    for now in [46_000, 47_000, 49_000] { net.update_fallback(now, fallback, true, Some(0)); }
    assert_eq!(net.fallback_active, fallback);
    assert_eq!(queue::SENT.with_borrow(|frames| frames.len()), 3);
    let mut conflict = [0; 60];
    arp::request([2,1,2,3,4,5], fallback, fallback, &mut conflict).unwrap();
    queue::RX.with_borrow_mut(|rx| rx.push_back(conflict.to_vec()));
    net.remaining = 1;
    assert!(net.receive(Instant::from_millis(49_001)).is_none());
    assert_eq!(net.fallback_active, 0);
    net.update_fallback(60_000, fallback, true, Some(0));
    assert_eq!(net.fallback_active, 0);
    net.active = 0x0a000102;
    net.update_fallback(61_000, fallback, true, Some(0));
    assert_eq!(net.fallback_active, 0);
}

#[cfg(feature = "shared-wan")]
#[test]
fn router_relay_copies_bound_ipv4_and_arp_replies_only() {
    let mut net = device::Net::new();
    net.active = 0x0a00015b;
    net.epoch = 14;
    net.allow = true;
    net.remaining = 4;
    let peer = [2, 1, 2, 3, 4, 5];
    let mut request = [0; 60];
    arp::request(peer, 0x0a000101, net.active, &mut request).unwrap();
    let mut reply = request;
    reply[..6].copy_from_slice(&device::MAC);
    reply[32..38].copy_from_slice(&device::MAC);
    set16(&mut reply, 20, 2);
    let mut ip = [0; 60];
    ip[..6].copy_from_slice(&device::MAC);
    ip[6..12].copy_from_slice(&peer);
    set16(&mut ip, 12, 0x0800);
    ip[14] = 0x45;
    setip(&mut ip, 30, net.active);
    for frame in [request, reply, ip] {
        queue::RX.with_borrow_mut(|rx| rx.push_back(frame.to_vec()));
        let _ = net.receive(Instant::from_millis(10));
    }
    queue::SENT.with_borrow(|sent| {
        assert_eq!(sent.as_slice(), &[(0x64000000, 14, reply.to_vec()), (0x64000000, 14, ip.to_vec())]);
    });
}

#[cfg(feature = "shared-wan")]
#[test]
fn native_internet_echo_uses_gateway_and_returns_to_its_socket() {
    use smoltcp::{iface::SocketStorage, socket::icmp, wire::IpAddress};
    let mut net = device::Net::new();
    net.active = 0x0a000162;
    net.gateway = 0x0a000101;
    net.allow = true;
    net.internet_cookie = 0x5235;
    let now = Instant::from_millis(1);
    let mut iface = Interface::new(Config::new(EthernetAddress(device::MAC).into()), &mut net, now);
    iface.update_ip_addrs(|addrs| addrs.push(IpCidr::Ipv4(Ipv4Cidr::new(
        Ipv4Address::from_bits(net.active), 24))).unwrap());
    iface.routes_mut().add_default_ipv4_route(Ipv4Address::from_bits(net.gateway)).unwrap();
    let mut rx_metadata = [icmp::PacketMetadata::EMPTY];
    let mut tx_metadata = [icmp::PacketMetadata::EMPTY];
    let mut rx_data = [0; 64];
    let mut tx_data = [0; 64];
    let mut echo = icmp::Socket::new(icmp::PacketBuffer::new(&mut rx_metadata[..], &mut rx_data[..]),
                                     icmp::PacketBuffer::new(&mut tx_metadata[..], &mut tx_data[..]));
    echo.bind(icmp::Endpoint::Ident(0x5242)).unwrap();
    let mut storage = [SocketStorage::EMPTY];
    let mut sockets = SocketSet::new(&mut storage[..]);
    let handle = sockets.add(echo);
    let gateway = [2, 1, 2, 3, 4, 5];
    let mut arp_reply = [0; 60];
    arp::request(gateway, net.gateway, net.active, &mut arp_reply).unwrap();
    arp_reply[..6].copy_from_slice(&device::MAC);
    arp_reply[32..38].copy_from_slice(&device::MAC);
    set16(&mut arp_reply, 20, 2);
    queue::RX.with_borrow_mut(|q| q.push_back(arp_reply.to_vec()));
    net.remaining = 8;
    iface.poll(now, &mut net, &mut sockets);
    queue::SENT.with_borrow_mut(|q| q.clear());

    let mut payload = [0; 24];
    payload[0] = 8;
    set16(&mut payload, 4, 0x5242);
    set16(&mut payload, 6, device::INTERNET_SEQ);
    payload[8..16].copy_from_slice(&net.internet_cookie.to_be_bytes());
    payload[16..24].copy_from_slice(&u64::from(device::INTERNET_SEQ).to_be_bytes());
    let sum = checksum(&payload);
    set16(&mut payload, 2, sum);
    let remote = IpAddress::Ipv4(Ipv4Address::from_bits(device::INTERNET_TARGET));
    sockets.get_mut::<icmp::Socket>(handle).send_slice(&payload, remote).unwrap();
    iface.poll(Instant::from_millis(2), &mut net, &mut sockets);
    assert_eq!(net.internet_tx_mask, 1);
    let mut reply = queue::SENT.with_borrow_mut(|q| q.pop().unwrap().2);
    assert_eq!(&reply[..6], &gateway);
    reply[..6].copy_from_slice(&device::MAC);
    reply[6..12].copy_from_slice(&gateway);
    setip(&mut reply, 26, device::INTERNET_TARGET);
    setip(&mut reply, 30, net.active);
    reply[34] = 0;
    set16(&mut reply, 36, 0);
    let sum = checksum(&reply[34..58]);
    set16(&mut reply, 36, sum);
    queue::RX.with_borrow_mut(|q| q.push_back(reply));
    net.remaining = 8;
    iface.poll(Instant::from_millis(3), &mut net, &mut sockets);
    let (data, source) = sockets.get_mut::<icmp::Socket>(handle).recv().unwrap();
    assert_eq!(source, remote);
    assert_eq!(&data[4..], &payload[4..]);
    queue::SENT.with_borrow(|q| assert!(q.is_empty()));
}
