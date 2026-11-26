#![no_std]
#![no_main]

mod queue;
#[cfg(feature = "zc")]
mod zc;
#[cfg(feature = "gro")]
mod tso;
#[cfg(feature = "forward-workers")]
mod forward_shared;
#[cfg(feature = "forward-workers")]
mod forward_dispatch;
#[cfg(feature = "shared-wan")]
mod shared;
#[cfg(feature = "lan-b51-udp")]
mod b51_udp;
#[cfg(feature = "lan-b51-udp")]
mod network_snapshot;
use core::sync::atomic::{AtomicU32, Ordering};
use r5s_router_core::{Port, bridge::Bridge, ethernet::EthernetRouter, u16at};
use sel4_microkit::{Handler, Infallible, protection_domain};

include!("lan_profile.rs");
const BASE: [usize; 2] = [0x63000000, 0x63008000];

fn stat(index: usize) -> &'static AtomicU32 {
    unsafe { &*((0x41200000 as *const AtomicU32).add(index)) }
}

#[cfg(feature = "shared-wan")]
fn echo_trace(stage: usize, frame: &[u8], outcome: u32, ports: u8) {
    use r5s_router_core::ipat;
    if frame.len() < 42 || frame[12..14] != [8, 0] || frame[14] != 0x45 || frame[23] != 1 {
        return;
    }
    let (source, destination) = (ipat(frame, 26), ipat(frame, 30));
    let request = stage < 2;
    if if request { frame[34] != 8 || destination != 0x08080808 }
       else { frame[34] != 0 || source != 0x08080808 } { return; }
    let base = 22 + stage * 7;
    stat(21).fetch_add(1, Ordering::AcqRel);
    stat(base).fetch_add(1, Ordering::Relaxed);
    for (i, value) in [source, destination, ipat(frame, 38),
        outcome | (u32::from(ports) << 8), ipat(frame, 0), u32::from(u16at(frame, 4))]
        .into_iter().enumerate()
    {
        stat(base + 1 + i).store(value, Ordering::Relaxed);
    }
    stat(21).fetch_add(1, Ordering::Release);
}

#[cfg(feature = "shared-wan")]
fn echo_outcome(result: &Result<Option<(Port, usize)>, r5s_router_core::Drop>, response: &[u8]) -> u32 {
    match result {
        Ok(Some(_)) if response[12..14] == [8, 6] => 3,
        Ok(Some(_)) => 1,
        Ok(None) => 2,
        Err(error) => 16 + *error as u32,
    }
}

/// Copy-fallback egress with an optional download-fallback return token: a
/// tokened frame's copies each carry the stamp, and the armed tally is
/// extended so the client returns to its direct lane only after every copy
/// completes. A copy whose marker cannot be queued retires the stamp at once.
fn send(mask: u8, frame: &[u8], forwarded: bool, token: Option<u32>, _control: bool) {
    #[cfg(feature = "zc")]
    if let Some(token) = token {
        zc::extend_lan_token(zc::META_VA, token, u32::from(mask.count_ones()));
    }
    #[cfg(feature = "shared-wan")]
    let mut accepted_ports = 0;
    for (port, base) in BASE.into_iter().enumerate() {
        if mask & (1 << port) == 0 { continue; }
        let copy = || queue::push(base + queue::DIRECTION, 0, frame);
        #[cfg(feature = "zc")]
        let result = match token {
            Some(token) => zc::RefQueue::push_copy_token(zc::link_queue(zc::LINKS_VA,
                if port == 0 { zc::Q_LAN1_TX } else { zc::Q_LAN2_TX }), token, copy),
            None if _control => queue::push(zc::lan_control_tx(port), 0, frame),
            None => zc::RefQueue::push_copy(zc::link_queue(zc::LINKS_VA,
                if port == 0 { zc::Q_LAN1_TX } else { zc::Q_LAN2_TX }), copy),
        };
        #[cfg(not(feature = "zc"))]
        let result = copy();
        let accepted = result.unwrap_or(false);
        #[cfg(feature = "zc")]
        if !accepted && let Some(token) = token {
            zc::retire_lan_token(zc::META_VA, token);
        }
        #[cfg(feature = "shared-wan")]
        if accepted { accepted_ports |= 1 << port; }
        let counter = if !accepted { 2 } else if forwarded { 0 } else { 1 };
        stat(port * 4 + counter).fetch_add(1, Ordering::Relaxed);
    }
    #[cfg(feature = "zc")]
    {
        zc::count(zc::META_VA, zc::C_COPIED_PACKETS);
        zc::add_copied_bytes(zc::META_VA, frame.len() as u32);
    }
    #[cfg(feature = "shared-wan")]
    if !forwarded {
        echo_trace(3, frame, if accepted_ports != 0 { 1 } else if mask == 0 { 5 } else { 6 }, accepted_ports);
    }
}

