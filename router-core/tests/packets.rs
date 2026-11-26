use r5s_router_core::{*,nat::Router,dhcp::{Server,Reply}};
const CLIENT1: Ip = LAN[0] + 99;
const CLIENT2: Ip = LAN[1] + 99;
fn wan()->Wan {Wan {address:0x0a000132,mask:0xffffff00,gateway:0x0a000101,dns:[0x0a000101,0,0],dns_count:1,expires:10000000}}
fn udp(src:Ip,dst:Ip,sp:u16,dp:u16,data:&[u8],zero:bool)->Vec<u8> {
 let mut p=vec![0;28+data.len()];p[0]=0x45;let length=p.len();set16(&mut p,2,length as u16);p[8]=64;p[9]=17;setip(&mut p,12,src);setip(&mut p,16,dst);set16(&mut p,20,sp);set16(&mut p,22,dp);set16(&mut p,24,(8+data.len()) as u16);p[28..].copy_from_slice(data);let c=checksum(&p[..20]);set16(&mut p,10,c);if !zero {let c=transport_checksum(&p,length);set16(&mut p,26,if c==0 {65535}else{c});}p
}
#[test] fn two_lans_same_port_and_return_filter() {
 let mut router=Router::new(42);router.configure(Some(wan()),0).unwrap();
 let mut a=udp(CLIENT1,0x08080404,12345,443,b"odd",false);let mut b=udp(CLIENT2,0x08080404,12345,443,b"odd",false);
 assert_eq!(router.forward(Port::Lan1,&mut a,1,1500),Ok(Port::Wan));assert_eq!(router.forward(Port::Lan2,&mut b,1,1500),Ok(Port::Wan));
 let ap=u16at(&a,20);let bp=u16at(&b,20);assert_ne!(ap,bp);assert_eq!(transport_checksum(&a,a.len()),0);
 for (ext,private,port) in [(ap,CLIENT1,Port::Lan1),(bp,CLIENT2,Port::Lan2)] {
  let mut reply=udp(0x08080404,wan().address,443,ext,b"reply",false);assert_eq!(router.forward(Port::Wan,&mut reply,2,1500),Ok(port));assert_eq!(ipat(&reply,16),private);assert_eq!(u16at(&reply,22),12345);assert_eq!(transport_checksum(&reply,reply.len()),0);
  let mut unsolicited=udp(0x09090909,wan().address,443,ext,b"bad",false);assert_eq!(router.forward(Port::Wan,&mut unsolicited,3,1500),Err(Drop::Policy));
 }
 let mut inter=udp(CLIENT1,CLIENT2,100,200,b"x",true);assert_eq!(router.forward(Port::Lan1,&mut inter,4,1500),Err(Drop::Policy));
}
#[test] fn zero_checksum_expiry_and_generation() {
 let mut r=Router::new(3);r.configure(Some(wan()),0).unwrap();let mut p=udp(CLIENT1,0x01010101,200,53,b"abc",true);r.forward(Port::Lan1,&mut p,1,1500).unwrap();assert_eq!(u16at(&p,26),0);
 let ext=u16at(&p,20);r.expire(120002);let mut stale=udp(0x01010101,wan().address,53,ext,b"abc",true);assert_eq!(r.forward(Port::Wan,&mut stale,120003,1500),Err(Drop::Policy));
 let before=r.generation;let mut changed=wan();changed.address+=1;r.configure(Some(changed),120004).unwrap();assert_eq!(r.generation,before+1);assert_eq!(r.active(),0);
}
#[test] fn bounded_pressure_and_spoofing() {
 let mut r=Router::new(1);r.configure(Some(wan()),0).unwrap();
 for i in 0..16 {let mut p=udp(CLIENT1,0x01010101,200+i,53,b"x",false);r.forward(Port::Lan1,&mut p,1,1500).unwrap();}
 let mut p=udp(CLIENT1,0x01010101,1000,53,b"x",false);assert_eq!(r.forward(Port::Lan1,&mut p,1,1500),Err(Drop::Full));
 let mut p=udp(CLIENT2,0x01010101,1000,53,b"x",false);assert_eq!(r.forward(Port::Lan1,&mut p,1,1500),Err(Drop::Spoof));
 let mut bad=wan();bad.address=LAN[0]+253;bad.gateway=LAN[0]+1;assert_eq!(r.configure(Some(bad),1),Err(Drop::Overlap));
 assert_eq!(wan().next_hop(0x08080808),wan().gateway);assert_eq!(wan().next_hop(0x0a000188),0x0a000188);
}
#[test] fn related_icmp_from_intermediate_router() {
 let mut r=Router::new(11);r.configure(Some(wan()),0).unwrap();let mut outbound=udp(CLIENT1,0x08080404,1234,443,b"1234567",false);r.forward(Port::Lan1,&mut outbound,1,1500).unwrap();
 let mut p=vec![0;56];p[0]=0x45;set16(&mut p,2,56);p[8]=64;p[9]=1;setip(&mut p,12,0x0a000101);setip(&mut p,16,wan().address);p[20]=3;p[21]=4;set16(&mut p,26,1400);p[28..].copy_from_slice(&outbound[..28]);let c=checksum(&p[20..]);set16(&mut p,22,c);let c=checksum(&p[..20]);set16(&mut p,10,c);
 assert_eq!(r.forward(Port::Wan,&mut p,2,1500),Ok(Port::Lan1));assert_eq!(ipat(&p,16),CLIENT1);assert_eq!(ipat(&p,40),CLIENT1);assert_eq!(u16at(&p,48),1234);assert_eq!(checksum(&p[20..]),0);assert_eq!(checksum(&p[28..48]),0);
}
fn request(kind:u8,requested:Option<Ip>)->Vec<u8> {let mut p=vec![0;240];p[0]=1;p[1]=1;p[2]=6;p[28..34].copy_from_slice(&[2,1,2,3,4,5]);p[236..240].copy_from_slice(&[99,130,83,99]);p.extend_from_slice(&[53,1,kind]);if let Some(ip)=requested {p.extend_from_slice(&[50,4]);p.extend_from_slice(&ip.to_be_bytes());}p.push(255);p}
#[test] fn dhcp_probe_bind_renew_release_isolation() {
 let mut r=Router::new(1);let mut server=Server::new(Port::Lan1).unwrap();let d=request(1,None);
 assert_eq!(server.receive(Port::Wan,&d,0,&mut r),Err(Drop::Policy));assert_eq!(server.receive(Port::Lan1,&d,0,&mut r),Ok(Reply::Probe(CLIENT1)));
 assert_eq!(server.probe_result(CLIENT1,false,3000),Ok(Reply::Offer(CLIENT1)));
 let req=request(3,Some(CLIENT1));assert_eq!(server.receive(Port::Lan1,&req,3001,&mut r),Ok(Reply::Ack(CLIENT1)));
 let mut reply=[0;512];assert_eq!(server.response(&req,5,CLIENT1,&[0x0a000101],&mut reply),Ok(300));assert!(reply.windows(6).any(|b|b==[6,4,10,0,1,1]));
 let mut release=request(7,None);setip(&mut release,12,CLIENT1);assert_eq!(server.receive(Port::Lan1,&release,4000,&mut r),Ok(Reply::Released(CLIENT1)));assert_eq!(server.used(),0);
}
#[test] fn malformed_packets_and_fragments_do_not_allocate() {
 let mut r=Router::new(1);r.configure(Some(wan()),0).unwrap();let p=udp(CLIENT1,0x01010101,100,200,b"abc",false);
 for length in 0..p.len() {let mut short=p[..length].to_vec();assert!(r.forward(Port::Lan1,&mut short,1,1500).is_err());}assert_eq!(r.active(),0);
 let mut fragment=p.clone();set16(&mut fragment,6,0x2000);set16(&mut fragment,10,0);let c=checksum(&fragment[..20]);set16(&mut fragment,10,c);assert_eq!(r.forward(Port::Lan1,&mut fragment,1,1500),Err(Drop::Unsupported));assert_eq!(r.active(),0);
 let mut dhcp=request(1,None);dhcp.pop();dhcp.extend_from_slice(&[53,1,1,255]);assert!(dhcp::options(&dhcp).is_err());
 let mut ttl=p.clone();ttl[8]=1;set16(&mut ttl,10,0);let c=checksum(&ttl[..20]);set16(&mut ttl,10,c);assert_eq!(r.forward(Port::Lan1,&mut ttl,1,1500),Err(Drop::Ttl));
}
fn tcp(src:Ip,dst:Ip,sp:u16,dp:u16,seq:u32,ack:u32,flags:u8)->Vec<u8> {
 let mut p=vec![0;40];p[0]=0x45;set16(&mut p,2,40);p[8]=64;p[9]=6;setip(&mut p,12,src);setip(&mut p,16,dst);set16(&mut p,20,sp);set16(&mut p,22,dp);setip(&mut p,24,seq);setip(&mut p,28,ack);p[32]=0x50;p[33]=flags;set16(&mut p,34,16384);let c=checksum(&p[..20]);set16(&mut p,10,c);let c=transport_checksum(&p,40);set16(&mut p,36,c);p
}
#[test]
fn translated_tcp_pair_matches_ordered_single_packets_and_rejects_bad_checksums() {
    fn established() -> Router {
        let mut router = Router::new(7);
        router.configure(Some(wan()), 0).unwrap();
        let mut syn = tcp(CLIENT1, 0x08080808, 1234, 443, 100, 0, 2);
        router.forward(Port::Lan1, &mut syn, 1, 1500).unwrap();
        let mut reply = tcp(0x08080808, wan().address, 443, u16at(&syn, 20), 500, 101, 18);
        router.forward(Port::Wan, &mut reply, 2, 1500).unwrap();
        let mut ack = tcp(CLIENT1, 0x08080808, 1234, 443, 101, 501, 16);
        router.forward(Port::Lan1, &mut ack, 3, 1500).unwrap();
        router
    }
    let data = |sequence, flags| {
        let mut packet = tcp(CLIENT1, 0x08080808, 1234, 443, sequence, 501, flags);
        packet.extend_from_slice(&[0x7f; 64]);
        set16(&mut packet, 2, 104);
        set16(&mut packet, 10, 0);
        set16(&mut packet, 36, 0);
        let ip = checksum(&packet[..20]);
        let transport = transport_checksum(&packet, 104);
        set16(&mut packet, 10, ip);
        set16(&mut packet, 36, transport);
        packet
    };
    let [first, last] = [data(101, 16), data(165, 24)];
    let mut sequential = established();
    let mut expected = [first.clone(), last.clone()];
    for packet in &mut expected { sequential.forward(Port::Lan1, packet, 4, 1500).unwrap(); }
    let mut pair = established();
    let [mut a, mut b] = [first.clone(), last.clone()];
    assert_eq!(pair.forward_pair(Port::Lan1, [&mut a, &mut b], 4, 1500, [0; 2]), [Ok(Port::Wan); 2]);
    assert_eq!([a, b], expected);
    let [mut a, mut corrupt] = [first, last];
    corrupt[70] ^= 1;
    assert_eq!(established().forward_pair(Port::Lan1, [&mut a, &mut corrupt], 4, 1500, [0; 2]),
               [Ok(Port::Wan), Err(Drop::Checksum)]);
}
#[test] fn tcp_handshake_retransmit_and_unsolicited_ack() {
 let mut r=Router::new(7);r.configure(Some(wan()),0).unwrap();
 let mut bad=tcp(CLIENT1,0x08080808,1234,443,100,0,16);assert_eq!(r.forward(Port::Lan1,&mut bad,1,1500),Err(Drop::State));assert_eq!(r.active(),0);
 let syn=tcp(CLIENT1,0x08080808,1234,443,100,0,2);let mut p=syn.clone();r.forward(Port::Lan1,&mut p,2,1500).unwrap();let ext=u16at(&p,20);
 let mut retry=syn;r.forward(Port::Lan1,&mut retry,3,1500).unwrap();assert_eq!(u16at(&retry,20),ext);
 let mut incorrect=tcp(0x08080808,wan().address,443,ext,500,102,18);assert_eq!(r.forward(Port::Wan,&mut incorrect,4,1500),Err(Drop::State));
 let mut reply=tcp(0x08080808,wan().address,443,ext,500,101,18);assert_eq!(r.forward(Port::Wan,&mut reply,5,1500),Ok(Port::Lan1));assert_eq!(transport_checksum(&reply,40),0);
 let mut ack=tcp(CLIENT1,0x08080808,1234,443,101,501,16);r.forward(Port::Lan1,&mut ack,6,1500).unwrap();
 let mut fin=tcp(CLIENT1,0x08080808,1234,443,101,501,17);r.forward(Port::Lan1,&mut fin,7,1500).unwrap();r.expire(120008);assert_eq!(r.active(),0);
}
#[test] fn icmp_echo_id_and_checksum() {
 let mut r=Router::new(18);r.configure(Some(wan()),0).unwrap();
 let mut p=vec![0;31];p[0]=0x45;set16(&mut p,2,31);p[8]=64;p[9]=1;setip(&mut p,12,CLIENT2);setip(&mut p,16,0x08080808);p[20]=8;set16(&mut p,24,42);p[28..].copy_from_slice(b"odd");let c=checksum(&p[20..]);set16(&mut p,22,c);let c=checksum(&p[..20]);set16(&mut p,10,c);
 r.forward(Port::Lan2,&mut p,1,1500).unwrap();assert_ne!(u16at(&p,24),42);assert_eq!(checksum(&p[20..]),0);
 p[20]=0;setip(&mut p,12,0x08080808);setip(&mut p,16,wan().address);set16(&mut p,22,0);let c=checksum(&p[20..]);set16(&mut p,22,c);set16(&mut p,10,0);let c=checksum(&p[..20]);set16(&mut p,10,c);
 assert_eq!(r.forward(Port::Wan,&mut p,2,1500),Ok(Port::Lan2));assert_eq!(u16at(&p,24),42);assert_eq!(checksum(&p[20..]),0);
}
#[test] fn arp_conflict_probe_deadlines_and_decline_wire() {
 use r5s_router_core::arp::{Acd,Action};
 let own=[2,0,0,0,0,1];let other=[2,0,0,0,0,2];let mut acd=Acd::new(wan().address,0,100);
 assert_eq!(acd.tick(99),Action::None);assert_eq!(acd.tick(100),Action::Probe(wan().address));assert_eq!(acd.tick(1100),Action::Probe(wan().address));assert_eq!(acd.tick(2100),Action::Probe(wan().address));assert_eq!(acd.tick(4099),Action::None);assert_eq!(acd.tick(4100),Action::Bind(wan().address));assert_eq!(acd.tick(4100),Action::Announce(wan().address));assert_eq!(acd.tick(6100),Action::Announce(wan().address));assert_eq!(acd.tick(8100),Action::None);
 let mut frame=[0;60];arp::request(other,0,wan().address,&mut frame).unwrap();assert!(acd.conflict(&frame,own));assert!(!acd.conflict(&frame,other));
 let mut decline=[0;342];let n=dhcp::decline(own,wan().address,wan().gateway,123,&mut decline).unwrap();assert_eq!(n,342);assert_eq!(checksum(&decline[14..34]),0);assert_eq!(transport_checksum(&decline[14..],n-14),0);assert_eq!(dhcp::options(&decline[42..]).unwrap().kind,4);
}
#[test] fn parser_fuzz_is_bounded_and_does_not_create_wan_state() {
 let mut r=Router::new(9);r.configure(Some(wan()),0).unwrap();let mut seed=1u32;
 for length in 0..1600 {let mut p=vec![0;length];for b in &mut p {seed=seed.wrapping_mul(1664525).wrapping_add(1013904223);*b=(seed>>24) as u8;}
  let _=r.forward(Port::Wan,&mut p,1,1500);let _=dhcp::options(&p);let _=dhcp::reply_options(&p);let _=arp::parse(&p);
 }assert_eq!(r.active(),0);
}

