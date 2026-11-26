use r5s_router_core::{dhcp::{Server,Reply,LEASE_SECONDS},nat::Router,*};

fn request(kind:u8,id:u8,address:Ip) -> Vec<u8> {
    let mut p=vec![0;240];
    p[..3].copy_from_slice(&[1,1,6]);
    p[28..34].copy_from_slice(&[2,0,0,0,1,id]);
    p[236..240].copy_from_slice(&[99,130,83,99]);
    p.extend([53,1,kind,61,3,1,1,id]);
    if address!=0 {p.extend([50,4]);p.extend(address.to_be_bytes());}
    p.push(255);
    p
}

#[test]
fn exhaust_both_pools_then_expire_without_crossing_interfaces() {
    let mut r=Router::new(42);
    let mut servers=[Server::new(Port::Lan1).unwrap(),Server::new(Port::Lan2).unwrap()];
    for (i,port) in [Port::Lan1,Port::Lan2].into_iter().enumerate() {
        for n in 0..100 {
            let address=LAN[i]+99+u32::from(n);
            let s=&mut servers[i];
            assert_eq!(s.receive(port,&request(1,n,0),0,&mut r),Ok(Reply::Probe(address)));
            assert_eq!(s.probe_result(address,false,1000),Ok(Reply::Offer(address)));
            assert_eq!(s.receive(port,&request(3,n,address),1001,&mut r),Ok(Reply::Ack(address)));
        }
        assert_eq!(servers[i].receive(port,&request(1,101,0),1002,&mut r),Err(Drop::Full));
    }
    let expiry=1001+1000*u64::from(LEASE_SECONDS);
    servers[0].expire(expiry-1,&mut r);
    assert_eq!(servers[0].used(),100);
    servers[0].expire(expiry,&mut r);
    assert_eq!((servers[0].used(),servers[1].used()),(0,100));
    assert_eq!(servers[0].receive(Port::Lan1,&request(3,7,LAN[0]+106),expiry,&mut r),Ok(Reply::None));
}