/// Zero-copy LAN egress: exactly one egress port hands over the pool
/// reference. Flooding and full reference queues fall back to frame copies
/// and release the buffer. A download-fallback return token transfers to the
/// copies (or the reference) and is retired only when that send is finished.
#[cfg(feature = "zc")]
fn zc_send(mask: u8, r: &zc::Ref, frame: &[u8], forwarded: bool) {
    let token = (r.flags & zc::FLAG_LAN_TOKEN != 0).then_some(r.meta);
    if mask.count_ones() == 1 {
        let port = mask.trailing_zeros() as usize;
        let queue = zc::link_queue(zc::LINKS_VA,
            if port == 0 { zc::Q_LAN1_TX } else { zc::Q_LAN2_TX });
        if zc::RefQueue::push(queue, r) == Ok(true) {
            stat(port * 4 + usize::from(!forwarded)).fetch_add(1, Ordering::Relaxed);
            return;
        }
        zc::count(zc::META_VA, zc::C_FALLBACK_QUEUE);
    } else if mask != 0 {
        zc::count(zc::META_VA, zc::C_FALLBACK_LAYOUT);
    }
    if mask != 0 {
        send(mask, frame, forwarded, token, false);
    } else {
        zc::count(zc::META_VA, zc::C_DROPPED);
        if let Some(token) = token {
            zc::retire_lan_token(zc::META_VA, token);
        }
    }
    let _ = zc::reject(zc::META_VA, r);
}

fn local_reply(bridge: &Bridge, links: u8, frame: &[u8], now: u64) {
    let kind = if frame[12..14] == [8, 6] { 8 }
               else if frame[23] == 1 { 9 }
               else { match frame.get(282..285) { Some([53, 1, 2]) => 10,
                      Some([53, 1, 5]) => 11, Some([53, 1, 6]) => 12, _ => 13 } };
    stat(kind).fetch_add(1, Ordering::Relaxed);
    send(bridge.destination(&frame[..6], links, now), frame, false, None, true);
}

#[cfg(feature = "shared-wan")]
fn send_wan(stack: &mut EthernetRouter, epoch: Option<u32>, frame: &[u8]) {
    let queued = epoch.is_some_and(|epoch| {
        #[cfg(feature = "zc")]
        if frame.get(12..14) == Some(&[8, 6]) {
            let queued = queue::push(zc::WAN_NEIGHBOR_TX, epoch, frame) == Ok(true);
            if queued && option_env!("R5S_GMAC_HYBRID_IRQ") == Some("1") {
                sel4_microkit::Channel::new(zc::WAN_NEIGHBOR_TX_CHANNEL).notify();
            }
            return queued;
        }
        #[cfg(feature = "zc")]
        let (refs, frames) = if zc::wan_fast_path() && zc::wan_fast_packet(frame, false) {
            (zc::FAST_WAN_TX, zc::FAST_WAN_FRAMES)
        } else {(zc::link_queue(zc::LINKS_VA, zc::Q_WAN_EG), 0x64000000)};
        #[cfg(not(feature = "zc"))]
        let frames = 0x64000000;
        let copy = || queue::push(frames + queue::DIRECTION, epoch, frame);
        #[cfg(feature = "zc")]
        let result = zc::RefQueue::push_copy(refs, copy);
        #[cfg(not(feature = "zc"))]
        let result = copy();
        result == Ok(true)
    });
    stat(if queued { 18 } else { 20 }).fetch_add(1, Ordering::Relaxed);
    echo_trace(1, frame, if queued { 1 } else if epoch.is_none() { 7 } else { 6 }, 0);
    #[cfg(feature = "zc")]
    {
        zc::count(zc::META_VA, zc::C_COPIED_PACKETS);
        zc::add_copied_bytes(zc::META_VA, frame.len() as u32);
    }
    if !queued { stack.transmission_failed(Port::Wan, frame); }
}

