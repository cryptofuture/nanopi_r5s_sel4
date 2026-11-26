include!("../router-core/tests/ethernet.rs");

#[test]
fn discover_and_selecting_request_retry_after_silence() {
    use smoltcp::{iface::{Config, Interface, SocketSet, SocketStorage}, socket::dhcpv4,
                  time::Instant, wire::EthernetAddress};
    let mut net = wan_device::Net::new();
    net.allow = true;
    net.remaining = 8;
    let mut iface = Interface::new(Config::new(EthernetAddress(wan_device::MAC).into()),
                                   &mut net, Instant::from_millis(0));
    let mut storage = [SocketStorage::EMPTY];
    let mut sockets = SocketSet::new(&mut storage[..]);
    sockets.add(dhcpv4::Socket::new());
    for ms in [0, 10_000] {
        iface.poll(Instant::from_millis(ms), &mut net, &mut sockets);
        let discover = queue::TX.with_borrow_mut(|tx| tx.pop_front().unwrap());
        assert_eq!(dhcp::options(&discover[42..]).unwrap().kind, 1);
        if ms == 0 { continue; }
        let mut offer = vec![0;240];
        offer[..3].copy_from_slice(&[2,1,6]);
        offer[4..8].copy_from_slice(&discover[46..50]);
        setip(&mut offer,16,wan().address);
        offer[28..34].copy_from_slice(&wan_device::MAC);
        offer[236..].copy_from_slice(&[99,130,83,99]);
        offer.extend_from_slice(&[53,1,2,54,4]);
        offer.extend_from_slice(&wan().gateway.to_be_bytes());
        offer.push(255);
        queue::RX.with_borrow_mut(|rx|rx.push_back(udp(GATEWAY, [255;6],wan().gateway,u32::MAX,67,68,&offer)));
    }
    for ms in [10_001,15_001] {
        iface.poll(Instant::from_millis(ms), &mut net, &mut sockets);
        let request = queue::TX.with_borrow_mut(|tx| tx.pop_front().unwrap());
        let options = dhcp::options(&request[42..]).unwrap();
        assert_eq!((options.kind,options.server,options.requested),(3,Some(wan().gateway),Some(wan().address)));
    }
}