#[test]
fn delayed_probe_cannot_offer_an_expired_reservation() {
    let mut router = Router::new(1);
    let mut server = Server::new(Port::Lan1).unwrap();
    let discover = request(1,None);
    assert_eq!(server.receive(Port::Lan1,&discover,0,&mut router),Ok(Reply::Probe(CLIENT1)));
    assert_eq!(server.receive(Port::Lan1,&discover,29000,&mut router),Ok(Reply::Probe(CLIENT1)));
    assert_eq!(server.probe_result(CLIENT1,false,30000),Err(Drop::State));
}

#[test]
fn selected_server_requires_offer_transaction() {
    let mut router=Router::new(123);
    let mut server=Server::new(Port::Lan2).unwrap();
    let mut discover=request(1,None);
    setip(&mut discover,4,777);
    let Reply::Probe(address)=server.receive(Port::Lan2,&discover,0,&mut router).unwrap() else {panic!()};
    assert_eq!(server.probe_result(address,false,1000),Ok(Reply::Offer(address)));
    let mut select=request(3,Some(address));
    select.pop();
    select.extend([54,4]);
    select.extend(LAN[1].to_be_bytes());
    select.push(255);
    assert_eq!(server.receive(Port::Lan2,&select,1001,&mut router),Ok(Reply::None));
    setip(&mut select,4,777);
    assert_eq!(server.receive(Port::Lan2,&select,1002,&mut router),Ok(Reply::Ack(address)));
}

