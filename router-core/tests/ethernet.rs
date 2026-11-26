use r5s_router_core::{ethernet::EthernetRouter, *};
use std::time::Instant;

const CLIENT: [[u8; 6]; 2] = [[2, 1, 2, 3, 4, 1], [2, 1, 2, 3, 4, 2]];
const GATEWAY: [u8; 6] = [2, 3, 4, 5, 6, 7];

fn router_mac(port: Port) -> [u8; 6] {
    [2, 0x52, 0x35, 0x53, 0, port as u8 + 1]
}

fn wan() -> Wan {
    Wan { address: 0x0a000132, mask: 0xffffff00, gateway: 0x0a000101,
          dns: [0x0a000101,0,0], dns_count: 1, expires: 60000 }
}

fn udp(mac: [u8; 6], dest: [u8; 6], src: Ip, dst: Ip, sp: u16, dp: u16, data: &[u8]) -> Vec<u8> {
    let length = 28 + data.len();
    let mut frame = vec![0; 14 + length];
    frame[..6].copy_from_slice(&dest);
    frame[6..12].copy_from_slice(&mac);
    set16(&mut frame, 12, 0x0800);
    let ip = &mut frame[14..];
    ip[0] = 0x45;
    set16(ip, 2, length as u16);
    ip[8] = 64;
    ip[9] = 17;
    setip(ip, 12, src);
    setip(ip, 16, dst);
    set16(ip, 20, sp);
    set16(ip, 22, dp);
    set16(ip, 24, (8 + data.len()) as u16);
    ip[28..].copy_from_slice(data);
    set16(ip, 10, checksum(&ip[..20]));
    frame
}

fn arp_reply(port: Port, mac: [u8;6], source: Ip, target: Ip) -> [u8;60] {
    let mut frame = [0;60];
    arp::request(mac, source, target, &mut frame).unwrap();
    frame[..6].copy_from_slice(&router_mac(port));
    frame[32..38].copy_from_slice(&router_mac(port));
    set16(&mut frame, 20, 2);
    frame
}

fn dhcp_request(mac: [u8;6], kind: u8, wanted: Option<Ip>) -> Vec<u8> {
    let mut payload = vec![0;240];
    payload[..4].copy_from_slice(&[1, 1, 6, 0]);
    payload[28..34].copy_from_slice(&mac);
    payload[236..].copy_from_slice(&[99,130,83,99]);
    payload.extend([53,1,kind]);
    if let Some(ip) = wanted {
        payload.extend([50,4]);
        payload.extend(ip.to_be_bytes());
    }
    payload.push(255);
    udp(mac, [255;6], 0, u32::MAX, 68, 67, &payload)
}

#[test]
fn three_port_frame_simulation() {
    let mut router = EthernetRouter::new(42);
    let mut out = [0;1514];
    router.configure_wan(Some(wan()), 0).unwrap();
    let mut ports = [0;2];
    let remote = 0x08080808;
    for (index, port) in [Port::Lan1, Port::Lan2].into_iter().enumerate() {
        let address = LAN[index] + 99;
        let mac = CLIENT[index];
        let discover = dhcp_request(mac, 1, None);
        assert_eq!(router.receive(port, &discover, 0, &mut out), Ok(Some((port, 60))));
        assert_eq!(arp::parse(&out).unwrap().3, address);
        let (_, size) = router.tick_lan(port, 1000, &mut out).unwrap().unwrap();
        assert_eq!(dhcp::reply_options(&out[42..size]).unwrap().kind, 2);
        assert_eq!(ipat(&out, 58), address);
        let (_, size) = router.receive(port, &dhcp_request(mac, 3, Some(address)), 1001, &mut out).unwrap().unwrap();
        assert_eq!(dhcp::reply_options(&out[42..size]).unwrap().kind, 5);
        let packet = udp(mac, router_mac(port), address, remote, 1234, 53, b"query");
        let result = router.receive(port, &packet, 1002, &mut out).unwrap();
        if index == 0 {
            assert_eq!(result, Some((Port::Wan,60)));
            assert_eq!(arp::parse(&out).unwrap().3, wan().gateway);
            assert_eq!(router.receive(port, &packet, 1003, &mut out), Err(Drop::Full));
            let reply = arp_reply(Port::Wan, GATEWAY, wan().gateway, wan().address);
            assert_eq!(router.receive(Port::Wan, &reply, 1004, &mut out), Ok(Some((Port::Wan,packet.len()))));
        } else {
            assert_eq!(result, Some((Port::Wan,packet.len())));
        }
        assert_eq!(&out[..6], &GATEWAY);
        assert_eq!(ipat(&out,26), wan().address);
        ports[index] = u16at(&out,34);
        let cross = udp(mac, router_mac(port), address, LAN[1-index] + 99, 1234, 53, b"cross");
        assert_eq!(router.receive(port, &cross, 1005, &mut out), Err(Drop::Policy));
    }
    assert_ne!(ports[0], ports[1]);
    for (index, port) in [Port::Lan1, Port::Lan2].into_iter().enumerate() {
        let address = LAN[index] + 99;
        let packet = udp(GATEWAY, router_mac(Port::Wan), remote, wan().address, 53, ports[index], b"answer");
        assert_eq!(router.receive(Port::Wan, &packet, 1010, &mut out), Ok(Some((port,60))));
        let reply = arp_reply(port, CLIENT[index], address, LAN[index]);
        assert_eq!(router.receive(port, &reply, 1011, &mut out), Ok(Some((port,packet.len()))));
        assert_eq!(&out[..6], &CLIENT[index]);
        assert_eq!(ipat(&out,30), address);
        assert_eq!(u16at(&out,36), 1234);
    }
    let unsolicited = udp(GATEWAY, router_mac(Port::Wan), remote, wan().address, 53, 30000, b"drop");
    assert_eq!(router.receive(Port::Wan, &unsolicited, 1100, &mut out), Err(Drop::Policy));
    router.link_down(Port::Lan1, 1200);
    let old1 = udp(GATEWAY, router_mac(Port::Wan), remote, wan().address, 53, ports[0], b"old");
    let old2 = udp(GATEWAY, router_mac(Port::Wan), remote, wan().address, 53, ports[1], b"old");
    assert_eq!(router.receive(Port::Wan, &old1, 1201, &mut out), Err(Drop::Policy));
    let start = Instant::now();
    for _ in 0..100_000 {
        assert_eq!(router.receive(Port::Wan, &old2, 1201, &mut out), Ok(Some((Port::Lan2,old2.len()))));
    }
    println!("100000 cached host frame forwards in {:?}", start.elapsed());
    router.configure_wan(Some(Wan { address: wan().address + 1, ..wan() }), 1300).unwrap();
    let stale = udp(GATEWAY, router_mac(Port::Wan), remote, wan().address + 1, 53, ports[1], b"old");
    assert_eq!(router.receive(Port::Wan, &stale, 1301, &mut out), Err(Drop::Policy));
    router.link_down(Port::Wan, 1400);
    assert_eq!(router.receive(Port::Wan, &stale, 1401, &mut out), Err(Drop::NoWan));
}

