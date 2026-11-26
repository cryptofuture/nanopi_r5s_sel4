#![no_std]
#![no_main]
mod forward_shared;
mod queue;
#[cfg(feature = "zc")]
mod zc;
#[cfg(feature = "gro")]
mod tso;
mod shared;
use core::sync::atomic::Ordering::Relaxed;
use forward_shared::{Control, INPUT_WAN, TAG_SHIFT, TTL, base, counter};
use r5s_router_core::{Drop, LAN, Port, nat::Router};
use sel4_microkit::{Channel, ChannelSet, Handler, Infallible, protection_domain};

include!("worker_profile.rs");

struct Worker {
    router: Router,
    control: Control,
    sequence: u32,
    next_residency: u64,
    next_occupancy: u64,
    #[cfg(feature = "zc")]
    next_input: usize,
    #[cfg(feature = "zc")]
    steering_cursor: usize,
    #[cfg(feature = "zc")]
    pending_wan_lanes: u32,
    #[cfg(feature = "zc")]
    wan_batch_route: Option<(u32, [u8; 6])>,
    #[cfg(feature = "zc")]
    tx_flows: [[Option<TxFlow>; zc::DEDICATED_TX_LANES]; 2],
    #[cfg(feature = "zc")]
    lan_paths: [u8; 256],
    #[cfg(feature = "zc")]
    held_lan: [Option<zc::Ref>; 8],
    #[cfg(feature = "zc")]
    diagnostics: [u32;forward_shared::WORKER_DIAG_WORDS],
    #[cfg(feature = "zc")]
    learn: zc::LearnCoalescer,
    #[cfg(feature = "sched-stats")]
    next_scheduler: u64,
}

#[cfg(feature = "zc")]
#[derive(Clone, Copy)]
struct TxFlow {
    addresses: u64,
    transport: u64,
    last: u64,
}

/// Download path decision for one frame: send on the direct worker lane,
/// take the dispatcher fallback (arming a return token when the client is
/// tracked so it can come back in order), or leave the frame on the ordinary
/// result path without a token.
#[cfg(feature = "zc")]
enum LanDecision {
    Direct(usize),
    Ordinary,
    Fallback(u32),
    Wait,
}

impl Worker {
    fn sync(&mut self, now: u64) {
        let Some(sequence) = forward_shared::changed(self.sequence) else { return };
        let Some(mut control) = Control::snapshot() else { return };
        if control.wan_generation != self.control.wan_generation {
            let _ = self.router.configure(None,now);
        }
        if self.router.configure(control.wan,now).is_err() {
            let _ = self.router.configure(None,now);
            control.wan = None;
        }
        if control.revision != self.control.revision {
            for (host, (&before,&after)) in self.control.clients.iter().zip(control.clients.iter()).enumerate() {
                if before != after {
                    self.router.release_client(Port::Lan1,(LAN[0]&0xffffff00)|host as u32,now);
                }
            }
        }
        self.control=control;
        self.sequence=sequence;
    }

    /// Zero-copy path: translate one pool buffer in place and return its
    /// reference to the dispatcher. Every consumed reference is either freed
    /// here (drop) or published back; the payload is never copied.
    #[cfg(feature = "zc")]
    fn record_direction(&mut self, wan: bool, packets: u32, drops: u32) {
        let direction=usize::from(wan);
        self.diagnostics[direction]=self.diagnostics[direction].wrapping_add(packets);
        self.diagnostics[2+direction]=self.diagnostics[2+direction].wrapping_add(drops);
    }

    #[cfg(feature = "zc")]
    fn record_reject(&mut self, error: Drop) {
        let index=match error {
            Drop::Malformed | Drop::Checksum => 4,
            Drop::NoWan | Drop::NeighborBusy => 7,
            _ => 5,
        };
        self.diagnostics[index]=self.diagnostics[index].wrapping_add(1);
    }

    #[cfg(feature = "zc")]
    fn forward_ref(&mut self, r: &mut zc::Ref, now: u64) -> bool {
        if r.epoch != self.control.revision {
            self.sync(now);
        }
        if r.epoch != self.control.revision || self.control.wan.is_none() {
            let index=if r.epoch!=self.control.revision {6} else {7};
            self.diagnostics[index]=self.diagnostics[index].wrapping_add(1);
            let _ = zc::reject(zc::META_VA, r);
            return false;
        }
        if zc::status(zc::META_VA, r.id()) != (zc::CPU, r.generation()) {
            zc::count(zc::META_VA, zc::C_OWNERSHIP_ERRORS);
            return false;
        }
        let length = r.len as usize;
        if !(34..=1514).contains(&length) {
            self.record_reject(Drop::Malformed);
            let _ = zc::reject(zc::META_VA, r);
            return false;
        }
        let buffer = unsafe {
            core::slice::from_raw_parts_mut(
                (zc::POOL_VA + r.id() as usize * zc::BUFFER) as *mut u8,
                zc::BUFFER,
            )
        };
        let wan = r.flags & zc::FLAG_WAN != 0;
        let ingress = if wan { Port::Wan } else { Port::Lan1 };
        let outcome = self.router.forward_with_rx_checksum(ingress, &mut buffer[14..length], now, 1500, r.meta);
        if !self.accept_ref(r, outcome) { return false; }
        if !self.publish_ref(r, now) {
            let _ = zc::reject(zc::META_VA, r);
            return false;
        }
        true
    }

    #[cfg(feature = "zc")]
    fn worker_tx_published(&mut self, refs: usize, published: bool) -> bool {
        if published {
            self.pending_wan_lanes |= zc::wan_lane_bit(refs);
        } else {
            zc::count(zc::META_VA, zc::WORKER_TX_FULL);
        }
        published
    }