/// Zero-copy WAN egress: the reference travels to the WAN application and the
/// GMAC driver with the link epoch. Failure releases the buffer and reports
/// the transmission exactly like the copy path.
#[cfg(all(feature = "shared-wan", feature = "zc"))]
fn zc_send_wan(stack: &mut EthernetRouter, epoch: Option<u32>, r: &zc::Ref, frame: &[u8]) {
    echo_trace(1, frame, if epoch.is_some() { 1 } else { 7 }, 0);
    // A download-fallback token is settled at the dispatch decision: this
    // frame will never complete on a LAN queue, so it cannot be overtaken.
    if r.flags & zc::FLAG_LAN_TOKEN != 0 {
        zc::retire_lan_token(zc::META_VA, r.meta);
    }
    let queued = epoch.is_some_and(|epoch| {
        let mut request = *r;
        request.epoch = epoch;
        let refs = if zc::wan_fast_path() && zc::wan_fast_packet(frame, false) {zc::FAST_WAN_TX}
            else {zc::link_queue(zc::LINKS_VA, zc::Q_WAN_EG)};
        zc::RefQueue::push(refs, &request) == Ok(true)
    });
    stat(if queued { 18 } else { 20 }).fetch_add(1, Ordering::Relaxed);
    if queued {
        return;
    }
    if epoch.is_some() {
        zc::count(zc::META_VA, zc::C_FALLBACK_QUEUE);
    }
    stack.transmission_failed(Port::Wan, frame);
    let _ = zc::reject(zc::META_VA, r);
}

/// Outcome of one zero-copy LAN ingress reference.
#[cfg(all(feature = "zc", feature = "forward-workers"))]
enum RefOutcome {
    /// The reference was forwarded, flooded or released; nothing else to do.
    Handled,
    /// The frame was copied into the stack scratch for local processing.
    Copied(usize),
}

/// Zero-copy LAN ingress: classification reads the pool buffer in place.
/// Forwarding and single-port bridge switching keep the reference; local
/// delivery, flooding and status replies that need the stack copy first.
#[cfg(all(feature = "zc", feature = "forward-workers"))]
fn lan_ref(port: usize, r: zc::Ref, bridge: &mut Bridge, links: u8,
           now: u64, frame: &mut [u8; 1514], next_report_ms: &mut u64,
           workers: &mut forward_dispatch::Dispatch) -> RefOutcome {
    let length = r.len as usize;
    if zc::status(zc::META_VA, r.id()) != (zc::CPU, r.generation()) {
        zc::count(zc::META_VA, zc::C_OWNERSHIP_ERRORS);
        return RefOutcome::Handled;
    }
    if !(14..=1514).contains(&length) {
        let _ = zc::reject(zc::META_VA, &r);
        return RefOutcome::Handled;
    }
    let buffer = unsafe {
        core::slice::from_raw_parts_mut(
            (zc::POOL_VA + r.id() as usize * zc::BUFFER) as *mut u8, zc::BUFFER)
    };
    let Some((local, egress)) = bridge.receive(port, &buffer[..length], links, now) else {
        #[cfg(feature = "shared-wan")]
        echo_trace(0, &buffer[..length], 4, 1 << port);
        let _ = zc::release_cpu(zc::META_VA, &r);
        return RefOutcome::Handled;
    };
    if BRIDGE && egress != 0 {
        if local || egress.count_ones() != 1 {
            // Flooded frames and broadcast delivery to the stack both need
            // more than one consumer: copy and release the buffer.
            zc::count(zc::META_VA, zc::C_FALLBACK_LAYOUT);
            frame[..length].copy_from_slice(&buffer[..length]);
            zc::count(zc::META_VA, zc::C_COPIED_PACKETS);
            zc::add_copied_bytes(zc::META_VA, length as u32);
            let _ = zc::release_cpu(zc::META_VA, &r);
            return RefOutcome::Copied(length);
        }
        zc_send(egress, &r, &buffer[..length], true);
        return RefOutcome::Handled;
    }
    if !local || !IP {
        #[cfg(feature = "shared-wan")]
        echo_trace(0, &buffer[..length], 4, 1 << port);
        let _ = zc::release_cpu(zc::META_VA, &r);
        return RefOutcome::Handled;
    }
    #[cfg(feature = "lan-b51-udp")]
    if now >= *next_report_ms && b51_udp::reply(&mut buffer[..length], r5s_router_core::bridge::MAC,
        r5s_router_core::LAN[0], 0xffffff00, || network_snapshot::read(u32::from(PORT_MASK), true))
    {
        *next_report_ms = now.saturating_add(250);
        let length = b51_udp::frame_length(buffer);
        send(1 << port, &buffer[..length], false, None, true);
        let _ = zc::release_cpu(zc::META_VA, &r);
        return RefOutcome::Handled;
    }
    let input = &buffer[..length];
    let dhcp = length >= 42 && input[12..14] == [8, 0]
        && input[23] == 17 && u16at(input, 34) == 68 && u16at(input, 36) == 67;
    if dhcp && !DHCP {
        let _ = zc::release_cpu(zc::META_VA, &r);
        return RefOutcome::Handled;
    }
    #[cfg(feature = "forward-workers")]
    if input[12..14] == [8, 0] && !dhcp && length >= 34
        && r5s_router_core::ipat(input, 30) != r5s_router_core::LAN[0]
    {
        workers.dispatch_ref(Port::Lan1, &r, input, 1 << port);
        return RefOutcome::Handled;
    }
    frame[..length].copy_from_slice(input);
    zc::count(zc::META_VA, zc::C_COPIED_PACKETS);
    zc::add_copied_bytes(zc::META_VA, length as u32);
    let _ = zc::release_cpu(zc::META_VA, &r);
    RefOutcome::Copied(length)
}