fn tcp(mac: [u8; 6], port: Port, src: Ip, dst: Ip, ports: (u16,u16), seq: u32, ack: u32, flags: u8) -> Vec<u8> {
    let mut frame = udp(mac, router_mac(port), src, dst, ports.0, ports.1, &[0;12]);
    let ip = &mut frame[14..];
    ip[9] = 6;
    setip(ip, 24, seq);
    setip(ip, 28, ack);
    ip[32] = 0x50;
    ip[33] = flags;
    set16(ip, 34, 1024);
    set16(ip, 10, 0);
    set16(ip, 10, checksum(&ip[..20]));
    set16(ip, 36, transport_checksum(ip,40));
    frame
}

#[test]
fn both_lans_complete_independent_tcp_handshakes() {
    let remote = 0x08080808;
    let mut router = EthernetRouter::new(55);
    let mut out = [0;1514];
    let mut translated = [0;2];
    router.configure_wan(Some(wan()), 0).unwrap();
    for (i, port) in [Port::Lan1, Port::Lan2].into_iter().enumerate() {
        let address = LAN[i] + 99;
        let syn = tcp(CLIENT[i], port, address, remote, (10000,443), 10, 0, 2);
        if i == 0 {
            assert_eq!(router.receive(port, &syn, 0, &mut out), Ok(Some((Port::Wan,60))));
            let arp = arp_reply(Port::Wan, GATEWAY, wan().gateway, wan().address);
            assert_eq!(router.receive(Port::Wan, &arp, 1, &mut out), Ok(Some((Port::Wan,54))));
        } else {
            assert_eq!(router.receive(port, &syn, 1, &mut out), Ok(Some((Port::Wan,54))));
        }
        translated[i] = u16at(&out,34);
    }
    assert_ne!(translated[0], translated[1]);
    for (i, port) in [Port::Lan1, Port::Lan2].into_iter().enumerate() {
        let address = LAN[i] + 99;
        let syn_ack = tcp(GATEWAY, Port::Wan, remote, wan().address, (443,translated[i]), 42, 11, 18);
        assert_eq!(router.receive(Port::Wan, &syn_ack, 2, &mut out), Ok(Some((port,60))));
        let arp = arp_reply(port, CLIENT[i], address, LAN[i]);
        assert_eq!(router.receive(port, &arp, 3, &mut out), Ok(Some((port,54))));
        assert_eq!(ipat(&out,30),address);
        assert_eq!(u16at(&out,36),10000);
        let ack = tcp(CLIENT[i], port, address, remote, (10000,443), 11, 43, 16);
        assert_eq!(router.receive(port, &ack, 4, &mut out), Ok(Some((Port::Wan,54))));
        assert_eq!(u16at(&out,34),translated[i]);
        assert_eq!(transport_checksum(&out[14..54],40),0);
    }
}

#[test]
fn conflicting_arp_suppresses_dhcp_offer_only_on_affected_port() {
    let mut router = EthernetRouter::new(1);
    let mut out = [0;1514];
    for (i, port) in [Port::Lan1, Port::Lan2].into_iter().enumerate() {
        let discover = dhcp_request(CLIENT[i], 1, None);
        assert_eq!(router.receive(port, &discover, 0, &mut out), Ok(Some((port,60))));
    }
    let mut conflict = [0;60];
    arp::request(GATEWAY, LAN[0] + 99, LAN[0] + 99, &mut conflict).unwrap();
    assert_eq!(router.receive(Port::Lan1, &conflict, 200, &mut out), Ok(None));
    assert_eq!(router.tick_lan(Port::Lan1, 1000, &mut out), Ok(None));
    assert_eq!(router.tick_lan(Port::Lan2, 1000, &mut out), Ok(Some((Port::Lan2,342))));
}

#[test]
fn ttl_expiry_returns_icmp_to_ingress_and_fragments_fail_closed() {
    let mut router = EthernetRouter::new(66);
    router.configure_wan(Some(wan()), 0).unwrap();
    let mut out = [0; 1514];
    let port = Port::Lan1;
    let mut packet = udp(CLIENT[0], router_mac(port), LAN[0]+99, 0x08080808, 1234, 53, b"x");
    packet[22] = 1;
    set16(&mut packet, 24, 0);
    let c = checksum(&packet[14..34]);
    set16(&mut packet, 24, c);
    assert_eq!(router.receive(port, &packet, 1, &mut out), Ok(Some((port,70))));
    assert_eq!(&out[..6], &CLIENT[0]);
    assert_eq!(out[34], 11);
    assert_eq!(checksum(&out[34..70]), 0);
    assert_eq!(&out[42..70], &packet[14..42]);
    set16(&mut packet, 20, 0x2000);
    set16(&mut packet, 24, 0);
    let c = checksum(&packet[14..34]);
    set16(&mut packet, 24, c);
    assert_eq!(router.receive(port, &packet, 2, &mut out), Err(Drop::Unsupported));
}