#[cfg(feature = "shared-nat")]
#[test]
fn parallel_shards_steer_both_directions_and_isolate_expiration() {
    use r5s_router_core::steering::{self, Steering, WORKERS};
    let peer = 0x08080808;
    let mut steering = Steering::new();
    let syn = tcp(CLIENT1,peer,1234,443,100,0,2);
    let chosen = |worker| if worker==3 {0} else {8};
    assert_eq!(steering.outbound(&syn, |_| u32::MAX),None);
    assert_eq!(steering.outbound(&syn,chosen),Some(3));
    assert_eq!(steering.outbound(&syn,|n| if n==3 {8} else {0}),Some(3));
    let mut uses = [0; WORKERS];
    for port in 2000..2064 {
        let packet=udp(CLIENT1,peer,port,53,b"x",false);
        let owner=steering.outbound(&packet,|n| uses[n]).unwrap();
        uses[owner]+=1;
    }
    assert!(uses.iter().all(|&count|count>0));

    let external: Vec<_> = std::thread::scope(|scope| {
        (0..WORKERS).map(|owner| scope.spawn(move || {
            let mut router=Router::sharded(7,owner as u8);
            router.configure(Some(wan()),0).unwrap();
            let mut request=udp(CLIENT1,peer,12345,53,b"query",false);
            assert_eq!(router.forward(Port::Lan1,&mut request,1,1500),Ok(Port::Wan));
            let external=u16at(&request,20);
            let mut reply=udp(peer,wan().address,53,external,b"reply",false);
            assert_eq!(steering::inbound(&reply),Some(owner));
            assert_eq!(router.forward(Port::Wan,&mut reply,2,1500),Ok(Port::Lan1));
            assert_eq!(ipat(&reply,16),CLIENT1);
            let mut expired=udp(peer,wan().address,53,external,b"reply",false);
            assert_eq!(router.forward(Port::Wan,&mut expired,30_002,1500),Err(Drop::Policy));
            assert_eq!(router.active(),0);

            let mut opening=tcp(CLIENT1,peer,1234,443,100,0,2);
            router.forward(Port::Lan1,&mut opening,30_003,1500).unwrap();
            let tcp_port=u16at(&opening,20);
            let mut response=tcp(peer,wan().address,443,tcp_port,500,101,18);
            assert_eq!(steering::inbound(&response),Some(owner));
            assert_eq!(router.forward(Port::Wan,&mut response,30_004,1500),Ok(Port::Lan1));
            router.release_client(Port::Lan1,CLIENT1,30_005);
            assert_eq!(router.active(),0);
            external
        })).collect::<Vec<_>>().into_iter().map(|join|join.join().unwrap()).collect()
    });
    assert_eq!(external.iter().collect::<std::collections::BTreeSet<_>>().len(),WORKERS);
    let mut owner=Router::sharded(42,3);
    owner.configure(Some(wan()),0).unwrap();
    let mut translated=syn.clone();
    owner.forward(Port::Lan1,&mut translated,1,1500).unwrap();
    steering.release_idle(3,&owner.active_endpoints(1));
    assert_eq!(steering.outbound(&syn,|n| if n==1 {0}else{8}),Some(3));
    owner.expire(600_001);
    steering.release_idle(3,&owner.active_endpoints(600_001));
    assert_eq!(steering.outbound(&syn,|n| if n==1 {0}else{8}),Some(1));

    for port in 1..=128 {
        let mut query=udp(CLIENT1,peer,port,53,b"x",false);
        owner.forward(Port::Lan1,&mut query,610_000,1500).unwrap();
    }
    let mut overflow=udp(CLIENT1,peer,129,53,b"x",false);
    assert_eq!(owner.forward(Port::Lan1,&mut overflow,610_001,1500),Err(Drop::NatSlots));
}