    #[cfg(feature = "zc")]
    fn gateway_egress(&self, frame: &[u8], ordinary_outbound: bool) -> bool {
        zc::worker_wan_egress() && ordinary_outbound
            && frame.len() >= 34 && frame[12..15] == [8, 0, 0x45] && matches!(frame[23], 6 | 17)
            && self.control.wan.is_some_and(|wan| wan.next_hop(r5s_router_core::ipat(frame, 30)) == wan.gateway)
    }

    /// Source-learn record for the dispatcher's bridge. Identical learns are
    /// coalesced inside a short window per worker; a failed push is retried
    /// on the next frame rather than cached.
    #[cfg(feature = "zc")]
    fn learn_before_egress(&mut self, lan: u16, frame: &[u8], revision: u32, now: u64) -> bool {
        if lan == 0 { return true; }
        let mac = &frame[6..12];
        if !self.learn.due(mac, lan, revision, now) {
            zc::count(zc::META_VA, zc::C_LEARN_COALESCED);
            return true;
        }
        let submitted = zc::RefQueue::push(forward_shared::result_ref(ID),
            &zc::source_learn(mac, lan, revision)) == Ok(true);
        zc::count(zc::META_VA, if submitted { zc::C_LEARN_SUBMITTED } else { zc::C_LEARN_DROPS });
        if submitted { self.learn.pushed(mac, lan, revision, now); }
        submitted
    }

    /// Write the final L2 header, hardware checksum fields and Ethernet
    /// padding for a WAN-bound payload on the worker core. The caller must
    /// cache-maintain the buffer before applying [`Self::wan_ref_ready`].
    #[cfg(feature = "zc")]
    fn wan_ref_payload(record: &zc::Ref, mac: [u8; 6]) -> Option<(usize, bool)> {
        let frame = unsafe {core::slice::from_raw_parts_mut(
            (zc::POOL_VA + record.id() as usize * zc::BUFFER) as *mut u8, zc::BUFFER)};
        let size = zc::worker_wan_header(frame, mac)?;
        let transport = zc::TX_CHECKSUM_REQUESTED && zc::tx_checksum_offload(zc::META_VA)
            && matches!(
                zc::nat_tx_checksum_kind(&frame[..size], record.flags),
                zc::TxChecksum::Tcp | zc::TxChecksum::Udp
            );
        if transport {
            // Hardware insertion overwrites these fields; zeroing them here
            // lets the GMAC pass the checksum request without writing (and
            // re-cleaning) the prepared payload.
            let offset = if frame[23] == 6 { 50 } else { 40 };
            frame[24..26].fill(0);
            frame[offset..offset + 2].fill(0);
        }
        if size < 60 { frame[size..60].fill(0); }
        Some((size, transport))
    }

    /// Mark a payload-finalised record ready for the GMAC submit path.
    #[cfg(feature = "zc")]
    fn wan_ref_ready(record: &mut zc::Ref, size: usize, transport: bool) {
        record.len = size as u16;
        record.flags |= zc::FLAG_WAN_READY;
        if transport { record.flags |= zc::FLAG_WAN_TRANSPORT_READY; }
    }

    /// Finalise a WAN-bound payload on the worker core: L2 header, hardware
    /// checksum fields and Ethernet padding are written here and the buffer is
    /// cache-maintained (write-back policy), so the GMAC submits without any
    /// per-packet payload work. On maintenance failure the record is left
    /// unprepared and the GMAC keeps its per-packet fallback path.
    #[cfg(feature = "zc")]
    fn prepare_wan_ref(record: &mut zc::Ref, mac: [u8; 6]) {
        let Some((size, transport)) = Self::wan_ref_payload(record, mac) else { return; };
        if zc::maintain_payload(record, zc::PayloadCache::BeforeTx(size.max(60))).is_err() {
            return;
        }
        Self::wan_ref_ready(record, size, transport);
    }

    /// Finalise a WAN-bound burst with one cache clean covering every buffer
    /// in it. Records whose header write fails stay unprepared; if the span
    /// clean fails, each record falls back to its own maintenance call and is
    /// prepared or left unprepared independently.
    #[cfg(feature = "zc")]
    fn prepare_wan_ref_burst(records: &mut [zc::Ref], mac: [u8; 6]) {
        let mut sizes = [0usize; tso::TSO_MAX_FRAGS];
        let mut transports = [false; tso::TSO_MAX_FRAGS];
        let mut usable = [false; tso::TSO_MAX_FRAGS];
        for (index, record) in records.iter().enumerate() {
            if index == sizes.len() { break; }
            if let Some((size, transport)) = Self::wan_ref_payload(record, mac) {
                sizes[index] = size;
                transports[index] = transport;
                usable[index] = true;
            }
        }
        let spanned = zc::maintain_payload_tx_span(records).is_ok();
        for (index, record) in records.iter_mut().enumerate() {
            if index == sizes.len() || !usable[index] { continue; }
            let size = sizes[index];
            let cleaned = if spanned { Ok(()) }
                else { zc::maintain_payload(record, zc::PayloadCache::BeforeTx(size.max(60))) };
            if cleaned.is_ok() {
                Self::wan_ref_ready(record, size, transports[index]);
            }
        }
    }

    /// The b51 driver's compile-time TX checksum-insertion state for a LAN
    /// port (the same gate the driver applies at submit time).
    #[cfg(feature = "zc")]
    fn lan_tx_checksum(port: usize) -> bool {
        zc::TX_CHECKSUM_REQUESTED
            && if port == 0 {
                option_env!("R5S_LAN1_TX_IPV4_CHECKSUM") == Some("1")
            } else {
                option_env!("R5S_LAN2_TX_IPV4_CHECKSUM") == Some("1")
            }
    }