// Supply deterministic GMAC queues to the actual WAN smoltcp Device adapter.
mod queue {
    use std::{cell::RefCell, collections::VecDeque};
    pub const BASE: usize = 0;
    pub const DIRECTION: usize = 1;
    thread_local! {
        pub static RX: RefCell<VecDeque<Vec<u8>>> = RefCell::new(VecDeque::new());
        pub static TX: RefCell<VecDeque<Vec<u8>>> = RefCell::new(VecDeque::new());
    }
    pub fn pop(base: usize, _: u32, out: &mut [u8]) -> Result<usize, ()> {
        assert_eq!(base, BASE);
        RX.with_borrow_mut(|queue| {
            let Some(frame) = queue.pop_front() else { return Ok(0) };
            if frame.len() > out.len() { return Err(()); }
            out[..frame.len()].copy_from_slice(&frame);
            Ok(frame.len())
        })
    }
    pub fn pop_meta(base:usize,epoch:u32,out:&mut [u8])->Result<(usize,u32),()> {
        pop(base,epoch,out).map(|n|(n,0))
    }
    pub fn push_meta(base:usize,epoch:u32,frame:&[u8],_:u32)->Result<bool,()> {
        push(base,epoch,frame)
    }
    pub fn push(base: usize, _: u32, frame: &[u8]) -> Result<bool, ()> {
        assert_eq!(base, DIRECTION);
        TX.with_borrow_mut(|queue| {
            if queue.len() >= 8 { return Ok(false); }
            queue.push_back(frame.to_vec());
            Ok(true)
        })
    }
}

#[allow(dead_code)]
#[path = "../../pds/wan_dhcp_freshness/src/device.rs"]
mod wan_device;
#[path = "../../pds/wan_dhcp_freshness/src/progress.rs"]
mod progress;