#[test]
fn incremental_nat_matches_full_checksum_on_odd_and_maximum_payloads() {
    let mut router=Router::new(23);
    let config=wan();
    let peer=0x08080808;
    router.configure(Some(config),0).unwrap();
    for (n,size) in [0,1,2,17,511,1471,1472].into_iter().enumerate() {
        let payload=vec![0xff-size as u8;size];
        let local=12000+n as u16;
        let mut request=udp(CLIENT1,peer,local,53,&payload,false);
        router.forward(Port::Lan1,&mut request,1,1500).unwrap();
        assert_eq!(validate_ip(&request),Ok(request.len()));
        assert_eq!(request[8],63);
        assert_eq!(transport_checksum(&request,request.len()),0);
        let mut reply=udp(peer,config.address,53,u16at(&request,20),&payload,false);
        router.forward(Port::Wan,&mut reply,2,1500).unwrap();
        assert_eq!(validate_ip(&reply),Ok(reply.len()));
        assert_eq!(transport_checksum(&reply,reply.len()),0);
        assert_eq!(u16at(&reply,22),local);
        assert_eq!(&reply[28..],payload.as_slice());
        let mut damaged=udp(CLIENT1,peer,local,53,&payload,false);
        damaged[27]^=1;
        assert_eq!(router.forward(Port::Lan1,&mut damaged,3,1500),Err(Drop::Checksum));
    }
}


