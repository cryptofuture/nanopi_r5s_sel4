#[path = "../pds/gmac_dma_corrected/src/tso.rs"]
mod tso;
#[path = "../pds/gmac_dma_corrected/src/tso_testgen.rs"]
mod tso_testgen;
#[path = "../pds/gmac_dma_corrected/src/zc.rs"]
mod zc;

use r5s_router_core::{LAN, Port, Wan, bridge::{Bridge, MAC}, checksum, nat::Router, set16, setip, transport_checksum, u16at, validate_ip};
use std::{hint::black_box, sync::atomic::AtomicU32, time::Instant};

const COUNT: usize = 100_000;
const FLOW_COUNT: usize = 128;

fn packet(slot: usize) -> [u8; 1500] {
    let mut packet = [0x8f; 1500];
    packet[..28].fill(0);
    packet[0] = 0x45;
    packet[8] = 64;
    packet[9] = 17;
    set16(&mut packet, 2, 1500);
    setip(&mut packet, 12, LAN[0] + 2 + slot as u32);
    setip(&mut packet, 16, 0x08080808);
    set16(&mut packet, 20, 1234);
    set16(&mut packet, 22, 4321);
    set16(&mut packet, 24, 1480);
    let ip = checksum(&packet[..20]);
    set16(&mut packet, 10, ip);
    let transport = transport_checksum(&packet, 1500);
    set16(&mut packet, 26, if transport == 0 { u16::MAX } else { transport });
    packet
}

fn measure(name: &str, count: usize, bytes: usize, alloc: usize, queue: usize, run: impl FnOnce()) {
    let start = Instant::now();
    run();
    let seconds = start.elapsed().as_secs_f64();
    println!("{name},{:.1},{:.0},{:.0},{alloc},{queue}",
             seconds * 1e9 / count as f64, count as f64 / seconds, (count * bytes) as f64 / seconds);
}

fn tcp(packet: &mut [u8], source: u32, dest: u32, ports: [u16; 2], seq: u32, ack: u32, flags: u8) {
    packet[..40].fill(0);
    packet[0] = 0x45;
    set16(packet, 2, packet.len() as u16);
    packet[8..10].copy_from_slice(&[64, 6]);
    setip(packet, 12, source);
    setip(packet, 16, dest);
    set16(packet, 20, ports[0]);
    set16(packet, 22, ports[1]);
    setip(packet, 24, seq);
    setip(packet, 28, ack);
    packet[32..34].copy_from_slice(&[0x50, flags]);
    let header = checksum(&packet[..20]);
    set16(packet, 10, header);
    let transport = transport_checksum(packet, packet.len());
    set16(packet, 36, transport);
}

fn tcp_pairs(wan: Wan) {
    let peer = 0x08080808;
    let ports = [1234, 443];
    let mut router = Router::sharded(42, 0);
    router.configure(Some(wan), 0).unwrap();
    let pairs: Vec<_> = (0..FLOW_COUNT).map(|slot| {
        let client = LAN[0] + 2 + slot as u32;
        let mut setup = [0; 40];
        tcp(&mut setup, client, peer, ports, 100, 0, 2);
        router.forward(Port::Lan1, &mut setup, 3, 1500).unwrap();
        let external = u16at(&setup, 20);
        tcp(&mut setup, peer, wan.address, [ports[1], external], 500, 101, 18);
        router.forward(Port::Wan, &mut setup, 3, 1500).unwrap();
        tcp(&mut setup, client, peer, ports, 101, 501, 16);
        router.forward(Port::Lan1, &mut setup, 3, 1500).unwrap();
        let mut first = [0x71; 1500];
        let mut last = [0x92; 1500];
        tcp(&mut first, client, peer, ports, 101, 501, 16);
        tcp(&mut last, client, peer, ports, 1561, 501, 24);
        (first, last)
    }).collect();
    measure("nat_tcp_pairs", COUNT, 1500, 0, 0, || {
        for index in 0..COUNT / 2 {
            let (mut a, mut b) = *black_box(&pairs[index % FLOW_COUNT]);
            assert_eq!(router.forward_pair(Port::Lan1, [&mut a, &mut b], 4, 1500, [6, 6]), [Ok(Port::Wan); 2]);
            black_box((&a, &b));
        }
    });
}