#[test]
fn real_wan_dhcp_socket_to_lan_forwarding() {
    use smoltcp::{
        iface::{Config, Interface, SocketSet, SocketStorage},
        socket::dhcpv4,
        time::Instant,
        wire::{DhcpMessageType, DhcpPacket, DhcpRepr, EthernetAddress},
    };
    let mut device = wan_device::Net::new();
    device.allow = true;
    device.remaining = 8;
    let zero = Instant::from_millis(0);
    let mut iface = Interface::new(Config::new(EthernetAddress(wan_device::MAC).into()), &mut device, zero);
    let mut backing = [SocketStorage::EMPTY];
    let mut sockets = SocketSet::new(&mut backing[..]);
    let mut reply_buffer = [0; 1514];
    let mut dhcp = dhcpv4::Socket::new();
    dhcp.set_receive_packet_buffer(&mut reply_buffer);
    let handle = sockets.add(dhcp);
    assert!(matches!(sockets.get_mut::<dhcpv4::Socket>(handle).poll(), Some(dhcpv4::Event::Deconfigured)));
    let mut lease = None;

    fn next_request() -> Vec<u8> {
        queue::TX.with_borrow_mut(|tx| tx.pop_front().expect("DHCP transmission"))
    }
    fn reply(request: &[u8], kind: u8, config: Wan) -> Vec<u8> {
        let mut payload = vec![0; 240];
        payload[..4].copy_from_slice(&[2, 1, 6, 0]);
        payload[4..8].copy_from_slice(&request[46..50]);
        setip(&mut payload, 16, config.address);
        setip(&mut payload, 20, config.gateway);
        payload[28..34].copy_from_slice(&wan_device::MAC);
        payload[236..].copy_from_slice(&[99, 130, 83, 99]);
        payload.extend_from_slice(&[53, 1, kind]);
        for (option, value) in [(54, config.gateway), (1, config.mask), (3, config.gateway),
                               (6, config.dns[0]), (51, 60), (58, 30), (59, 52)] {
            payload.extend_from_slice(&[option, 4]);
            payload.extend_from_slice(&value.to_be_bytes());
        }
        payload.push(255);
        udp(GATEWAY, [255; 6], config.gateway, u32::MAX, 67, 68, &payload)
    }

    iface.poll(zero, &mut device, &mut sockets);
    let discover = next_request();
    assert_eq!(device.progress.bits, 1);
    let packet = DhcpPacket::new_checked(&discover[42..]).unwrap();
    let decoded = DhcpRepr::parse(&packet).unwrap();
    assert_eq!(decoded.message_type, DhcpMessageType::Discover);
    let offer = reply(&discover, 2, wan());
    let mut corrupt = offer.clone();
    corrupt[46] ^= 1;
    queue::RX.with_borrow_mut(|rx| {
        rx.push_back(corrupt);
        rx.push_back(offer);
    });
    iface.poll(Instant::from_millis(1), &mut device, &mut sockets);
    let request = next_request();
    assert_eq!(device.progress.bits, 7);
    let packet = DhcpPacket::new_checked(&request[42..]).unwrap();
    let decoded = DhcpRepr::parse(&packet).unwrap();
    assert_eq!(decoded.message_type, DhcpMessageType::Request);
    assert_eq!(decoded.requested_ip.unwrap().to_bits(), wan().address);

    let mut unrelated = reply(&request, 5, wan());
    unrelated[46] ^= 1;
    queue::RX.with_borrow_mut(|rx| rx.push_back(unrelated));
    iface.poll(Instant::from_millis(2), &mut device, &mut sockets);
    assert!(sockets.get_mut::<dhcpv4::Socket>(handle).poll().is_none());

    let mut wrong = wan();
    wrong.gateway += 1;
    queue::RX.with_borrow_mut(|rx| rx.push_back(reply(&request, 5, wrong)));
    device.remaining = 8;
    iface.poll(Instant::from_millis(2), &mut device, &mut sockets);
    assert!(sockets.get_mut::<dhcpv4::Socket>(handle).poll().is_none());

    let mut ack = reply(&request, 5, wan());
    ack[..6].copy_from_slice(&wan_device::MAC);
    queue::RX.with_borrow_mut(|rx| rx.push_back(ack));
    iface.poll(Instant::from_millis(3), &mut device, &mut sockets);
    if let Some(dhcpv4::Event::Configured(config)) = sockets.get_mut::<dhcpv4::Socket>(handle).poll() {
        let packet = DhcpRepr::parse(config.packet.as_ref().unwrap()).unwrap();
        let mut dns = [0; 3];
        for (index, addr) in config.dns_servers.iter().enumerate() {
            dns[index] = addr.to_bits();
        }
        lease = Some(Wan {
            address: config.address.address().to_bits(),
            mask: u32::MAX << (32 - config.address.prefix_len()),
            gateway: config.router.unwrap().to_bits(),
            dns,
            dns_count: config.dns_servers.len(),
            expires: 3 + 1000 * u64::from(packet.lease_duration.unwrap()),
        });
    }
    let lease = lease.expect("matching DHCP ACK");
    assert_eq!(lease.address, wan().address);
    let mut router = EthernetRouter::new(42);
    router.configure_wan(Some(lease), 3).unwrap();
    let mut out = [0; 1514];

    for (index, port) in [Port::Lan1, Port::Lan2].into_iter().enumerate() {
        let client = CLIENT[index];
        let local = LAN[index] + 99;
        router.receive(port, &dhcp_request(client, 1, None), 4, &mut out).unwrap();
        router.tick_lan(port, 1004, &mut out).unwrap().unwrap();
        let (_, size) = router.receive(port, &dhcp_request(client, 3, Some(local)), 1005, &mut out).unwrap().unwrap();
        assert_eq!(dhcp::reply_options(&out[42..size]).unwrap().kind, 5);
        let outbound = udp(client, router_mac(port), local, 0x08080808, 1500, 53, b"query");
        if index == 0 {
            assert_eq!(router.receive(port, &outbound, 1006, &mut out), Ok(Some((Port::Wan,60))));
            let arp = arp_reply(Port::Wan, GATEWAY, lease.gateway, lease.address);
            assert_eq!(router.receive(Port::Wan, &arp, 1007, &mut out), Ok(Some((Port::Wan,outbound.len()))));
        } else {
            assert_eq!(router.receive(port, &outbound, 1007, &mut out), Ok(Some((Port::Wan,outbound.len()))));
        }
        assert_eq!(ipat(&out,26), lease.address);
        let remote_port = u16at(&out, 34);
        let incoming = udp(GATEWAY, router_mac(Port::Wan), 0x08080808, lease.address,
                           53, remote_port, b"answer");
        assert_eq!(router.receive(Port::Wan, &incoming, 1010, &mut out), Ok(Some((port,60))));
        let lan_arp = arp_reply(port, client, local, LAN[index]);
        assert_eq!(router.receive(port, &lan_arp, 1011, &mut out), Ok(Some((port,incoming.len()))));
        assert_eq!(ipat(&out,30),local);
        assert_eq!(u16at(&out,36),1500);
    }

    // At T2, rebinding sends a broadcast even if the old server cannot be reached.
    iface.update_ip_addrs(|addrs| {
        addrs.push(smoltcp::wire::IpCidr::Ipv4(smoltcp::wire::Ipv4Cidr::new(
            smoltcp::wire::Ipv4Address::from_bits(lease.address), 24,
        ))).unwrap();
    });
    let renew_at = 30_003;
    device.remaining = 8;
    iface.poll(Instant::from_millis(renew_at), &mut device, &mut sockets);
    let arp_request = next_request();
    assert_eq!(arp::parse(&arp_request).unwrap().3, lease.gateway);
    queue::RX.with_borrow_mut(|rx| rx.push_back(
        arp_reply(Port::Wan, GATEWAY, lease.gateway, lease.address).to_vec()
    ));
    iface.poll(Instant::from_millis(renew_at + 1), &mut device, &mut sockets);
    let renew = next_request();
    assert_eq!(&renew[..6], &GATEWAY);
    let packet = DhcpPacket::new_checked(&renew[42..]).unwrap();
    let decoded = DhcpRepr::parse(&packet).unwrap();
    assert_eq!(decoded.message_type, DhcpMessageType::Request);
    assert_eq!(decoded.client_ip.to_bits(), lease.address);

    let rebind_at = 52_003;
    device.remaining = 8;
    iface.poll(Instant::from_millis(rebind_at), &mut device, &mut sockets);
    let rebind = next_request();
    assert_eq!(&rebind[..6], &[255; 6]);
    let packet = DhcpPacket::new_checked(&rebind[42..]).unwrap();
    let decoded = DhcpRepr::parse(&packet).unwrap();
    assert_eq!(decoded.message_type, DhcpMessageType::Request);
    assert_eq!(decoded.client_ip.to_bits(), lease.address);
    queue::RX.with_borrow_mut(|rx| rx.push_back(reply(&rebind, 5, lease)));
    iface.poll(Instant::from_millis(rebind_at + 1), &mut device, &mut sockets);
    assert!(matches!(sockets.get_mut::<dhcpv4::Socket>(handle).poll(),
                     Some(dhcpv4::Event::Configured(_))));
    let new_expiry = rebind_at as u64 + 60_001;
    router.configure_wan(Some(Wan { expires: new_expiry, ..lease }), rebind_at as u64 + 1).unwrap();

    device.remaining = 8;
    iface.poll(Instant::from_millis(new_expiry as i64), &mut device, &mut sockets);
    assert!(matches!(sockets.get_mut::<dhcpv4::Socket>(handle).poll(),
                     Some(dhcpv4::Event::Deconfigured)));
    let expired = udp(CLIENT[0], router_mac(Port::Lan1), LAN[0] + 99,
                      0x08080808, 1500, 53, b"expired");
    assert_eq!(router.receive(Port::Lan1, &expired, new_expiry, &mut out), Err(Drop::NoWan));
}