    /// Finalise a LAN-bound payload on the worker core: checksum seed fields
    /// (when the driver will insert checksums) and Ethernet padding are
    /// written here and the buffer is cache-maintained, so the b51 submits
    /// without per-packet payload work. On maintenance failure the record is
    /// left unprepared and the driver keeps its per-packet fallback path.
    /// The L2 header must already be written (lan_egress / egress loop).
    #[cfg(feature = "zc")]
    fn prepare_lan_ref(record: &mut zc::Ref, port: usize) {
        let length = usize::from(record.len);
        let frame = unsafe {core::slice::from_raw_parts_mut(
            (zc::POOL_VA + record.id() as usize * zc::BUFFER) as *mut u8, zc::BUFFER)};
        let transport = Self::lan_tx_checksum(port)
            && !matches!(
                zc::prepare_nat_tx_checksum(&mut frame[..length], record.flags, true),
                zc::TxChecksum::None
            );
        if length < 60 { frame[length..60].fill(0); }
        if zc::maintain_payload(record, zc::PayloadCache::BeforeTx(length.max(60))).is_err() {
            return;
        }
        record.flags |= zc::FLAG_WAN_READY;
        if transport { record.flags |= zc::FLAG_WAN_TRANSPORT_READY; }
    }

    #[cfg(feature = "zc")]
    fn ref_uses_gateway(&self, r: &zc::Ref) -> bool {
        if !zc::worker_wan_egress() { return false; }
        let frame = unsafe { core::slice::from_raw_parts(
            (zc::POOL_VA + r.id() as usize * zc::BUFFER) as *const u8, usize::from(r.len)) };
        self.gateway_egress(frame, r.flags & (zc::FLAG_WAN | zc::FLAG_TTL) == 0)
    }

    #[cfg(feature = "zc")]
    fn tx_lane_drained(inbound: bool, lane: usize) -> bool {
        if inbound {
            (0..2).all(|port| zc::worker_lan_tx_drained(zc::worker_lan_refs(port, lane)))
        } else {
            zc::worker_wan_tx_drained(zc::worker_wan_refs(lane))
        }
    }

    #[cfg(feature = "zc")]
    fn tx_lane(&mut self, frame: &[u8], inbound: bool, now: u64) -> usize {
        let shared = zc::tx_lane(ID, zc::DEDICATED_TX_LANES);
        let Some(tuple) = frame.get(26..38) else { return shared; };
        let addresses = u64::from_be_bytes(tuple[..8].try_into().unwrap());
        let transport = (u64::from(frame[23]) << 32)
            | u64::from(u32::from_be_bytes(tuple[8..].try_into().unwrap()));
        let flows = &mut self.tx_flows[usize::from(inbound)];
        if let Some((lane, flow)) = flows.iter_mut().enumerate().find_map(|(lane, slot)| {
            slot.as_mut().filter(|flow| flow.addresses == addresses && flow.transport == transport)
                .map(|flow| (lane, flow))
        }) {
            flow.last = now;
            return zc::tx_lane(ID, lane);
        }
        if !Self::tx_lane_drained(inbound, shared) { return shared; }
        for (index, slot) in flows.iter_mut().enumerate() {
            let lane = zc::tx_lane(ID, index);
            if slot.is_none_or(|flow| now.saturating_sub(flow.last) >= 1000)
                && Self::tx_lane_drained(inbound, lane)
            {
                *slot = Some(TxFlow {addresses, transport, last: now});
                return lane;
            }
        }
        shared
    }

    #[cfg(feature = "zc")]
    fn lan_egress(&mut self, frame: &mut [u8], inbound: bool, now: u64) -> Result<LanDecision, ()> {
        if option_env!("R5S_WORKER_LAN_EGRESS") != Some("1") || !inbound
            || frame.len() < 34 || frame[12..15] != [8, 0, 0x45] || !matches!(frame[23], 6 | 17)
        {
            return Ok(LanDecision::Ordinary);
        }
        let ip = r5s_router_core::ipat(frame, 30);
        if ip & 0xffffff00 != LAN[0] & 0xffffff00 { return Ok(LanDecision::Ordinary); }
        if self.held_lan[0].is_some() && self.lan_held(ip) {
            return Ok(LanDecision::Wait);
        }
        let client = (ip & 255) as u32;
        let path = &mut self.lan_paths[client as usize];
        let route = zc::lan_route(forward_shared::CONTROL + zc::LAN_ROUTE_OFFSET, ip, now)?
            .filter(|(_, port, _)| !zc::worker_lan_tx_closed(zc::worker_lan_refs(*port, zc::tx_lane(ID,0))));
        let settled = zc::lan_token_settled(zc::META_VA, ID as u32, client);
        if *path == 1 && route.is_some() && !settled {
            return Ok(LanDecision::Wait);
        }
        let port = match zc::select_lan_tx_path(path, route.map(|(_, port, _)| port), settled,
            |_| settled) {
            Ok(port) => port,
            Err(()) => return Ok(LanDecision::Wait),
        };
        if let Some(port) = port {
            if let Some((mac, _, _)) = route.filter(|(_, target, _)| *target == port) {
                frame[..6].copy_from_slice(&mac);
                frame[6..12].copy_from_slice(&r5s_router_core::bridge::MAC);
            }
            return Ok(LanDecision::Direct(port));
        }
        Ok(LanDecision::Fallback(zc::arm_lan_token(zc::META_VA, ID as u32, client)))
    }

    #[cfg(feature = "zc")]
    fn lan_held(&self, ip: u32) -> bool {
        self.held_lan.iter().flatten().any(|r| {
            let frame = unsafe {core::slice::from_raw_parts(
                (zc::POOL_VA + r.id() as usize * zc::BUFFER) as *const u8,usize::from(r.len))};
            r5s_router_core::ipat(frame,30)==ip
        })
    }

    #[cfg(feature = "zc")]
    fn hold_lan(&mut self, r: zc::Ref) -> bool {
        let Some(slot) = self.held_lan.iter_mut().find(|slot| slot.is_none()) else {
            self.diagnostics[8]=self.diagnostics[8].wrapping_add(1);
            return false;
        };
        *slot = Some(r);
        counter(ID,forward_shared::LAN_RETRY).store(1,core::sync::atomic::Ordering::Release);
        true
    }

