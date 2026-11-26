use crate::{forward_shared::{self, Control, INPUT_WAN, LAN_TAG_MASK, LAN_TAG_SHIFT, TAG_SHIFT, TTL, WORKERS, base},
    queue, echo_trace, send, send_wan, stat};
#[cfg(feature = "zc")]
use crate::{zc, zc_send, zc_send_wan};
use core::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use r5s_router_core::{Drop, Port, bridge::{Bridge, MAC}, ethernet::EthernetRouter, steering::{self,Steering}, u16at};
use sel4_microkit::Channel;

#[cfg(feature = "zc")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Egress {
    Wan(u32),
    Lan(u8),
}

#[cfg(feature = "gro")]
const GRO_FRAGMENTS: usize = if zc::LARGE_GRO { crate::tso::TSO_MAX_FRAGS } else { 2 };

#[cfg(feature = "gro")]
struct Aggregate {
    egress: Egress,
    refs: [zc::Ref; GRO_FRAGMENTS],
    count: usize,
    started: u64,
}

pub struct Dispatch {
    state: Control,
    steering: Steering,
    pending: u8,
    sample_at: u64,
    maintenance_at: u64,
    #[cfg(feature = "zc")]
    retry_at: u64,
    next_output: usize,
    residency: [u32;WORKERS],
    queue_peak: [u32;WORKERS],
    #[cfg(feature = "zc")]
    gateway: Option<(u32, [u8; 6], u64)>,
    #[cfg(feature = "gro")]
    aggregate: Option<Aggregate>,
}
impl Dispatch {
    pub const fn new() -> Self {
        Self {state:Control::new(), steering:Steering::new(), pending:0, sample_at:0, maintenance_at:0, next_output:0, residency:[0;WORKERS], queue_peak:[0;WORKERS],
              #[cfg(feature = "zc")]
              retry_at:0,
              #[cfg(feature = "zc")]
              gateway:None,
              #[cfg(feature = "gro")]
              aggregate:None}
    }
    pub fn update(&mut self, stack: &mut EthernetRouter, link_epoch: Option<u32>) {
        let generation = stack.routing_generation() as u32;
        let wan = stack.wan();
        self.state.publish(generation, wan, link_epoch, stack.take_nat_releases());
    }
    #[cfg(feature = "zc")]
    pub fn update_wan_route(&mut self, stack: &mut EthernetRouter, link_epoch: Option<u32>,
                            now: u64, frame: &mut [u8; 1514]) {
        if !zc::worker_wan_egress() { return; }
        let (mac, until) = stack.wan_gateway_neighbor(now).and_then(|(mac, until)| {
            let expires = stack.wan()?.expires.min(until);
            (expires > now && link_epoch.is_some()).then_some((mac, expires))
        }).unwrap_or(([0; 6], 0));
        let value = (self.state.revision, mac, until);
        if self.gateway != Some(value) {
            self.gateway = Some(value);
            zc::publish_wan_route(zc::WORKER_WAN_ROUTE, value.0, mac, until);
        }
        if link_epoch.is_some() && until == 0
            && let Ok(Some(size)) = stack.resolve_wan_gateway(now, frame)
        {
            send_wan(stack, link_epoch, &frame[..size]);
        }
    }
    fn owner(&mut self, port: Port, frame: &[u8],
        #[cfg(feature = "zc")] admission: &mut Option<zc::TransportPin>) -> Option<usize> {
        let wan = port == Port::Wan;
        if frame.len() < 34 { return None; }
        if self.state.wan.is_none() || frame[6]&1 != 0 || frame[6..12] == [0;6]
            || frame[..6] != if wan {[2,0x52,0x35,0x53,0,1]} else {MAC} { return None; }
        #[cfg(feature = "zc")]
        if !wan && zc::lan_fast_path() {
            if let Some((key, hash)) = steering::transport_endpoint(&frame[14..]) {
                *admission = zc::pin_transport(key, hash, |worker| {
                    zc::RefQueue::depth(forward_shared::request_ref(worker)).unwrap_or(zc::RQ_COUNT)
                });
                return admission.as_ref().map(|pin| pin.worker);
            }
        }
        if wan { steering::inbound(&frame[14..]) }
        else { self.steering.outbound(&frame[14..], |n| {
            #[cfg(feature = "zc")]
            let (depth, capacity) = (zc::RefQueue::depth(forward_shared::request_ref(n)).unwrap_or(zc::RQ_COUNT), zc::RQ_COUNT);
            #[cfg(not(feature = "zc"))]
            let (depth, capacity) = (queue::depth(base(n)).unwrap_or(queue::COUNT), queue::COUNT);
            let active=forward_shared::counter(n,2).load(Relaxed)&1023;
            if depth>=capacity || active>=128 {u32::MAX} else {depth*128+active}
        }) }
    }
    pub fn dispatch(&mut self, port: Port, frame: &[u8], ports: u8, verified:u32) {
        let wan = port == Port::Wan;
        #[cfg(feature = "zc")]
        let mut admission = None;
        let owner = self.owner(port, frame, #[cfg(feature = "zc")] &mut admission);
        let queued = owner.is_some_and(|owner| {
            let tag = self.state.revision << TAG_SHIFT | u32::from(wan);
            let copy = || queue::push_meta(base(owner),tag,frame,verified);
            #[cfg(feature = "zc")]
            let result = zc::RefQueue::push_copy(forward_shared::request_ref(owner), copy);
            #[cfg(not(feature = "zc"))]
            let result = copy();
            if result != Ok(true) { return false; }
            self.pending |= 1<<owner;
            let depth = queue::depth(base(owner)).unwrap_or(0);
            self.queue_peak[owner]=self.queue_peak[owner].max(depth);
            true
        });
        stat(if queued {81} else {64}).fetch_add(1,Relaxed);
        echo_trace(if wan {2}else{0},frame,if queued {8}else{6},ports);
    }
    /// Zero-copy dispatch: hand the pool reference to the owning worker. The
    /// payload stays in the pool; only the reference is queued. A reference
    /// that cannot be queued is released here.
    #[cfg(feature = "zc")]
    pub fn dispatch_ref(&mut self, port: Port, r: &zc::Ref, frame: &[u8], ports: u8) {
        let wan = port == Port::Wan;
        let mut admission = None;
        let owner = self.owner(port, frame, &mut admission);
        let mut queued = false;
        echo_trace(if wan {2}else{0},frame,if owner.is_some() {8}else{6},ports);
        if let Some(owner) = owner {
            let mut request = *r;
            if wan { request.flags |= zc::FLAG_WAN; }
            request.epoch = self.state.revision;
            if zc::RefQueue::push(forward_shared::request_ref(owner), &request) == Ok(true) {
                queued = true;
                self.pending |= 1 << owner;
                let depth = zc::RefQueue::depth(forward_shared::request_ref(owner)).unwrap_or(0);
                self.queue_peak[owner]=self.queue_peak[owner].max(depth);
            }
        }
        if !queued {
            zc::count(zc::META_VA, zc::C_FALLBACK_QUEUE);
            let _ = zc::reject(zc::META_VA, r);
        }
        stat(if queued {81} else {64}).fetch_add(1,Relaxed);
    }
    pub fn complete(&mut self, stack: &mut EthernetRouter, bridge: &mut Bridge, links: u8,
                    epoch: Option<u32>, now: u64, frame: &mut [u8;1514]) -> bool {
        let mut any=false;
        #[cfg(feature = "zc")]
        let mut learns = zc::LearnBatch::new();
        for offset in 0..WORKERS {
            let worker=(self.next_output+offset)%WORKERS;
            for _ in 0..queue::batch(option_env!("R5S_DATAPATH_BATCH"), 4) {
                #[cfg(feature = "zc")]
                let token = {
                    let Ok(Some(r)) = zc::RefQueue::pop(forward_shared::result_ref(worker)) else { break };
                    any=true;
                    if r.flags & zc::FLAG_SOURCE_LEARN != 0 {
                        if r.epoch == self.state.revision {
                            let lan = (r.flags & zc::LAN_INGRESS_MASK) >> zc::LAN_INGRESS_SHIFT;
                            if lan != 0 {
                                let port = u16::from(lan - 1);
                                let mac = zc::learned_mac(r);
                                if !learns.push(port, mac) {
                                    Self::apply_learns(&mut learns, bridge, links, now);
                                    let _ = learns.push(port, mac);
                                }
                            }
                        }
                        continue;
                    }
                    // Coalesced learns are applied before any frame so the
                    // per-record apply order is preserved.
                    Self::apply_learns(&mut learns, bridge, links, now);
                    if r.flags & zc::FLAG_COPY == 0 {
                        self.complete_ref(stack, bridge, links, epoch, now, r);
                        continue;
                    }
                    (r.flags & zc::FLAG_LAN_TOKEN != 0).then_some(r.meta)
                };
                #[cfg(not(feature = "zc"))]
                let token: Option<u32> = None;
                #[cfg(feature = "gro")]
                self.flush_aggregate(stack);
                let Ok((size,tag)) = queue::pop_tagged(base(worker)+queue::DIRECTION,frame) else {
                    stat(64).fetch_add(1,Relaxed);
                    #[cfg(feature = "zc")]
                    Self::retire_token(token);
                    break;
                };
                if size==0 {
                    #[cfg(feature = "zc")]
                    Self::retire_token(token);
                    break;
                }
                any=true;
                if tag>>TAG_SHIFT != self.state.revision || epoch.is_none() {
                    stat(64).fetch_add(1,Relaxed);
                    #[cfg(feature = "zc")]
                    Self::retire_token(token);
                    continue;
                }
                let lan = (tag & LAN_TAG_MASK) >> LAN_TAG_SHIFT;
                if !Self::learn_ingress(bridge, lan, &frame[..size], links, now) {
                    stat(64).fetch_add(1,Relaxed);
                    #[cfg(feature = "zc")]
                    Self::retire_token(token);
                    continue;
                }
                let ingress=if tag&INPUT_WAN != 0 {Port::Wan}else{Port::Lan1};
                let ip_length=u16at(frame,16) as usize;
                if !(20..=size-14).contains(&ip_length) {
                    stat(64).fetch_add(1,Relaxed);
                    #[cfg(feature = "zc")]
                    Self::retire_token(token);
                    continue;
                }
                match stack.finish_worker(ingress,(tag&TTL!=0).then_some(Drop::Ttl),ip_length,now,frame) {
                    Ok(Some((Port::Wan,length))) => {
                        #[cfg(feature = "zc")]
                        Self::retire_token(token);
                        send_wan(stack,epoch,&frame[..length])
                    }
                    Ok(Some((Port::Lan1,length))) => {
                        stat(19).fetch_add(1,Relaxed);
                        send(bridge.destination(&frame[..6],links,now),&frame[..length],false,token,false);
                    }
                    _ => {
                        stat(64).fetch_add(1,Relaxed);
                        #[cfg(feature = "zc")]
                        Self::retire_token(token);
                    }
                }
            }
        }
        #[cfg(feature = "gro")]
        self.flush_aggregate(stack);
        #[cfg(feature = "zc")]
        Self::apply_learns(&mut learns, bridge, links, now);
        self.next_output=(self.next_output+1)%WORKERS;
        any
    }
    /// Apply the coalesced cross-worker source learns in insertion order.
    #[cfg(feature = "zc")]
    fn apply_learns(learns: &mut zc::LearnBatch, bridge: &mut Bridge, links: u8, now: u64) {
        learns.apply(|port, mac| bridge.learn_source(usize::from(port), mac, links, now));
    }
    /// Finish one zero-copy result: L2 rewrite happens in place in the pool
    /// buffer, then the reference moves to the egress driver. Fallback copies
    /// only when the neighbor is unresolved or the frame must be flooded.
    /// Every outcome settles a download-fallback return token exactly once —
    /// by transfer to the egress send, or by retiring it at the drop.
    #[cfg(feature = "zc")]
    fn retire_token(token: Option<u32>) {
        if let Some(token) = token {
            zc::retire_lan_token(zc::META_VA, token);
        }
    }
    #[cfg(feature = "zc")]
    fn retire_ref_token(r: &zc::Ref) {
        if r.flags & zc::FLAG_LAN_TOKEN != 0 {
            zc::retire_lan_token(zc::META_VA, r.meta);
        }
    }
    #[cfg(feature = "zc")]
    fn complete_ref(&mut self, stack: &mut EthernetRouter, bridge: &mut Bridge, links: u8,
                    epoch: Option<u32>, now: u64, r: zc::Ref) {
        if r.epoch != self.state.revision || epoch.is_none() {
            Self::retire_ref_token(&r);
            let _ = zc::reject(zc::META_VA, &r);
            stat(64).fetch_add(1,Relaxed);
            return;
        }
        let length = r.len as usize;
        if zc::status(zc::META_VA, r.id()) != (zc::CPU, r.generation()) {
            zc::count(zc::META_VA, zc::C_OWNERSHIP_ERRORS);
            Self::retire_ref_token(&r);
            return;
        }
        if !(34..=1514).contains(&length) {
            Self::retire_ref_token(&r);
            let _ = zc::reject(zc::META_VA, &r);
            stat(64).fetch_add(1,Relaxed);
            return;
        }
        let buffer = unsafe {
            core::slice::from_raw_parts_mut(
                (zc::POOL_VA + r.id() as usize * zc::BUFFER) as *mut u8, zc::BUFFER)
        };
        let lan = u32::from((r.flags & zc::LAN_INGRESS_MASK) >> zc::LAN_INGRESS_SHIFT);
        if !Self::learn_ingress(bridge, lan, &buffer[..length], links, now) {
            Self::retire_ref_token(&r);
            let _ = zc::reject(zc::META_VA, &r);
            stat(64).fetch_add(1, Relaxed);
            return;
        }
        let ip_length=u16at(buffer,16) as usize;
        if !(20..=length-14).contains(&ip_length) {
            Self::retire_ref_token(&r);
            let _ = zc::reject(zc::META_VA, &r);
            stat(64).fetch_add(1,Relaxed);
            return;
        }
        let ingress=if r.flags & zc::FLAG_WAN != 0 {Port::Wan}else{Port::Lan1};
        let ttl=(r.flags & zc::FLAG_TTL != 0).then_some(Drop::Ttl);
        let scratch: &mut [u8;1514] = match <&mut [u8;1514]>::try_from(&mut buffer[..1514]) {Ok(value)=>value,Err(_)=>{
            Self::retire_ref_token(&r);
            let _ = zc::reject(zc::META_VA, &r); stat(64).fetch_add(1,Relaxed); return;
        }};
        match stack.finish_worker(ingress,ttl,ip_length,now,scratch) {
            Ok(Some((Port::Wan,length))) => {
                let mut done=r;
                done.len=length as u16;
                self.output_ref(stack,Egress::Wan(epoch.unwrap()),done);
            }
            Ok(Some((Port::Lan1,length))) => {
                stat(19).fetch_add(1,Relaxed);
                let mut done=r;
                done.len=length as u16;
                self.output_ref(stack,Egress::Lan(bridge.destination(&buffer[..6],links,now)),done);
            }
            _ => {
                Self::retire_ref_token(&r);
                let _ = zc::reject(zc::META_VA, &r);
                stat(64).fetch_add(1,Relaxed);
            }
        }
    }

    fn learn_ingress(bridge: &mut Bridge, lan: u32, frame: &[u8], links: u8, now: u64) -> bool {
        lan == 0 || bridge.receive((lan - 1) as usize, frame, links, now) == Some((true, 0))
    }
    #[cfg(feature = "zc")]
    fn output_ref(&mut self, stack: &mut EthernetRouter, egress: Egress, reference: zc::Ref) {
        #[cfg(feature = "gro")]
        {
            // A return token must survive untouched: never rewrite the ref
            // into a TSO group (that replaces its flags).
            if reference.flags & zc::FLAG_LAN_TOKEN != 0 {
                self.flush_aggregate(stack);
                Self::emit_ref(stack, egress, reference);
                return;
            }
            let frame = Self::frame(&reference);
            let started = Self::gro_time();
            let hold = match option_env!("R5S_GRO_HOLD_US") {
                Some("100") => 100,
                Some("500") => 500,
                Some("1000") => 1000,
                _ => 250,
            };
            if self.aggregate.as_ref().is_some_and(|group| started.saturating_sub(group.started) >= hold) {
                zc::count(zc::META_VA, zc::C_GRO_TIMEOUT);
                self.flush_aggregate(stack);
            }
            let target = match egress {
                Egress::Wan(_) if zc::wan_fast_path() && zc::wan_fast_packet(frame, false) => Some(zc::FAST_WAN_TX),
                Egress::Lan(1) => Some(zc::link_queue(zc::LINKS_VA, zc::Q_LAN1_TX)),
                Egress::Lan(2) => Some(zc::link_queue(zc::LINKS_VA, zc::Q_LAN2_TX)),
                _ => None,
            };
            if let Some(queue) = target.filter(|_| crate::tso::gro_payload(frame).is_some()) {
                zc::count(zc::META_VA, zc::C_GRO_ELIGIBLE);
                if let Some(group) = self.aggregate.as_mut() {
                    let previous = Self::frame(&group.refs[group.count - 1]);
                    let mss = (group.egress == egress
                        && group.refs[..group.count].iter().all(|r| r.id() != reference.id()))
                        .then(|| crate::tso::gro_follow(previous, frame)).flatten();
                    if let Some(mss) = mss {
                        if frame[47] == 0x18
                            && zc::RefQueue::depth(queue).is_ok_and(|depth| depth + (group.count as u32) < zc::RQ_COUNT)
                        {
                            group.refs[group.count] = reference;
                            group.count += 1;
                            let group = self.aggregate.take().unwrap();
                            Self::submit_aggregate(queue, group, mss);
                            return;
                        }
                        if frame[47] == 0x10 && group.count + 1 < GRO_FRAGMENTS {
                            group.refs[group.count] = reference;
                            group.count += 1;
                            return;
                        }
                        zc::count(zc::META_VA, zc::C_GRO_FALLBACK);
                    } else {
                        zc::count(zc::META_VA, zc::C_GRO_INCOMPATIBLE);
                    }
                    self.flush_aggregate(stack);
                }
                if frame[47] == 0x10 {
                    self.aggregate = Some(Aggregate { egress, refs: [reference; GRO_FRAGMENTS], count: 1, started });
                    return;
                }
            }
            self.flush_aggregate(stack);
        }
        Self::emit_ref(stack, egress, reference);
    }

    #[cfg(feature = "gro")]
    fn submit_aggregate(queue: usize, mut group: Aggregate, mss: u16) {
        let refs = &mut group.refs[..group.count];
        if option_env!("R5S_GRO_TSO") == Some("1") {
            let flags = (zc::POOL_VA + refs[0].id() as usize * zc::BUFFER + 47) as *mut u8;
            unsafe { flags.write(0x18); }
        }
        for (index, r) in refs.iter_mut().enumerate() {
            if index == 0 {
                r.flags = crate::tso::FLAG_TSO_FIRST | crate::tso::FLAG_TSO_GRO;
                r.meta = crate::tso::pack_first_meta(mss, false, 20);
            } else {
                let frame = unsafe {core::slice::from_raw_parts_mut(
                    (zc::POOL_VA + r.id() as usize * zc::BUFFER) as *mut u8, usize::from(r.len))};
                frame.copy_within(54.., 0);
                r.flags = if index + 1 == group.count { crate::tso::FLAG_TSO_LAST } else { crate::tso::FLAG_TSO_CONT };
                r.meta = 0;
                r.len = mss;
            }
            if let Egress::Wan(epoch) = group.egress { r.epoch = epoch; }
        }
        if zc::RefQueue::push_batch(queue, refs) == Ok(true) {
            zc::count(zc::META_VA, zc::C_GRO_GROUPS);
            zc::counter(zc::META_VA, zc::C_GRO_SEGMENTS).fetch_add(group.count as u32, Relaxed);
        } else {
            for r in refs { let _ = zc::reject(zc::META_VA, r); }
            zc::count(zc::META_VA, zc::C_GRO_FALLBACK);
        }
    }

    #[cfg(feature = "gro")]
    fn gro_time() -> u64 {
        let ticks: u64;
        unsafe {core::arch::asm!("isb; mrs {}, cntvct_el0", out(reg) ticks, options(nostack));}
        ticks / 24
    }

    #[cfg(feature = "gro")]
    fn flush_aggregate(&mut self, stack: &mut EthernetRouter) {
        if let Some(group) = self.aggregate.take() {
            zc::count(zc::META_VA, zc::C_GRO_FLUSH);
            for &reference in &group.refs[..group.count] {
                Self::emit_ref(stack, group.egress, reference);
            }
        }
    }

    #[cfg(feature = "zc")]
    fn frame(reference: &zc::Ref) -> &[u8] {
        unsafe {core::slice::from_raw_parts(
            (zc::POOL_VA + reference.id() as usize * zc::BUFFER) as *const u8, usize::from(reference.len))}
    }

    #[cfg(feature = "zc")]
    fn emit_ref(stack: &mut EthernetRouter, egress: Egress, reference: zc::Ref) {
        let frame = Self::frame(&reference);
        match egress {
            Egress::Wan(epoch) => zc_send_wan(stack,Some(epoch),&reference,frame),
            Egress::Lan(mask) => zc_send(mask,&reference,frame,false),
        }
    }

    pub fn notify(&mut self, now: u64) {
        let maintenance = now >= self.maintenance_at;
        if now>=self.sample_at {
            self.sample_at=now.saturating_add(10);
            let mut slots=0;
            for worker in 0..WORKERS {
                let sample=|index| forward_shared::counter(worker,index).load(Acquire);
                #[cfg(feature = "sched-stats")]
                for n in 0..6 { stat(84+6*worker+n).store(sample(4+n),Relaxed); }
                #[cfg(feature = "zc")]
                for i in 0..forward_shared::WORKER_DIAG_WORDS {
                    stat(160+worker*forward_shared::WORKER_DIAG_WORDS+i)
                        .store(sample(forward_shared::WORKER_DIAG+i),Relaxed);
                }
                let occupancy=sample(2);
                slots+=occupancy;
                #[cfg(not(feature = "zc"))]
                let queue=queue::depth(base(worker)).unwrap_or(0);
                #[cfg(feature = "zc")]
                let queue=zc::RefQueue::depth(forward_shared::request_ref(worker)).unwrap_or(0);
                let queue=queue.min(15);
                let peak=self.queue_peak[worker];
                for (i,value) in [sample(0),sample(1),occupancy&1023 | queue<<10 | peak<<14,sample(3)].into_iter().enumerate() {
                    stat(65+4*worker+i).store(value,Relaxed);
                }
                if maintenance {
                    self.pending|=1<<worker;
                    let snapshot = crate::shared::read::<{steering::BITMAP_WORDS+3}>(forward_shared::residency(worker));
                    if let Some(words) = snapshot {
                        #[cfg(feature = "zc")]
                        let refs_drained = words[2]==zc::RefQueue::produced(forward_shared::request_ref(worker))
                            && zc::RefQueue::depth(forward_shared::result_ref(worker))==Ok(0);
                        #[cfg(not(feature = "zc"))]
                        let refs_drained = true;
                        if refs_drained && self.residency[worker] != words[0] && words[1]==queue::produced(base(worker))
                            && crate::queue::depth(base(worker)+queue::DIRECTION)==Ok(0)
                        {
                            self.steering.release_idle(worker,words[3..].try_into().unwrap());
                            self.residency[worker]=words[0];
                        }
                    }
                }
            }
            stat(50).store((512<<20)|slots,Release);
            if maintenance {
                #[cfg(feature = "zc")]
                let interval = if zc::lan_fast_path() { 100 } else { 1000 };
                #[cfg(not(feature = "zc"))]
                let interval = 1000;
                self.maintenance_at = now.saturating_add(interval);
            }
        }
        #[cfg(feature = "zc")]
        let retry = now >= self.retry_at;
        #[cfg(feature = "zc")]
        if retry { self.retry_at = now.saturating_add(1); }
        for worker in 0..WORKERS {
            let pending = self.pending & (1 << worker) != 0;
            #[cfg(feature = "zc")]
            let pending = {
                let request = forward_shared::request_ref(worker);
                let work = zc::RefQueue::depth(request).is_ok_and(|depth| depth != 0);
                let held = retry && forward_shared::counter(worker,forward_shared::LAN_RETRY).load(Acquire) != 0;
                (work || held || pending && maintenance) && zc::RefQueue::claim_wake(request)
            };
            if pending { Channel::new(worker + 8).notify(); }
        }
        self.pending=0;
    }
}