#[test]
fn narrower_wan_returns_fragmentation_needed_for_df() {
    let mut router = EthernetRouter::new(1);
    router.configure_wan(Some(wan()), 0).unwrap();
    router.set_mtu(Port::Wan, 1200).unwrap();
    let mut out = [0;1514];
    let mut frame = udp(CLIENT[0], router_mac(Port::Lan1), LAN[0]+99, 0x08080808,
                        4000, 53, &[0; 1250]);
    assert_eq!(router.receive(Port::Lan1,&frame,1,&mut out), Err(Drop::Mtu));
    set16(&mut frame,20,0x4000);
    set16(&mut frame,24,0);
    let c = checksum(&frame[14..34]);
    set16(&mut frame,24,c);
    assert_eq!(router.receive(Port::Lan1,&frame,2,&mut out), Ok(Some((Port::Lan1,70))));
    assert_eq!(&out[34..36], &[3,4]);
    assert_eq!(u16at(&out,40),1200);
    assert_eq!(checksum(&out[34..70]),0);
    assert_eq!(&out[42..70],&frame[14..42]);
}

#[test]
fn service_drops_old_wan_lease_and_continues_other_port_on_queue_failure() {
    use r5s_router_core::service::{Frames, Link, Service};
    struct Model {
        links: [Option<Link>; 3],
        ingress: [Option<Vec<u8>>; 3],
        output: Vec<Port>,
    }
    impl Frames for Model {
        fn link(&mut self, port: Port) -> Option<Link> { self.links[port as usize] }
        fn take(&mut self, port: Port, epoch: u32, out: &mut [u8;1514]) -> Result<usize, Drop> {
            assert_eq!(epoch, self.links[port as usize].unwrap().epoch);
            let Some(frame) = self.ingress[port as usize].take() else { return Ok(0) };
            out[..frame.len()].copy_from_slice(&frame);
            Ok(frame.len())
        }
        fn send(&mut self, port: Port, epoch: u32, frame: &[u8]) -> Result<(), Drop> {
            assert_eq!(epoch, self.links[port as usize].unwrap().epoch);
            assert_eq!(frame[12..14], [8,6]);
            self.output.push(port);
            if port == Port::Lan1 { Err(Drop::Full) } else { Ok(()) }
        }
    }
    let mut service = Service::new(7);
    let mut io = Model {
        links: [Some(Link { up: true, epoch: 1 });3],
        ingress: [None, None, None],
        output: Vec::new(),
    };
    service.poll(&mut io,0);
    assert_eq!(service.lease(1,Some(wan()),0), Ok(()));
    io.links[0] = None;
    service.poll(&mut io,1);
    // Missing sample does not invalidate the preceding validated generation.
    assert_eq!(service.lease(1,Some(wan()),1),Ok(()));
    io.links[0] = Some(Link { up: true, epoch: 2 });
    io.ingress[1] = Some(dhcp_request(CLIENT[0],1,None));
    io.ingress[2] = Some(dhcp_request(CLIENT[1],1,None));
    assert_eq!(service.poll(&mut io,2),[None,Some(Drop::Full),None]);
    assert_eq!(io.output, [Port::Lan1, Port::Lan2]);
    assert_eq!(service.lease(1,Some(wan()),2),Err(Drop::State));
    io.ingress[2] = Some(udp(CLIENT[1],router_mac(Port::Lan2),LAN[1]+99,0x08080808,1234,53,b"x"));
    assert_eq!(service.poll(&mut io,3),[None,None,Some(Drop::NoWan)]);
}

#[test]
fn renewals_use_ciaddr_and_current_dns_on_each_lan() {
    let mut router = EthernetRouter::new(9);
    let mut out = [0;1514];
    for (i,port) in [Port::Lan1,Port::Lan2].into_iter().enumerate() {
        let mac = CLIENT[i];
        let address = LAN[i]+99;
        router.receive(port,&dhcp_request(mac,1,None),0,&mut out).unwrap();
        router.tick_lan(port,1000,&mut out).unwrap().unwrap();
        assert_eq!((&out[..6],ipat(&out,30)),(&mac[..],address));
        router.receive(port,&dhcp_request(mac,3,Some(address)),1001,&mut out).unwrap().unwrap();
        let config=Wan {dns:[0x0a000102+i as u32,0,0],..wan()};
        router.configure_wan(Some(config),1002).unwrap();
        let mut renew=dhcp_request(mac,3,None);
        setip(&mut renew,42+12,address);
        let frame=udp(mac,router_mac(port),address,LAN[i],68,67,&renew[42..]);
        let (_,size)=router.receive(port,&frame,1003,&mut out).unwrap().unwrap();
        let opts=dhcp::reply_options(&out[42..size]).unwrap();
        assert_eq!((opts.kind,opts.lease,opts.renew,opts.rebind),(5,Some(43200),Some(21600),Some(37800)));
        assert_eq!((&out[..6],ipat(&out,30)),(&mac[..],address));
        assert!(out[282..size].windows(6).any(|v|v[..2]==[6,4] && ipat(v,2)==config.dns[0]));
    }
    let bad=Wan {address:LAN[0]+5,gateway:LAN[0],dns:[0x08080808,0,0],..wan()};
    assert_eq!(router.configure_wan(Some(bad),1004),Err(Drop::Overlap));
    let pkt=udp(CLIENT[0],router_mac(Port::Lan1),LAN[0]+99,0x08080808,4000,53,b"query");
    assert_eq!(router.receive(Port::Lan1,&pkt,1005,&mut out),Err(Drop::NoWan));
}