    #[cfg(feature = "zc")]
    fn hold_lan_copy(&mut self, frame: &[u8], epoch: u32) -> bool {
        let Some(mut r) = zc::alloc(zc::META_VA,zc::PARTITION_WAN) else {
            self.diagnostics[9]=self.diagnostics[9].wrapping_add(1);
            return false;
        };
        unsafe { core::ptr::copy_nonoverlapping(frame.as_ptr(),
            (zc::POOL_VA + r.id() as usize * zc::BUFFER) as *mut u8,frame.len()); }
        r.len = frame.len() as u16;
        r.flags = zc::FLAG_WAN | zc::FLAG_NAT_VALIDATED;
        r.epoch = epoch;
        if zc::publish(zc::META_VA,&r).is_err() { return false; }
        if self.hold_lan(r) { return true; }
        let _ = zc::reject(zc::META_VA,&r);
        false
    }

    #[cfg(feature = "zc")]
    fn retry_lan(&mut self, now: u64) -> u32 {
        let mut dropped = 0;
        let held = core::mem::take(&mut self.held_lan);
        for r in held.into_iter().flatten() {
            if r.epoch != self.control.revision || self.control.wan.is_none() {
                let index=if r.epoch!=self.control.revision {6} else {7};
                self.diagnostics[index]=self.diagnostics[index].wrapping_add(1);
                let _ = zc::reject(zc::META_VA,&r);
                dropped += 1;
            } else if !self.publish_ref(&r,now) {
                let _ = zc::reject(zc::META_VA,&r);
                dropped += 1;
            }
        }
        counter(ID,forward_shared::LAN_RETRY)
            .store(u32::from(self.held_lan[0].is_some()),core::sync::atomic::Ordering::Release);
        self.record_direction(true,0,dropped);
        dropped
    }

    #[cfg(feature = "zc")]
    fn publish_ref(&mut self, r: &zc::Ref, now: u64) -> bool {
        if option_env!("R5S_WORKER_LAN_EGRESS") == Some("1") && r.flags & zc::FLAG_WAN != 0 {
            let frame = unsafe { core::slice::from_raw_parts_mut(
                (zc::POOL_VA + r.id() as usize * zc::BUFFER) as *mut u8, usize::from(r.len)) };
            match self.lan_egress(frame, true, now) {
                Ok(LanDecision::Direct(port)) => {
                    let refs = zc::worker_lan_refs(port, self.tx_lane(frame, true, now));
                    let stamp = zc::arm_lan_token(zc::META_VA, ID as u32, r5s_router_core::ipat(frame,30) & 255);
                    let mut record = zc::Ref { flags: r.flags | zc::FLAG_LAN_TOKEN, meta: stamp << 16, ..*r };
                    if option_env!("R5S_WORKER_LAN_PREP") == Some("1") {
                        Self::prepare_lan_ref(&mut record, port);
                    }
                    if zc::publish_worker_lan_tx(refs, || zc::RefQueue::push(refs, &record) == Ok(true)) {
                        return true;
                    }
                    zc::retire_lan_token(zc::META_VA, stamp);
                    return self.hold_lan(*r);
                }
                Ok(LanDecision::Wait) | Err(()) => return self.hold_lan(*r),
                Ok(LanDecision::Ordinary) => {}
                Ok(LanDecision::Fallback(token)) => {
                    let mut request = *r;
                    request.flags |= zc::FLAG_LAN_TOKEN;
                    request.meta = token;
                    let queued = zc::RefQueue::push(forward_shared::result_ref(ID), &request) == Ok(true);
                    if !queued {
                        let _ = zc::retire_lan_token(zc::META_VA, token);
                        return self.hold_lan(*r);
                    }
                    return true;
                }
            }
        }
        if self.ref_uses_gateway(r) {
            let frame = unsafe { core::slice::from_raw_parts(
                (zc::POOL_VA + r.id() as usize * zc::BUFFER) as *const u8, usize::from(r.len)) };
            let lan = (r.flags & zc::LAN_INGRESS_MASK) >> zc::LAN_INGRESS_SHIFT;
            if !self.learn_before_egress(lan, frame, r.epoch, now) { return false; }
            let refs = zc::worker_wan_refs(self.tx_lane(frame, false, now));
            let mut record = zc::Ref { meta: now as u32, ..*r };
            if let Some(mac) = self.wan_batch_route.filter(|(epoch,_)| *epoch == r.epoch).map(|(_,mac)| mac) {
                Self::prepare_wan_ref(&mut record, mac);
            }
            return self.worker_tx_published(refs, zc::RefQueue::push(refs, &record) == Ok(true));
        }
        zc::RefQueue::push(forward_shared::result_ref(ID), r) == Ok(true)
    }

    #[cfg(feature = "zc")]
    fn accept_ref(&mut self, r: &mut zc::Ref, outcome: Result<Port, Drop>) -> bool {
        match outcome {
            Ok(_) => {
                if zc::TX_CHECKSUM_REQUESTED { r.flags |= zc::FLAG_NAT_VALIDATED; }
            }
            Err(Drop::Ttl) if r.flags & zc::FLAG_WAN == 0 => r.flags |= zc::FLAG_TTL,
            Err(error) => {
                self.record_reject(error);
                let _ = zc::reject(zc::META_VA, r);
                return false;
            }
        }
        true
    }