#[protection_domain]
fn init() -> impl Handler {
    let mut bridge = Bridge::new();
    #[cfg(not(feature = "shared-wan"))]
    let mut stack = EthernetRouter::local_lan_dns(0x5235, &DNS[..DNS_COUNT]);
    #[cfg(feature = "shared-wan")]
    let mut stack = EthernetRouter::shared_lan_router_dns(shared::millis() as u32, &DNS[..DNS_COUNT]);
    #[cfg(feature = "shared-wan")]
    let mut upstream = None;
    #[cfg(feature = "forward-workers")]
    let mut workers = {
        stack.track_nat_releases();
        forward_dispatch::Dispatch::new()
    };
    #[cfg(feature = "shared-wan")]
    stat(21).store(2, Ordering::Release);
    // Declared unconditionally: the LAN status report timer is feature-gated
    // at its use sites but threaded through the per-frame call unconditionally.
    let mut next_report_ms = 0;
    let mut frame = [0; 1514];
    let mut response = [0; 1514];
    let mut previous = 0;
    let mut tick = 0;
    loop {
        let mut processed = false;
        let mut ticks: u64;
        unsafe { core::arch::asm!("isb", "mrs {}, cntvct_el0", out(reg) ticks, options(nostack)); }
        let now = ticks / 24_000;
        let mut links = 0;
        for port in 0..2 {
            if PORT_MASK & (1 << port) == 0 { continue; }
            let address = 0x41000000 + port * 0x100000 + 32 * 4;
            if unsafe { &*(address as *const AtomicU32) }.load(Ordering::Acquire) == 1 {
                links |= 1 << port;
            } else if previous & (1 << port) != 0 {
                bridge.link_down(port);
            }
        }
        previous = links;
        #[cfg(feature = "shared-wan")]
        {
            if now >= tick {
                if let Some((epoch, lease)) = wan_lease() {
                    if upstream.is_some_and(|(previous, _)| previous != epoch) {
                        stack.link_down(Port::Wan, now);
                    }
                    if stack.configure_wan(lease, now).is_ok() && lease.is_some() {
                        upstream = Some((epoch, now.saturating_add(100)));
                    } else { upstream = None; }
                } else if upstream.is_some_and(|(_, until)| now >= until) {
                    stack.link_down(Port::Wan, now);
                    upstream = None;
                }
            }
            #[cfg(feature = "forward-workers")]
            {
                workers.update(&mut stack, upstream.map(|(epoch, _)| epoch));
                #[cfg(feature = "zc")]
                workers.update_wan_route(&mut stack, upstream.map(|(epoch, _)| epoch), now, &mut response);
                processed |= workers.complete(&mut stack, &mut bridge, links, upstream.map(|v| v.0), now, &mut response);
            }
            if let Some((epoch, _)) = upstream {
                for _ in 0..queue::batch(option_env!("R5S_DATAPATH_BATCH"), 4) {
                    #[cfg(all(feature = "zc", feature = "forward-workers"))]
                    {
                        let Ok(Some(r)) = zc::RefQueue::pop(zc::link_queue(zc::LINKS_VA, zc::Q_APP_FWD))
                        else { break };
                        processed = true;
                        if r.flags & zc::FLAG_COPY == 0 {
                            let length = usize::from(r.len);
                            if r.epoch != epoch || !(34..=1514).contains(&length)
                                || zc::status(zc::META_VA, r.id()) != (zc::CPU, r.generation()) {
                                let _ = zc::reject(zc::META_VA, &r);
                            } else {
                                let input = unsafe {
                                    core::slice::from_raw_parts(
                                        (zc::POOL_VA + r.id() as usize * zc::BUFFER) as *const u8, length)
                                };
                                workers.dispatch_ref(Port::Wan, &r, input, 0);
                            }
                            continue;
                        }
                    }
                    let Ok((length,_verified)) = queue::pop_meta(0x64000000, epoch, &mut frame) else { break };
                    if length == 0 { break; }
                    processed = true;
                    #[cfg(feature = "forward-workers")]
                    if frame[12..14] == [8,0] {
                        workers.dispatch(Port::Wan, &frame[..length], 0,_verified);
                        continue;
                    }
                    let result = stack.receive(Port::Wan, &frame[..length], now, &mut response);
                    echo_trace(2, &frame[..length], echo_outcome(&result, &response), 0);
                    match result {
                        Ok(Some((Port::Lan1, size))) => {
                            stat(19).fetch_add(1, Ordering::Relaxed);
                            send(bridge.destination(&response[..6], links, now), &response[..size], false, None, false);
                        }
                        Ok(Some((Port::Wan, size))) => send_wan(&mut stack, Some(epoch), &response[..size]),
                        _ => {}
                    }
                }
            }
        }
        for (port, base) in BASE.into_iter().enumerate() {
            if PORT_MASK & (1 << port) == 0 { continue; }
            for _ in 0..queue::batch(option_env!("R5S_DATAPATH_BATCH"), 4) {
                #[cfg(all(feature = "zc", feature = "forward-workers"))]
                let from_pool = {
                    let queue = zc::link_queue(zc::LINKS_VA,
                        if port == 0 { zc::Q_LAN1_RX } else { zc::Q_LAN2_RX });
                    match zc::RefQueue::pop(queue) {
                        Ok(Some(r)) if r.flags & zc::FLAG_COPY != 0 => None,
                        Ok(Some(r)) => {
                            processed = true;
                            match lan_ref(port, r, &mut bridge, links, now,
                                                     &mut frame, &mut next_report_ms, &mut workers) {
                            RefOutcome::Handled => continue,
                                RefOutcome::Copied(length) => Some(length),
                            }
                        }
                        _ => break,
                    }
                };
                #[cfg(not(all(feature = "zc", feature = "forward-workers")))]
                let from_pool: Option<usize> = None;
                let (length,_verified) = match from_pool {
                    Some(length) => (length, 0u32),
                    None => match queue::pop_meta(base, 0, &mut frame) {
                        Ok((0,_)) => break,
                        Ok(value) => value,
                        Err(()) => {
                            stat(port * 4 + 3).fetch_add(1, Ordering::Relaxed);
                            break;
                        }
                    },
                };
                processed = true;
                let input = &frame[..length];
                let Some((local, egress)) = bridge.receive(port, input, links, now) else {
                    #[cfg(feature = "shared-wan")]
                    echo_trace(0, input, 4, 1 << port);
                    continue;
                };
                if BRIDGE { send(egress, input, true, None, false); }
                if !local || !IP {
                    #[cfg(feature = "shared-wan")]
                    echo_trace(0, input, 4, 1 << port);
                    continue;
                }
                #[cfg(feature = "lan-b51-udp")]
                if now >= next_report_ms && b51_udp::reply(&mut frame[..length], r5s_router_core::bridge::MAC,
                    r5s_router_core::LAN[0], 0xffffff00, || network_snapshot::read(u32::from(PORT_MASK), true))
                {
                    next_report_ms = now.saturating_add(250);
                    send(1 << port, &frame[..b51_udp::frame_length(&frame)], false, None, true);
                    continue;
                }
                let input = &frame[..length];
                let dhcp = input.len() >= 42 && input[12..14] == [8, 0]
                    && input[23] == 17 && u16at(input, 34) == 68 && u16at(input, 36) == 67;
                if dhcp && !DHCP { continue; }
                if input[12..14] == [8, 6] { stat(14).fetch_add(1, Ordering::Relaxed); }
                if input.len() >= 42 && input[12..14] == [8, 0] && input[23] == 1 && input[34] == 8 {
                    stat(15).fetch_add(1, Ordering::Relaxed);
                }
                if dhcp { stat(16).fetch_add(1, Ordering::Relaxed); }
                #[cfg(feature = "forward-workers")]
                if input[12..14] == [8,0] && !dhcp && input.len() >= 34
                    && r5s_router_core::ipat(input,30) != r5s_router_core::LAN[0]
                {
                    workers.dispatch(Port::Lan1, input, 1 << port,_verified);
                    continue;
                }
                let result = stack.receive(Port::Lan1, input, now, &mut response);
                #[cfg(feature = "shared-wan")]
                echo_trace(0, input, echo_outcome(&result, &response), 1 << port);
                #[cfg(feature = "forward-workers")]
                workers.update(&mut stack, upstream.map(|(epoch, _)| epoch));
                match result {
                    Ok(Some((Port::Lan1, size))) => local_reply(&bridge, links, &response[..size], now),
                    #[cfg(feature = "shared-wan")]
                    Ok(Some((Port::Wan, size))) => {
                        send_wan(&mut stack, upstream.map(|(epoch, _)| epoch), &response[..size]);
                    }
                    _ => {}
                }
            }
        }
        if now >= tick {
            tick = now.saturating_add(10);
            #[cfg(all(feature = "forward-workers", feature = "zc"))]
            if option_env!("R5S_WORKER_LAN_EGRESS")==Some("1") {
                zc::publish_lan_routes(forward_shared::CONTROL + zc::LAN_ROUTE_OFFSET,
                    stack.resolved_lan_routes(&bridge, links, now));
            }
            stat(17).store(stack.local_neighbor_count(now) as u32, Ordering::Relaxed);
            #[cfg(all(feature = "shared-wan", not(feature = "forward-workers")))]
            {
                let (capacity, active, retired) = stack.nat_slot_usage(now);
                stat(50).store((capacity << 20) | (retired << 10) | active, Ordering::Release);
            }
            if DHCP {
                if let Ok(Some((_, size))) = stack.tick_lan(Port::Lan1, now, &mut response) {
                    local_reply(&bridge, links, &response[..size], now);
                }
            }
        }
        #[cfg(feature = "forward-workers")]
        {
            workers.update(&mut stack, upstream.map(|(epoch, _)| epoch));
            workers.notify(now);
        }
        #[cfg(feature = "zc")]
        zc::flush_counts(zc::META_VA);
        if !processed { sel4::r#yield(); }
    }
    #[allow(unreachable_code)] Idle
}

#[cfg(feature = "shared-wan")]
fn wan_lease() -> Option<(u32, Option<r5s_router_core::Wan>)> {
    use r5s_router_core::Wan;
    let app = shared::read::<25>(0x40004000)?;
    let driver = shared::read::<20>(0x40002000)?;
    // A producer can publish while this domain is preempted between reads.
    let now = shared::millis();
    let age = (now as u32).wrapping_sub(app[2]);
    let usable = app[1] == 1 && app[7] == 0 && age <= 100 && app[17] > age
        && app[24] & 8 != 0 && app[5] == driver[11] && app[8] == driver[19]
        && driver[1] == 1 && driver[2] == 2 && driver[3] == 0 && driver[14] == 1
        && (now as u32).wrapping_sub(driver[4]) <= 100;
    let lease = usable.then(|| Wan {
        address: app[10], mask: app[11], gateway: app[12],
        dns: [app[13], app[14], app[15]], dns_count: app[16] as usize,
        expires: now.saturating_add(u64::from(app[17] - age)),
    });
    Some((driver[11], lease))
}

struct Idle;
impl Handler for Idle { type Error = Infallible; }