#[test]
fn logical_lan_serves_shared_leases_arp_and_icmp_across_both_ports() {
    let mut bridge = bridge::Bridge::new();
    let dns = LAN[0] + 53;
    let mut stack = EthernetRouter::local_lan(17, Some(dns));
    let mut output = [0; 1514];
    let mut addresses = [0; 2];
    for (port, &mac) in CLIENT.iter().enumerate() {
        let now = port as u64 * 2000;
        let discover = dhcp_request(mac, 1, None);
        assert_eq!(bridge.receive(port, &discover, 3, now), Some((true, 1 << (1-port))));
        stack.receive(Port::Lan1, &discover, now, &mut output).unwrap().unwrap();
        let (_, size) = stack.tick_lan(Port::Lan1, now + 1000, &mut output).unwrap().unwrap();
        assert_eq!(bridge.destination(&output[..6], 3, now + 1000), 1 << port);
        assert_eq!(dhcp::reply_options(&output[42..size]).unwrap().kind, 2);
        addresses[port] = ipat(&output, 58);
        let request = dhcp_request(mac, 3, Some(addresses[port]));
        assert!(bridge.receive(port, &request, 3, now + 1001).unwrap().0);
        let (_, size) = stack.receive(Port::Lan1, &request, now + 1001, &mut output).unwrap().unwrap();
        assert_eq!(dhcp::reply_options(&output[42..size]).unwrap().kind, 5);
        let mut options = &output[282..size];
        let mut seen_dns = false;
        while options[0] != 255 {
            assert_ne!(options[0], 3);
            let len = usize::from(options[1]);
            if options[0] == 6 { assert_eq!(&options[2..2+len], &dns.to_be_bytes()); seen_dns = true; }
            options = &options[2+len..];
        }
        assert!(seen_dns);
    }
    assert_eq!(addresses, [(LAN[0] & 0xffffff00) + 100, (LAN[0] & 0xffffff00) + 101]);

    let mut renewal = dhcp_request(CLIENT[0], 3, None);
    setip(&mut renewal, 54, addresses[0]);
    assert!(bridge.receive(1, &renewal, 3, 5000).unwrap().0);
    let (_, length) = stack.receive(Port::Lan1, &renewal, 5000, &mut output).unwrap().unwrap();
    assert_eq!(dhcp::reply_options(&output[42..length]).unwrap().kind, 5);
    assert_eq!(ipat(&output, 58), addresses[0]);
    assert_eq!(bridge.destination(&output[..6], 3, 5000), 2);

    let mut frame = [0; 60];
    arp::request(CLIENT[0], addresses[0], LAN[0], &mut frame).unwrap();
    assert_eq!(stack.receive(Port::Lan1, &frame, 5100, &mut output), Ok(Some((Port::Lan1, 60))));
    assert_eq!(&output[6..12], &bridge::MAC);
    assert_eq!(stack.local_neighbor_count(5100), 1);
    assert_eq!(stack.local_neighbor_count(65_100), 0);

    let mut frame = udp(CLIENT[0], bridge::MAC, addresses[0], LAN[0], 0, 0, &[]);
    let ip = &mut frame[14..];
    ip[9] = 1;
    ip[20..28].copy_from_slice(&[8, 0, 0, 0, 0, 9, 0, 2]);
    let sum = checksum(&ip[20..]);
    set16(ip, 22, sum);
    set16(ip, 10, 0);
    let sum = checksum(&ip[..20]);
    set16(ip, 10, sum);
    assert_eq!(stack.receive(Port::Lan1, &frame, 5101, &mut output), Ok(Some((Port::Lan1, 42))));
    assert_eq!(output[34], 0);
    assert_eq!(checksum(&output[34..42]), 0);
    assert_eq!(validate_ip(&output[14..42]), Ok(28));
}