    #[cfg(feature = "gro")]
    fn frame(r: zc::Ref) -> &'static [u8] {
        unsafe { core::slice::from_raw_parts(
            (zc::POOL_VA + r.id() as usize * zc::BUFFER) as *const u8, usize::from(r.len)) }
    }

    #[cfg(feature = "gro")]
    fn take_burst(&self, queue: usize, limit: u32) -> Option<([zc::Ref; tso::TSO_MAX_FRAGS], usize)> {
        if limit < 2 { return None; }
        let first = zc::RefQueue::peek(queue).ok()??;
        let valid = |r: zc::Ref| r.flags & zc::FLAG_COPY == 0
            && (54..=1514).contains(&r.len)
            && zc::status(zc::META_VA, r.id()) == (zc::CPU, r.generation());
        if !valid(first) || first.epoch != self.control.revision || self.control.wan.is_none() {
            return None;
        }
        let marked=first.flags & zc::FLAG_GRO_MASK==zc::FLAG_GRO_FIRST;
        if !marked && !tso::gro_starts(Self::frame(first)) { return None; }
        let mut burst = [first; tso::TSO_MAX_FRAGS];
        let max = if zc::LARGE_GRO { tso::TSO_MAX_FRAGS as u32 } else { 2 };
        let mut count = 1;
        for offset in 1..limit.min(max) {
            let Some(next) = zc::RefQueue::peek_at(queue,offset).ok().flatten() else { break };
            if next.flags & !zc::FLAG_GRO_MASK != first.flags & !zc::FLAG_GRO_MASK
                || next.epoch != first.epoch || !valid(next)
                || burst[..count].iter().any(|r| r.id() == next.id())
            {
                break;
            }
            let compatible=if marked {offset==1 && next.flags & zc::FLAG_GRO_MASK==zc::FLAG_GRO_LAST}
                else {next.flags & zc::FLAG_GRO_MASK==0
                    && tso::gro_follow(Self::frame(burst[count-1]),Self::frame(next)).is_some()};
            if !compatible {break;}
            burst[count] = next;
            count += 1;
            if marked || Self::frame(next)[47] == 0x18 { break; }
        }
        if count < 2 { return None; }
        for r in &mut burst[..count] {
            let _=zc::RefQueue::pop(queue);
            r.flags &= !zc::FLAG_GRO_MASK;
        }
        Some((burst,count))
    }

    #[cfg(feature = "gro")]
    fn pack_direct_tso(records: &mut [zc::Ref]) {
        let mss = records[0].len - 54;
        let first_tcp_flags = (zc::POOL_VA + records[0].id() as usize * zc::BUFFER + 47) as *mut u8;
        let final_flags = Self::frame(*records.last().unwrap())[47];
        unsafe { first_tcp_flags.write(final_flags); }
        records[0].flags = tso::FLAG_TSO_FIRST | tso::FLAG_TSO_GRO;
        records[0].meta = tso::pack_first_meta(mss, false, 20);
        let count=records.len();
        for (i,record) in records.iter_mut().enumerate().skip(1) {
            let frame = unsafe {core::slice::from_raw_parts_mut(
                (zc::POOL_VA + record.id() as usize * zc::BUFFER) as *mut u8,usize::from(record.len))};
            frame.copy_within(54..,0);
            record.len = mss;
            record.flags = if i+1==count {tso::FLAG_TSO_LAST} else {tso::FLAG_TSO_CONT};
        }
    }

    #[cfg(feature = "gro")]
    fn publish_burst(&mut self, refs: &mut [zc::Ref], ingress: Port, now: u64) -> bool {
        let first = refs[0];
        if ingress != Port::Wan && self.ref_uses_gateway(&first) {
            let lan = (first.flags & zc::LAN_INGRESS_MASK) >> zc::LAN_INGRESS_SHIFT;
            if !self.learn_before_egress(lan, Self::frame(first), first.epoch, now) { return false; }
            let lane = self.tx_lane(Self::frame(first), false, now);
            let queue = zc::worker_wan_refs(lane);
            let mut records = [first; tso::TSO_MAX_FRAGS];
            for (record, r) in records.iter_mut().zip(refs.iter()) {
                *record = zc::Ref { meta: now as u32, ..*r };
            }
            if lane % zc::TX_FLOW_LANES != zc::DEDICATED_TX_LANES
                && zc::LARGE_GRO && zc::wan_tso_available() && option_env!("R5S_WAN_TSO") == Some("1")
                && option_env!("R5S_GRO_TSO") == Some("1")
                && zc::RefQueue::depth(queue).is_ok_and(|depth| depth + refs.len() as u32 <= zc::RQ_COUNT)
            {
                Self::pack_direct_tso(&mut records[..refs.len()]);
                let queued = zc::RefQueue::push_batch(queue,&records[..refs.len()]) == Ok(true);
                self.worker_tx_published(queue,queued);
                if queued {
                    zc::count(zc::META_VA,zc::C_GRO_GROUPS);
                    zc::counter(zc::META_VA,zc::C_GRO_SEGMENTS).fetch_add(refs.len() as u32,Relaxed);
                } else {
                    for r in refs.iter() { let _ = zc::reject(zc::META_VA,r); }
                    zc::count(zc::META_VA,zc::C_GRO_FALLBACK);
                }
                return true;
            }
            if let Some(mac) = self.wan_batch_route.filter(|(epoch,_)| *epoch == first.epoch).map(|(_,mac)| mac) {
                Self::prepare_wan_ref_burst(&mut records[..refs.len()], mac);
            }
            return self.worker_tx_published(queue,
                zc::RefQueue::push_batch(queue, &records[..refs.len()]) == Ok(true));
        }
        if ingress == Port::Wan && option_env!("R5S_WORKER_LAN_EGRESS") == Some("1") {
            let ip = r5s_router_core::ipat(Self::frame(first),30);
            let Ok(Some((mac,port,_))) = zc::lan_route(forward_shared::CONTROL + zc::LAN_ROUTE_OFFSET,ip,now)
                else { return false };
            let lane = self.tx_lane(Self::frame(first), true, now);
            let queue = zc::worker_lan_refs(port, lane);
            if self.lan_paths[(ip & 255) as usize] != port as u8 + 2
                || (self.held_lan[0].is_some() && self.lan_held(ip)) { return false; }
            for r in refs.iter() {
                let eth = unsafe {core::slice::from_raw_parts_mut(
                    (zc::POOL_VA + r.id() as usize * zc::BUFFER) as *mut u8,14)};
                eth[..6].copy_from_slice(&mac);
                eth[6..12].copy_from_slice(&r5s_router_core::bridge::MAC);
            }
            let tso = lane % zc::TX_FLOW_LANES != zc::DEDICATED_TX_LANES
                && zc::LARGE_GRO && option_env!("R5S_GRO_TSO") == Some("1")
                && (if port==0 {option_env!("R5S_LAN1_TSO")} else {option_env!("R5S_LAN2_TSO")}) == Some("1")
                && zc::RefQueue::depth(queue).is_ok_and(|depth| depth+refs.len() as u32<=zc::RQ_COUNT);
            let mut packed=false;
            let published=zc::publish_worker_lan_tx_many(queue,refs.len() as u32,|| {
                if !zc::RefQueue::depth(queue).is_ok_and(|depth| depth + refs.len() as u32 <= zc::RQ_COUNT) {
                    return false;
                }
                if tso {
                    Self::pack_direct_tso(refs);
                    packed=true;
                }
                let mut records = [first; tso::TSO_MAX_FRAGS];
                records[..refs.len()].copy_from_slice(refs);
                let count = if packed { 1 } else { refs.len() };
                for r in &mut records[..count] {
                    let stamp = zc::arm_lan_token(zc::META_VA, ID as u32, ip & 255);
                    r.flags |= zc::FLAG_LAN_TOKEN;
                    r.meta = (r.meta & 0xffff) | stamp << 16;
                    if !packed && option_env!("R5S_WORKER_LAN_PREP") == Some("1") {
                        Self::prepare_lan_ref(r, port);
                    }
                }
                let queued = zc::RefQueue::push_batch(queue,&records[..refs.len()])==Ok(true);
                if !queued {
                    for r in &records[..count] { zc::retire_lan_token(zc::META_VA, zc::direct_lan_token(r)); }
                }
                queued
            });
            if packed {
                if published {
                    zc::count(zc::META_VA,zc::C_GRO_GROUPS);
                    zc::counter(zc::META_VA,zc::C_GRO_SEGMENTS).fetch_add(refs.len() as u32,Relaxed);
                } else {
                    for r in refs.iter() {let _=zc::reject(zc::META_VA,r);}
                }
                return true;
            }
            return published;
        }
        zc::RefQueue::push_batch(forward_shared::result_ref(ID),refs) == Ok(true)
    }

    #[cfg(feature = "gro")]
    fn forward_burst(&mut self, refs: &mut [zc::Ref], now: u64) -> u32 {
        let ingress = if refs[0].flags & zc::FLAG_WAN != 0 { Port::Wan } else { Port::Lan1 };
        let mut accepted = [false; tso::TSO_MAX_FRAGS];
        let mut results = [Err(Drop::Malformed); tso::TSO_MAX_FRAGS];
        let mut next = 0;
        self.router.forward_burst(ingress,refs.iter().map(|r| {
            let ip = unsafe { core::slice::from_raw_parts_mut(
                (zc::POOL_VA + r.id() as usize * zc::BUFFER + 14) as *mut u8,usize::from(r.len)-14) };
            (ip,r.meta)
        }),now,1500,|result| { results[next]=result; next+=1; });
        for (index,r) in refs.iter_mut().enumerate() {
            accepted[index] = self.accept_ref(r,results[index]);
        }
        if accepted[..refs.len()].iter().all(|accepted| *accepted)
            && refs.iter().all(|r| r.flags & zc::FLAG_TTL == 0)
            && self.publish_burst(refs,ingress,now)
        {
            return 0;
        }
        let mut drops = 0;
        for (r, accepted) in refs.iter().zip(accepted) {
            if !accepted {
                drops += 1;
            } else if !self.publish_ref(r, now) {
                let _ = zc::reject(zc::META_VA, r);
                drops += 1;
            }
        }
        drops
    }
    fn drain(&mut self, now:u64, input:usize, #[cfg(feature = "zc")] refs:usize, limit:u32) -> (u32,u32) {
        let mut received = 0;
        let mut drops = 0;
        let mut remaining = limit;
        while remaining != 0 {
            #[cfg(feature = "gro")]
            if let Some((mut burst,count)) = self.take_burst(refs,remaining) {
                received += count as u32;
                remaining -= count as u32;
                let wan=burst[0].flags & zc::FLAG_WAN!=0;
                let rejected=self.forward_burst(&mut burst[..count],now);
                self.record_direction(wan,count as u32,rejected);
                drops+=rejected;
                continue;
            }
            remaining -= 1;
            #[cfg(feature = "zc")]
            {
                let Ok(Some(mut r)) = zc::RefQueue::pop(refs) else { break };
                if r.flags & zc::FLAG_COPY == 0 {
                    r.flags &= !zc::FLAG_GRO_MASK;
                    received += 1;
                    let rejected=u32::from(!self.forward_ref(&mut r,now));
                    self.record_direction(r.flags & zc::FLAG_WAN!=0,1,rejected);
                    drops+=rejected;
                    continue;
                }
            }
            #[cfg(feature = "zc")]
            let mut cop_wan=false;
            let result=unsafe {queue::consume_in_place_meta(input, |frame,tag,verified| {
                let wan=tag & INPUT_WAN != 0;
                #[cfg(feature = "zc")]
                {cop_wan=wan;}
                if tag >> TAG_SHIFT != self.control.revision { self.sync(now); }
                if tag >> TAG_SHIFT != self.control.revision || self.control.wan.is_none() {
                    #[cfg(feature = "zc")]
                    {
                        let index=if tag>>TAG_SHIFT!=self.control.revision {6} else {7};
                        self.diagnostics[index]=self.diagnostics[index].wrapping_add(1);
                    }
                    return false;
                }
                let ingress=if wan {Port::Wan}else{Port::Lan1};
                let tag=match self.router.forward_with_rx_checksum(ingress,&mut frame[14..],now,1500,verified) {
                    Ok(_) => tag,
                    Err(Drop::Ttl) if !wan => tag | TTL,
                    Err(error) => {
                        #[cfg(feature = "zc")]
                        self.record_reject(error);
                        let _=error;
                        return false;
                    }
                };
                #[cfg(feature = "zc")]
                if self.gateway_egress(frame, tag & (INPUT_WAN | TTL) == 0) {
                    let lan = ((tag & forward_shared::LAN_TAG_MASK) >> forward_shared::LAN_TAG_SHIFT) as u16;
                    let revision = tag >> TAG_SHIFT;
                    let lane = self.tx_lane(frame, false, now);
                    let refs = zc::worker_wan_refs(lane);
                    return self.learn_before_egress(lan, frame, revision, now)
                        && self.worker_tx_published(refs,
                            zc::RefQueue::push_copy_at(refs, revision, now as u32,
                                || queue::push(zc::worker_wan_frames(lane), revision, frame)) == Ok(true));
                }
                #[cfg(feature = "zc")]
                let token = match self.lan_egress(frame, wan, now) {
                    Ok(LanDecision::Direct(port)) => {
                        let revision = tag >> TAG_SHIFT;
                        let lane = self.tx_lane(frame, true, now);
                        let refs = zc::worker_lan_refs(port, lane);
                        let stamp = zc::arm_lan_token(zc::META_VA, ID as u32, r5s_router_core::ipat(frame,30) & 255);
                        if zc::publish_worker_lan_tx(refs, || zc::RefQueue::push_copy_token_at(refs, revision, stamp,
                            || queue::push(zc::worker_lan_frames(port, lane), revision, frame)) == Ok(true)) {
                            return true;
                        }
                        zc::retire_lan_token(zc::META_VA, stamp);
                        return self.hold_lan_copy(frame,revision);
                    }
                    Ok(LanDecision::Wait) | Err(()) => return self.hold_lan_copy(frame,tag >> TAG_SHIFT),
                    Ok(LanDecision::Fallback(token)) => Some(token),
                    Ok(LanDecision::Ordinary) => None,
                };
                let copy = || queue::push(base(ID)+queue::DIRECTION,tag,frame);
                #[cfg(feature = "zc")]
                let result = match token {
                    Some(token) => {
                        let queued = zc::RefQueue::push_copy_token(forward_shared::result_ref(ID), token, copy);
                        if queued != Ok(true) { let _ = zc::retire_lan_token(zc::META_VA, token); }
                        queued
                    }
                    None => zc::RefQueue::push_copy(forward_shared::result_ref(ID), copy),
                };
                #[cfg(not(feature = "zc"))]
                let result = copy();
                #[cfg(feature = "zc")]
                if result != Ok(true) && token.is_some() {
                    return self.hold_lan_copy(frame,tag >> TAG_SHIFT);
                }
                result == Ok(true)
            })};
            match result {
                Ok(None) => break,
                Ok(Some(queued)) => {
                    received+=1;
                    drops+=u32::from(!queued);
                    #[cfg(feature = "zc")]
                    self.record_direction(cop_wan,1,u32::from(!queued));
                }
                Err(()) => {drops+=1; break;}
            }
        }
        (received,drops)
    }

}