#[test]
fn rx_checksum_provenance_must_match_the_transport_protocol() {
    let mut router=Router::new(7);
    router.configure(Some(wan()),0).unwrap();
    let mut frame=udp(CLIENT1,0x08080808,1234,53,b"rx provenance",false);
    frame[28]^=1;
    assert_eq!(router.forward_with_rx_checksum(Port::Lan1,&mut frame,1,1500,6),Err(Drop::Checksum));
    assert_eq!(router.forward_with_rx_checksum(Port::Lan1,&mut frame,1,1500,0),Err(Drop::Checksum));
    frame[10]^=1;
    assert_eq!(router.forward_with_rx_checksum(Port::Lan1,&mut frame,1,1500,17),Err(Drop::Checksum));
    assert_eq!(router.active(),0);
}

#[cfg(feature = "shared-nat")]
#[test]
fn sharded_reverse_lookup_rejects_expiry_beyond_maintenance_slice() {
    let mut router = Router::sharded(42, 2);
    router.configure(Some(wan()), 0).unwrap();
    let mut last_external = 0;
    for local in 2000..2040 {
        let mut frame = udp(CLIENT1, 0x08080808, local, 443, b"test", false);
        assert_eq!(router.forward(Port::Lan1, &mut frame, 1, 1500), Ok(Port::Wan));
        last_external = u16at(&frame, 20);
        assert_eq!(last_external % 4, 2);
    }
    let mut reply = udp(0x08080808, wan().address, 443, last_external, b"test", false);
    assert_eq!(router.forward(Port::Wan, &mut reply, 30_002, 1500), Err(Drop::Policy));
    assert_eq!(router.active(), 0);
    for _ in 0..16 { router.maintain(30_002); }
    assert_eq!(router.active(), 0);
}