#[test]
fn shared_lan_releases_first_ping_on_wan_gateway_arp_reply() {
    let lease = wan();
    let client = LAN[0] + 100;
    let destination = 0x08080808;
    let mut stack = EthernetRouter::shared_lan_router(17, None);
    stack.configure_wan(Some(lease), 1).unwrap();
    let mut out = [0; 1514];
    let mut neighbor = [0; 60];
    arp::request(CLIENT[0], client, LAN[0], &mut neighbor).unwrap();
    stack.receive(Port::Lan1, &neighbor, 2, &mut out).unwrap();

    let mut request = udp(CLIENT[0], bridge::MAC, client, destination, 0, 0, b"ping");
    let ip = &mut request[14..];
    ip[9] = 1;
    ip[20..28].copy_from_slice(&[8, 0, 0, 0, 0, 9, 0, 1]);
    let sum = checksum(&ip[20..]);
    set16(ip, 22, sum);
    set16(ip, 10, 0);
    let sum = checksum(&ip[..20]);
    set16(ip, 10, sum);
    assert_eq!(stack.receive(Port::Lan1, &request, 3, &mut out), Ok(Some((Port::Wan, 60))));
    assert_eq!(arp::parse(&out[..60]).unwrap().3, lease.gateway);

    let gateway_reply = arp_reply(Port::Wan, GATEWAY, lease.gateway, lease.address);
    assert_eq!(stack.receive(Port::Wan, &gateway_reply, 4, &mut out),
               Ok(Some((Port::Wan, request.len()))));
    assert_eq!(&out[..6], &GATEWAY);
    assert_eq!(ipat(&out, 26), lease.address);
    assert_eq!(ipat(&out, 30), destination);

    let mut reply = out[..request.len()].to_vec();
    reply[..6].copy_from_slice(&router_mac(Port::Wan));
    reply[6..12].copy_from_slice(&GATEWAY);
    let ip = &mut reply[14..];
    setip(ip, 12, destination);
    setip(ip, 16, lease.address);
    ip[20] = 0;
    set16(ip, 22, 0);
    let sum = checksum(&ip[20..]);
    set16(ip, 22, sum);
    set16(ip, 10, 0);
    let sum = checksum(&ip[..20]);
    set16(ip, 10, sum);
    assert_eq!(stack.receive(Port::Wan, &reply, 5, &mut out),
               Ok(Some((Port::Lan1, reply.len()))));
    assert_eq!(&out[..6], &CLIENT[0]);
    assert_eq!(ipat(&out, 30), client);
    assert_eq!(u16at(&out, 38), 9);

    for source_port in 20_000..20_064 {
        let syn = tcp(CLIENT[0], Port::Lan1, client, destination, (source_port, 443), 1, 0, 2);
        assert_eq!(stack.receive(Port::Lan1, &syn, 6, &mut out),
                   Ok(Some((Port::Wan, syn.len()))));
    }
    for source_port in 20_000..20_032 {
        let syn = tcp(CLIENT[1], Port::Lan1, client + 1, destination, (source_port, 443), 1, 0, 2);
        assert_eq!(stack.receive(Port::Lan1, &syn, 6, &mut out),
                   Ok(Some((Port::Wan, syn.len()))));
    }
    let query = udp(CLIENT[0], bridge::MAC, client, lease.dns[0], 30_000, 53, b"query");
    assert_eq!(stack.receive(Port::Lan1, &query, 7, &mut out), Ok(Some((Port::Wan, query.len()))));

    set16(&mut request, 38, 41321);
    set16(&mut request, 36, 0);
    let sum = checksum(&request[34..]);
    set16(&mut request, 36, sum);
    assert_eq!(stack.receive(Port::Lan1, &request, 8, &mut out),
               Ok(Some((Port::Wan, request.len()))));

    #[cfg(feature = "shared-nat")]
    {
        for source_port in 21_000..21_413 {
            let syn = tcp(CLIENT[0], Port::Lan1, client, destination, (source_port, 443), 1, 0, 2);
            assert_eq!(stack.receive(Port::Lan1, &syn, 9, &mut out),
                       Ok(Some((Port::Wan, syn.len()))));
        }
        assert_eq!(stack.nat_slot_usage(9), (512, 512, 0));
        let extra = tcp(CLIENT[0], Port::Lan1, client, destination, (21_413, 443), 1, 0, 2);
        assert_eq!(stack.receive(Port::Lan1, &extra, 9, &mut out), Err(Drop::NatSlots));
        assert_eq!(stack.receive(Port::Lan1, &extra, 10_009, &mut out),
                   Ok(Some((Port::Wan, extra.len()))));
        assert_eq!(stack.nat_slot_usage(10_009), (512, 511, 0));
    }
}

#[test]
fn shared_wan_gateway_probe_also_resolves_a_regular_forward() {
    let mut lease = wan();
    lease.expires = 800;
    let mut stack = EthernetRouter::shared_lan_router(17, None);
    stack.configure_wan(Some(lease), 1).unwrap();
    let mut out = [0; 1514];
    let reply = arp_reply(Port::Wan, GATEWAY, lease.gateway, lease.address);
    assert_eq!(stack.receive(Port::Wan, &reply, 2, &mut out), Err(Drop::Policy));
    assert_eq!(stack.resolve_wan_gateway(3, &mut out), Ok(Some(60)));
    assert_eq!(arp::parse(&out[..60]).unwrap().3, lease.gateway);
    assert_eq!(stack.resolve_wan_gateway(4, &mut out), Ok(None));

    let packet = udp(CLIENT[0], bridge::MAC, LAN[0] + 100, 0x08080808, 42, 53, b"dns");
    assert_eq!(stack.receive(Port::Lan1, &packet, 5, &mut out), Ok(None));
    assert_eq!(stack.receive(Port::Wan, &reply, 6, &mut out), Ok(Some((Port::Wan, packet.len()))));
    assert_eq!(&out[..6], &GATEWAY);
    assert_eq!(stack.wan_gateway_neighbor(7), Some((GATEWAY, 800)));
    assert_eq!(stack.wan_gateway_neighbor(800), None);
    assert_eq!(stack.resolve_wan_gateway(800, &mut out), Err(Drop::NoWan));
}