#[protection_domain]
fn init() -> impl Handler {
    #[cfg(feature = "sched-stats")]
    let next_scheduler = if sel4::benchmark_reset_log().is_ok() {
        shared::millis().saturating_add(1000)
    } else {u64::MAX};
    Worker { router: Router::sharded(shared::millis() as u32 ^ ID as u32, ID as u8),
        control: Control::new(), sequence:0, next_residency:0, next_occupancy:0,
        #[cfg(feature = "zc")]
        lan_paths:[0;256],
        #[cfg(feature = "zc")]
        held_lan:[None;8],
        #[cfg(feature = "zc")]
        diagnostics:[0;forward_shared::WORKER_DIAG_WORDS],
        #[cfg(feature = "zc")]
        next_input:0,
        #[cfg(feature = "zc")]
        pending_wan_lanes: 0,
        #[cfg(feature = "zc")]
        wan_batch_route: None,
        #[cfg(feature = "zc")]
        steering_cursor: ID * zc::STEERING_SLOTS / 4,
        #[cfg(feature = "zc")]
        tx_flows: [[None; zc::DEDICATED_TX_LANES]; 2],
        #[cfg(feature = "zc")]
        learn: zc::LearnCoalescer::new(),
        #[cfg(feature = "sched-stats")]
        next_scheduler }
}