fn main() {
    let mut router = Router::sharded(42, 0);
    let wan = Wan { address: 0x0a000132, mask: 0xffffff00, gateway: 0x0a000101,
                    dns: [0x0a000101, 0, 0], dns_count: 1, expires: 10_000_000 };
    router.configure(Some(wan), 0).unwrap();
    let packets: Vec<_> = (0..FLOW_COUNT).map(packet).collect();
    println!("nat_hash={}", std::env::args().nth(1).unwrap());
    println!("bridge_local_hash={}", std::env::args().nth(2).unwrap());
    println!("nat_pair_lookup={}", std::env::args().nth(3).unwrap());
    println!("case,ns_per_packet,packets_per_second,bytes_per_second,allocations_per_packet,queue_ops_per_packet");
    let mut bridge = Bridge::new();
    let mut ethernet = [0; 1514];
    ethernet[..6].copy_from_slice(&MAC);
    ethernet[6..12].copy_from_slice(&[2, 1, 2, 3, 4, 5]);
    ethernet[12..14].copy_from_slice(&[8, 0]);
    ethernet[14..].copy_from_slice(&packets[0]);
    measure("bridge_local_unicast", COUNT, ethernet.len(), 0, 0, || {
        for now in 0..COUNT {
            assert_eq!(bridge.receive(0, black_box(&ethernet), 3, now as u64), Some((true, 0)));
        }
    });
    measure("nat_new_flow", FLOW_COUNT, 1500, 1, 0, || {
        for packet in &packets {
            let mut frame = *packet;
            assert_eq!(router.forward_with_rx_checksum(Port::Lan1, &mut frame, 1, 1500, 17), Ok(Port::Wan));
            black_box(frame);
        }
    });
    for batch in [1, 8, 16, 32, 64] {
        measure(&format!("nat_hit_batch_{batch}"), COUNT, 1500, 0, 0, || {
            for i in (0..COUNT).step_by(batch) {
                for n in i..(i + batch).min(COUNT) {
                    let mut frame = packets[n % FLOW_COUNT];
                    black_box(router.forward_with_rx_checksum(Port::Lan1, &mut frame, 2, 1500, 17)).unwrap();
                    black_box(frame);
                }
            }
        });
    }
    tcp_pairs(wan);
    measure("ipv4_parse", COUNT, 1500, 0, 0, || {
        for n in 0..COUNT {
            assert_eq!(validate_ip(black_box(&packets[n % FLOW_COUNT])), Ok(1500));
        }
    });
    measure("transport_checksum", COUNT, 1500, 0, 0, || {
        for n in 0..COUNT {
            assert_eq!(transport_checksum(black_box(&packets[n % FLOW_COUNT]), 1500), 0);
        }
    });
    let memory = [const { AtomicU32::new(0) }; zc::META_BYTES / 4];
    let base = memory.as_ptr() as usize;
    let queue = base + 0x1000;
    zc::init(base);
    measure("pool_lifecycle", COUNT, zc::BUFFER, 1, 0, || {
        for _ in 0..COUNT {
            let reference = zc::alloc(base, zc::PARTITION_WAN).unwrap();
            zc::publish(base, &reference).unwrap();
            zc::submit(base, &reference).unwrap();
            zc::complete(base, &reference).unwrap();
        }
    });
    let reference = zc::Ref::new(0, 1);
    measure("ref_queue_roundtrip", COUNT, 16, 0, 2, || {
        for _ in 0..COUNT {
            assert_eq!(zc::RefQueue::push(queue, black_box(&reference)), Ok(true));
            assert_eq!(zc::RefQueue::pop(queue), Ok(Some(reference)));
        }
    });
    let mut first = [0u8; 1514];
    first[12..15].copy_from_slice(&[8, 0, 0x45]);
    first[16..18].copy_from_slice(&1500u16.to_be_bytes());
    first[20..24].copy_from_slice(&[0x40, 0, 63, 6]);
    first[46..48].copy_from_slice(&[0x50, 0x10]);
    let mut last = first;
    last[18..20].copy_from_slice(&1u16.to_be_bytes());
    last[38..42].copy_from_slice(&1460u32.to_be_bytes());
    last[47] = 0x18;
    measure("gro_pair_qualification", COUNT, 2 * 1514, 0, 0, || {
        for _ in 0..COUNT {
            assert_eq!(tso::gro_pair(black_box(&first), black_box(&last)), Some(1460));
        }
    });
    #[repr(align(64))]
    struct Aligned([u8; 16384]);
    let mut payload = Aligned([0; 16384]);
    let address = payload.0.as_ptr() as u64;
    let mut fragments: Vec<_> = payload.0.chunks_exact_mut(zc::BUFFER).collect();
    let addresses: Vec<_> = (0..fragments.len()).map(|i| address + (i * zc::BUFFER) as u64).collect();
    let specification = tso_testgen::TestSpec::default_ipv4(1);
    let request = tso_testgen::build(&specification, &mut fragments, &addresses).unwrap();
    let mut scratch = [0u8; 2161];
    measure("software_segmentation_request", 1000, 16_000, 0, 0, || {
        for _ in 0..1000 {
            assert_eq!(tso::segment(&request, &mut scratch, &mut |segment| {
                black_box(segment);
                Ok(())
            }), Ok(11));
        }
    });
}