#[test]
fn shared_lan_router_translates_dns_from_both_ports() {
    let mut stack = EthernetRouter::shared_lan_router(17, None);
    let upstream = wan();
    stack.configure_wan(Some(upstream), 1).unwrap();
    let mut bridge = bridge::Bridge::new();
    let mut output = [0; 1514];
    for (port, client) in CLIENT.into_iter().enumerate() {
        let now = 10 + port as u64 * 10;
        let address = (LAN[0] & 0xffffff00) + 100 + port as u32;
        let mut neighbor = [0; 60];
        arp::request(client, address, LAN[0], &mut neighbor).unwrap();
        stack.receive(Port::Lan1, &neighbor, now, &mut output).unwrap();
        let request = udp(client, bridge::MAC, address, upstream.dns[0], 4000, 53, b"query");
        assert!(bridge.receive(port, &request, 3, now).unwrap().0);
        let (_, mut size) = stack.receive(Port::Lan1, &request, now, &mut output).unwrap().unwrap();
        if port == 0 {
            assert_eq!(arp::parse(&output[..size]).unwrap().3, upstream.gateway);
            let reply = arp_reply(Port::Wan, GATEWAY, upstream.gateway, upstream.address);
            size = stack.receive(Port::Wan, &reply, now + 1, &mut output).unwrap().unwrap().1;
        }
        assert_eq!(&output[..6], &GATEWAY);
        let ip = &output[14..size];
        assert_eq!(ipat(ip, 12), upstream.address);
        let remote = udp(GATEWAY, router_mac(Port::Wan), upstream.dns[0], upstream.address, 53,
                         u16at(ip, 20), b"answer");
        let (egress, size) = stack.receive(Port::Wan, &remote, now + 2, &mut output).unwrap().unwrap();
        assert_eq!(egress, Port::Lan1);
        assert_eq!(ipat(&output[14..size], 16), address);
        assert_eq!(bridge.destination(&output[..6], 3, now + 2), 1 << port);
    }
    let unsolicited = udp(GATEWAY, router_mac(Port::Wan), upstream.dns[0], upstream.address, 53, 1234, b"no flow");
    assert_eq!(stack.receive(Port::Wan, &unsolicited, 40, &mut output), Err(Drop::Policy));

    let request = dhcp_request(CLIENT[0], 1, None);
    stack.receive(Port::Lan1, &request, 50, &mut output).unwrap();
    let (_, size) = stack.tick_lan(Port::Lan1, 1050, &mut output).unwrap().unwrap();
    let mut options = &output[282..size];
    let mut routes = 0;
    while options[0] != 255 {
        let len = usize::from(options[1]);
        match options[0] {
            3 => { assert_eq!(&options[2..2+len], &LAN[0].to_be_bytes()); routes |= 1; }
            6 => { assert_eq!(&options[2..2+len], &upstream.dns[0].to_be_bytes()); routes |= 2; }
            _ => {}
        }
        options = &options[2+len..];
    }
    assert_eq!(routes, 3);
}

#[cfg(feature = "shared-nat")]
#[test]
fn split_translation_preserves_neighbor_resolution_and_reverse_delivery() {
    let upstream=wan();
    let client=LAN[0]+99;
    let peer=0x08080808;
    let mut stack=EthernetRouter::shared_lan_router(5,None);
    let mut worker=nat::Router::sharded(9,2);
    stack.configure_wan(Some(upstream),0).unwrap();
    worker.configure(Some(upstream),0).unwrap();
    let mut neighbor=[0;60];
    let mut output=[0;1514];
    arp::request(CLIENT[0],client,LAN[0],&mut neighbor).unwrap();
    stack.receive(Port::Lan1,&neighbor,1,&mut output).unwrap();
    let mut bridge = bridge::Bridge::new();
    assert_eq!(stack.resolved_lan_routes(&bridge, 3, 1).count(), 0);
    bridge.learn_source(0, CLIENT[0], 3, 1);
    assert_eq!(stack.resolved_lan_routes(&bridge, 3, 1).next(), Some((client, CLIENT[0], 0, 60_001)));
    assert_eq!(stack.resolved_lan_routes(&bridge, 2, 1).count(), 0);
    bridge.learn_source(1, CLIENT[0], 3, 2);
    assert_eq!(stack.resolved_lan_routes(&bridge, 3, 2).next(), Some((client, CLIENT[0], 1, 60_001)));
    assert_eq!(stack.resolved_lan_routes(&bridge, 3, 60_001).count(), 0);
    let request=udp(CLIENT[0],bridge::MAC,client,peer,1234,53,b"lookup");
    let length=request.len()-14;
    output[..request.len()].copy_from_slice(&request);
    worker.forward(Port::Lan1,&mut output[14..request.len()],2,1500).unwrap();
    let external=u16at(&output,34);
    let (_,probe)=stack.finish_worker(Port::Lan1,None,length,2,&mut output).unwrap().unwrap();
    assert_eq!(arp::parse(&output[..probe]).unwrap().3,upstream.gateway);
    let reply=arp_reply(Port::Wan,GATEWAY,upstream.gateway,upstream.address);
    assert_eq!(stack.receive(Port::Wan,&reply,3,&mut output).unwrap(),Some((Port::Wan,request.len())));
    assert_eq!(&output[..6],&GATEWAY);
    let mut reply=udp(GATEWAY,router_mac(Port::Wan),peer,upstream.address,53,external,b"response");
    assert_eq!(steering::inbound(&reply[14..]),Some(2));
    worker.forward(Port::Wan,&mut reply[14..],4,1500).unwrap();
    output[..reply.len()].copy_from_slice(&reply);
    assert_eq!(stack.finish_worker(Port::Wan,None,reply.len()-14,4,&mut output).unwrap(),
               Some((Port::Lan1,reply.len())));
    assert_eq!(&output[..6],&CLIENT[0]);
    assert_eq!(ipat(&output,30),client);
    output[..reply.len()].copy_from_slice(&reply);
    assert_eq!(stack.finish_worker(Port::Wan,None,reply.len()-14,5,&mut output).unwrap(),
               Some((Port::Lan1,reply.len())));
    stack.link_down(Port::Lan1,6);
    output[..reply.len()].copy_from_slice(&reply);
    let (_, size) = stack.finish_worker(Port::Wan,None,reply.len()-14,7,&mut output).unwrap().unwrap();
    assert_eq!(arp::parse(&output[..size]).unwrap().3, client);
}