impl Handler for Worker {
    type Error = Infallible;
    fn notified(&mut self, _: ChannelSet) -> Result<(), Infallible> {
        let input = base(ID);
        let now = shared::millis();
        self.sync(now);
        #[cfg(feature = "zc")]
        if zc::worker_wan_egress() {
            let revision = self.control.revision;
            self.wan_batch_route = zc::wan_route(zc::WORKER_WAN_ROUTE, revision, now)
                .map(|mac| (revision, mac));
        }
        #[cfg(feature = "zc")]
        let (received, mut drops) = {
            let routes = [
                Some((input, forward_shared::request_ref(ID))),
                zc::wan_fast_path().then(|| (zc::fast_wan_frames(ID), zc::fast_wan_requests(ID))),
                zc::lan_fast_path().then(|| (zc::fast_lan_frames(0, ID), zc::fast_lan_requests(0, ID))),
                zc::lan_fast_path().then(|| (zc::fast_lan_frames(1, ID), zc::fast_lan_requests(1, ID))),
            ];
            let mut received = 0;
            let mut drops = self.retry_lan(now);
            let mut pending = [0; 4];
            for (index, route) in routes.iter().enumerate() {
                if let Some((_, refs)) = route {
                    zc::RefQueue::rearm_wake(*refs);
                    pending[index] = zc::RefQueue::depth(*refs).unwrap_or(0);
                }
            }
            // Adaptive per-round quantum (the service_tx idiom): a single
            // deep route drains quickly while several runnable routes share
            // the pass fairly.
            let quantum = if pending.iter().filter(|&&depth| depth != 0).count() > 1 { 8 } else { 32 };
            for _ in 0..zc::RQ_COUNT.div_ceil(quantum) {
                let mut progress = false;
                for step in 0..routes.len() {
                    let index = (self.next_input + step) % routes.len();
                    let limit = pending[index].min(quantum);
                    if limit == 0 { continue; }
                    let Some((frames, refs)) = routes[index] else { continue };
                    let (count, dropped) = self.drain(now, frames, refs, limit);
                    // A short drain empties the route: zero the snapshot so
                    // this pass does not re-poll it (a producer push during
                    // the drain re-arms the wake and the next pass picks it up).
                    pending[index] = if count < limit { 0 } else { pending[index] - count };
                    received += count;
                    drops += dropped;
                    progress = true;
                }
                if !progress { break; }
            }
            self.next_input = (self.next_input + 1) % routes.len();
            (received, drops)
        };
        #[cfg(not(feature = "zc"))]
        let (received, drops) = self.drain(now, input, queue::COUNT);
        #[cfg(feature = "zc")]
        if option_env!("R5S_WORKER_LAN_EGRESS") == Some("1") {
            for (port, lane) in (0..2).flat_map(|port|
                (0..zc::TX_FLOW_LANES).map(move |lane| (port, ID * zc::TX_FLOW_LANES + lane))) {
                let refs = zc::worker_lan_refs(port, lane);
                if !zc::worker_lan_consumer_stopped(refs) {continue;}
                for _ in 0..zc::RQ_COUNT {
                    let Ok(Some(r)) = zc::RefQueue::pop(refs) else {break};
                    if r.flags & zc::FLAG_COPY != 0 {
                        let _ = unsafe {queue::consume_in_place(zc::worker_lan_frames(port, lane), |_, _| ())};
                    } else {
                        let _ = zc::reject(zc::META_VA, &r);
                    }
                    zc::retire_worker_lan_tx(refs);
                    zc::retire_lan_token(zc::META_VA, zc::direct_lan_token(&r));
                    drops += 1;
                }
            }
        }
        #[cfg(feature = "zc")]
        if zc::activate_wan_lanes(core::mem::take(&mut self.pending_wan_lanes))
            && zc::WORKER_TX_EVENT_DRIVEN
        {
            zc::count(zc::META_VA, zc::WORKER_TX_WAKEUPS);
            sel4_microkit::Channel::new(1).notify();
        }
        if received > 0 {
            counter(ID,0).fetch_add(received,Relaxed);
            counter(ID,3).fetch_add(1,Relaxed);
        }
        counter(ID,1).fetch_add(drops,Relaxed);
        #[cfg(feature = "zc")]
        if received!=0 || drops!=0 {
            for (i,value) in self.diagnostics.iter().enumerate() {
                counter(ID,forward_shared::WORKER_DIAG+i).store(*value,Relaxed);
            }
        }
        self.router.maintain(now);
        if now >= self.next_residency {
            self.next_residency=now.saturating_add(1000);
            use core::sync::atomic::Ordering::{AcqRel,Release};
            let address=forward_shared::residency(ID);
            let word=|i| forward_shared::word(address,i);
            word(0).fetch_add(1,AcqRel);
            word(1).store(queue::consumed(input),Relaxed);
            #[cfg(feature = "zc")]
            word(2).store(zc::RefQueue::consumed(forward_shared::request_ref(ID)),Relaxed);
            for (i,bits) in self.router.active_endpoints(now).into_iter().enumerate() {
                word(3+i).store(bits,Relaxed);
            }
            word(0).fetch_add(1,Release);
        }
        if now >= self.next_occupancy {
            self.next_occupancy=now.saturating_add(10);
            let (_,active,retired) = self.router.slot_usage(now);
            counter(ID,2).store(active|(retired<<10),Relaxed);
            #[cfg(feature = "zc")]
            if zc::lan_fast_path() {
                zc::publish_steering_load(ID, active + retired);
                zc::reap_transport_pins(ID, &mut self.steering_cursor, || {
                    [forward_shared::request_ref(ID), zc::fast_lan_requests(0, ID), zc::fast_lan_requests(1, ID)]
                        .into_iter().any(|refs| zc::RefQueue::depth(refs) != Ok(0))
                    || (zc::worker_wan_egress() && (0..zc::TX_FLOW_LANES).any(|lane|
                        !zc::worker_wan_tx_complete(zc::worker_wan_refs(zc::tx_lane(ID, lane)))))
                    || (option_env!("R5S_WORKER_LAN_EGRESS") == Some("1") && (0..zc::TX_FLOW_LANES)
                        .any(|lane| !Self::tx_lane_drained(true, zc::tx_lane(ID, lane))))
                    || zc::RefQueue::depth(forward_shared::result_ref(ID)) != Ok(0)
                    || self.held_lan[0].is_some()
                }, |key| self.router.retains_endpoint((key >> 24) as u32, (key >> 8) as u16, key as u8, now));
            }
        }
        #[cfg(feature = "sched-stats")]
        if now >= self.next_scheduler {
            let tcb = sel4::cap::Tcb::from_bits(6);
            sel4::benchmark_finalize_log();
            sel4::benchmark_get_thread_utilisation(tcb);
            let values = sel4::with_ipc_buffer(|buffer| {
                let words = buffer.msg_regs();
                [words[9].saturating_sub(words[4]), words[4], words[13]]
            });
            for (i, value) in values.into_iter().enumerate() {
                counter(ID,4+i*2).store(value as u32,Relaxed);
                counter(ID,5+i*2).store((value >> 32) as u32,Relaxed);
            }
            sel4::benchmark_reset_thread_utilisation(tcb);
            self.next_scheduler = if sel4::benchmark_reset_log().is_ok() {
                now.saturating_add(1000)
            } else {u64::MAX};
        }
        #[cfg(feature = "zc")]
        zc::flush_counts(zc::META_VA);
        Ok(())
    }
}
