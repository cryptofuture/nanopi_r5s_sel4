use crate::{arp, dhcp, nat::Router, *};

const MAC: [[u8; 6]; 3] = [
    [2, 0x52, 0x35, 0x53, 0, 1],
    [2, 0x52, 0x35, 0x53, 0, 2],
    [2, 0x52, 0x35, 0x53, 0, 3],
];
const BROADCAST: [u8; 6] = [255; 6];

#[derive(Clone, Copy)]
struct Neighbor {
    ip: Ip,
    mac: [u8; 6],
    until: u64,
}

#[derive(Clone, Copy)]
struct Offer {
    address: Ip,
    until: u64,
    request: [u8; 240],
}

struct Pending {
    ingress: Port,
    length: usize,
    frame: [u8; 1514],
}

pub struct EthernetRouter {
    router: Router,
    servers: [dhcp::Server; 2],
    neighbors: [[Option<Neighbor>; 32]; 3],
    last_neighbor: [usize; 3],
    resolving: [Option<(Ip, u64)>; 3],
    offers: [Option<Offer>; 2],
    pending: [Option<Pending>; 3],
    mtu: [u16; 3],
    local_only: bool,
    local_dns: [Ip; 3],
    local_dns_count: usize,
    shared_wan: bool,
    next_lease_check: u64,
}

impl EthernetRouter {
    pub fn new(seed: u32) -> Self {
        Self {
            router: Router::new(seed),
            servers: [dhcp::Server::new(Port::Lan1).unwrap(), dhcp::Server::new(Port::Lan2).unwrap()],
            neighbors: [[None; 32]; 3],
            last_neighbor: [0; 3],
            resolving: [None; 3],
            offers: [None; 2],
            pending: [const { None }; 3],
            mtu: [1500; 3],
            local_only: false,
            local_dns: [0; 3],
            local_dns_count: 0,
            shared_wan: false,
            next_lease_check: 0,
        }
    }

    pub fn local_lan(seed: u32, dns: Option<Ip>) -> Self {
        let mut stack = Self::new(seed);
        stack.local_only = true;
        if let Some(ip) = dns {
            stack.set_local_dns(core::slice::from_ref(&ip));
        }
        stack
    }

    pub fn local_lan_dns(seed: u32, dns: &[Ip]) -> Self {
        let mut stack = Self::new(seed);
        stack.local_only = true;
        stack.set_local_dns(dns);
        stack
    }

    pub fn shared_lan_router(seed: u32, dns: Option<Ip>) -> Self {
        let mut stack = Self::local_lan(seed, dns);
        stack.shared_wan = true;
        stack.router.enable_shared_lan();
        stack
    }

    pub fn shared_lan_router_dns(seed: u32, dns: &[Ip]) -> Self {
        let mut stack = Self::local_lan_dns(seed, dns);
        stack.shared_wan = true;
        stack.router.enable_shared_lan();
        stack
    }

    fn set_local_dns(&mut self, dns: &[Ip]) {
        self.local_dns = [0; 3];
        self.local_dns_count = 0;
        for &ip in dns {
            if self.local_dns_count == self.local_dns.len() {
                break;
            }
            if unicast(ip) {
                self.local_dns[self.local_dns_count] = ip;
                self.local_dns_count += 1;
            }
        }
    }

    pub fn track_nat_releases(&mut self) { self.router.track_releases(); }
    pub fn take_nat_releases(&mut self) -> [u32; 8] { self.router.take_releases() }
    pub fn routing_generation(&self) -> u64 { self.router.generation }
    pub fn wan(&self) -> Option<Wan> { self.router.wan }

    pub fn wan_gateway_neighbor(&self, now: u64) -> Option<([u8; 6], u64)> {
        let wan = self.router.wan?;
        if wan.expires <= now { return None; }
        self.neighbors[0].iter().flatten()
            .find(|entry| entry.ip == wan.gateway && entry.until > now)
            .map(|entry| (entry.mac, entry.until.min(wan.expires)))
    }

    pub fn resolve_wan_gateway(&mut self, now: u64, out: &mut [u8; 1514]) -> Result<Option<usize>, Drop> {
        let wan = self.router.wan.ok_or(Drop::NoWan)?;
        if wan.expires <= now { return Err(Drop::NoWan); }
        if self.wan_gateway_neighbor(now).is_some()
            || self.resolving[0].is_some_and(|(_, until)| until > now) {
            return Ok(None);
        }
        let size = arp::request(MAC[0], wan.address, wan.gateway, out)?;
        self.pending[0] = None;
        self.resolving[0] = Some((wan.gateway, now.saturating_add(1000)));
        Ok(Some(size))
    }

    pub fn local_neighbor_count(&self, now: u64) -> usize {
        self.neighbors[1].iter().flatten().filter(|entry| entry.until > now).count()
    }

    pub fn resolved_lan_routes<'a>(&'a self, bridge: &'a bridge::Bridge, links: u8, now: u64)
        -> impl Iterator<Item = (Ip, [u8; 6], usize, u64)> + 'a
    {
        self.neighbors[Port::Lan1 as usize].iter().flatten().filter_map(move |entry| {
            if !self.shared_wan || entry.until <= now { return None; }
            let (port, learned_until) = bridge.learned_destination(&entry.mac, links, now)?;
            Some((entry.ip, entry.mac, port, entry.until.min(learned_until)))
        })
    }

    pub fn nat_slot_usage(&self, now: u64) -> (u32, u32, u32) {
        self.router.slot_usage(now)
    }

    /// Set a port's IPv4 MTU. Flush buffered frames that may exceed it.
    pub fn set_mtu(&mut self, port: Port, mtu: u16) -> Result<(), Drop> {
        if !(68..=1500).contains(&mtu) {
            return Err(Drop::Mtu);
        }
        self.mtu[port as usize] = mtu;
        self.discard_link_pending(port);
        Ok(())
    }

    pub fn configure_wan(&mut self, wan: Option<Wan>, now: u64) -> Result<(), Drop> {
        let generation = self.router.generation;
        let result = self.router.configure(wan, now);
        if result.is_err() {
            self.router.configure(None, now)?;
        }
        if generation != self.router.generation {
            self.neighbors[0] = [None; 32];
            self.resolving = [None; 3];
            self.pending = [const { None }; 3];
        }
        result
    }

    pub fn link_down(&mut self, port: Port, now: u64) {
        self.neighbors[port as usize] = [None; 32];
        self.discard_link_pending(port);
        if port == Port::Wan {
            self.configure_wan(None, now).unwrap();
        } else {
            self.offers[port as usize - 1] = None;
            self.router.reset_lan(port, now);
        }
    }

    fn discard_link_pending(&mut self, port: Port) {
        for (egress, pending) in self.pending.iter_mut().enumerate() {
            if egress == port as usize || pending.as_ref().is_some_and(|frame| frame.ingress == port) {
                *pending = None;
                self.resolving[egress] = None;
            }
        }
    }

    pub fn transmission_failed(&mut self, port: Port, frame: &[u8]) {
        let index = port as usize;
        let Ok((1, _, source, target)) = arp::parse(frame) else { return };
        if source == 0 && index != 0 {
            let lan = index - 1;
            if self.offers[lan].is_some_and(|offer| offer.address == target) {
                self.servers[lan].cancel_probe(target);
                self.offers[lan] = None;
            }
        } else if self.resolving[index].is_some_and(|(hop, _)| hop == target) {
            self.resolving[index] = None;
            self.pending[index] = None;
        }
    }

    fn address(&self, port: Port) -> Result<Ip, Drop> {
        match port {
            Port::Wan => self.router.wan.map(|wan| wan.address).ok_or(Drop::NoWan),
            _ => Ok(LAN[port as usize - 1]),
        }
    }

    fn expire_leases(&mut self, now: u64) {
        for (index, server) in self.servers.iter_mut().enumerate() {
            if server.maintain(now, &mut self.router) {
                self.pending = [const { None }; 3];
                self.resolving = [None; 3];
                self.neighbors[index + 1] = [None; 32];
            }
        }
    }

    pub fn receive(
        &mut self, port: Port, frame: &[u8], now: u64, out: &mut [u8; 1514],
    ) -> Result<Option<(Port, usize)>, Drop> {
        if self.router.wan.is_some_and(|wan| wan.expires <= now) {
            self.link_down(Port::Wan, now);
        }
        if now >= self.next_lease_check {
            self.next_lease_check = now.saturating_add(10);
            self.expire_leases(now);
        }
        if self.shared_wan && port == Port::Lan2 { return Err(Drop::Policy); }
        let index = port as usize;
        if !(14..=1514).contains(&frame.len()) {
            return Err(Drop::Malformed);
        }
        if frame[6] & 1 != 0 || frame[6..12] == [0; 6] {
            return Err(Drop::Spoof);
        }
        let broadcast = frame[..6] == BROADCAST;
        if !broadcast && frame[..6] != MAC[index] {
            return Err(Drop::Policy);
        }
        if frame[12..14] == [8, 6] {
            return self.receive_arp(port, frame, now, out);
        }
        if frame[12..14] != [8, 0] {
            return Err(Drop::Unsupported);
        }

        let ip = &frame[14..];
        let length = validate_ip(ip)?;
        if port != Port::Wan && ip[9] == 17 && length >= 28
            && u16at(ip, 20) == 68 && u16at(ip, 22) == 67
        {
            let destination = ipat(ip, 16);
            if destination != u32::MAX && destination != LAN[index - 1] {
                return Err(Drop::Policy);
            }
            if u16at(ip, 24) as usize != length - 20
                || (u16at(ip, 26) != 0 && transport_checksum(ip, length) != 0)
            {
                return Err(Drop::Checksum);
            }
            let request = &ip[28..length];
            if request.get(28..34) != Some(&frame[6..12]) {
                return Err(Drop::Spoof);
            }
            return self.receive_dhcp(port, request, now, out);
        }
        if broadcast {
            return Err(Drop::Policy);
        }
        if self.local_only && (!self.shared_wan || (port == Port::Lan1 && ipat(ip, 16) == LAN[0])) {
            let source = ipat(ip, 12);
            if port != Port::Lan1 || ipat(ip, 16) != LAN[0] || ip[9] != 1 || length < 28
                || !unicast(source) || (source ^ LAN[0]) & 0xffffff00 != 0
                || source == LAN[0] || matches!(source as u8, 0 | 255)
                || ip[20..22] != [8, 0] || checksum(&ip[20..length]) != 0
            {
                return Err(Drop::Policy);
            }
            header(out, frame[6..12].try_into().unwrap(), MAC[1], 0x0800);
            let reply = &mut out[14..14 + length];
            reply.copy_from_slice(&ip[..length]);
            reply[8] = 64;
            setip(reply, 12, LAN[0]);
            setip(reply, 16, source);
            reply[20] = 0;
            set16(reply, 22, 0);
            let sum = checksum(&reply[20..]);
            set16(reply, 22, sum);
            set16(reply, 10, 0);
            let sum = checksum(&reply[..20]);
            set16(reply, 10, sum);
            return Ok(Some((port, length + 14)));
        }
        out[14..14 + length].copy_from_slice(&ip[..length]);
        let packet = &mut out[14..14 + length];
        let mtu = if port != Port::Wan { self.mtu[0] as usize } else { 1500 };
        let egress = match self.router.forward(port, packet, now, mtu) {
            Ok(egress) => egress,
            Err(Drop::Ttl) if port != Port::Wan => {
                return self.icmp_error(port, frame, length, 11, 0, 0, out);
            }
            Err(Drop::Mtu) if port != Port::Wan && u16at(ip, 6) & 0x4000 != 0 => {
                return self.icmp_error(port, frame, length, 3, 4, mtu as u16, out);
            }
            Err(error) => return Err(error),
        };
        self.finish_forward(port, egress, length, now, out)
    }

    pub fn finish_worker(&mut self, ingress: Port, error: Option<Drop>, length: usize, now: u64,
                         frame: &mut [u8;1514]) -> Result<Option<(Port, usize)>, Drop> {
        match error {
            Some(Drop::Ttl) if ingress == Port::Lan1 => {
                let original = *frame;
                self.icmp_error(ingress, &original, length, 11, 0, 0, frame)
            }
            Some(error) => Err(error),
            None => self.finish_forward(ingress, if ingress == Port::Wan {Port::Lan1} else {Port::Wan}, length, now, frame),
        }
    }

    fn finish_forward(&mut self, port: Port, egress: Port, length: usize, now: u64,
                      out: &mut [u8;1514]) -> Result<Option<(Port, usize)>, Drop> {
        if length > self.mtu[egress as usize] as usize {
            return Err(Drop::Mtu);
        }
        let destination = ipat(out, 30);
        let hop = if egress == Port::Wan {
            self.router.wan.ok_or(Drop::NoWan)?.next_hop(destination)
        } else {
            destination
        };
        let egress_index = egress as usize;
        if let Some(neighbor) = self.forwarding_neighbor(egress_index, hop, now) {
            header(out, neighbor.mac, MAC[egress_index], 0x0800);
            return Ok(Some((egress, length + 14)));
        }
        if let Some((resolving, _)) = self.resolving[egress_index].filter(|&(_, until)| until > now) {
            if resolving == hop && self.pending[egress_index].is_none() {
                self.pending[egress_index] = Some(Pending { ingress: port, length: length + 14, frame: *out });
                return Ok(None);
            }
            return Err(if self.shared_wan { Drop::NeighborBusy } else { Drop::Full });
        }
        self.resolving[egress_index] = Some((hop, now.saturating_add(1000)));
        self.pending[egress_index] = Some(Pending { ingress: port, length: length + 14, frame: *out });
        let size = arp::request(MAC[egress_index], self.address(egress)?, hop, out)?;
        Ok(Some((egress, size)))
    }

    fn forwarding_neighbor(&mut self, port: usize, ip: Ip, now: u64) -> Option<Neighbor> {
        let matches = |entry: Neighbor| entry.ip == ip && entry.until > now;
        let entries = &self.neighbors[port];
        if let Some(entry) = entries[self.last_neighbor[port]].filter(|entry| matches(*entry)) {
            return Some(entry);
        }
        let slot = entries.iter().position(|entry| entry.is_some_and(matches))?;
        self.last_neighbor[port] = slot;
        entries[slot]
    }

    fn icmp_error(
        &self, port: Port, frame: &[u8], length: usize, kind: u8, code: u8,
        mtu: u16, out: &mut [u8; 1514],
    ) -> Result<Option<(Port, usize)>, Drop> {
        let original = &frame[14..14 + length];
        if original[9] == 1 && (length < 28 || !matches!(original[20], 0 | 8)) {
            return Err(if kind == 11 { Drop::Ttl } else { Drop::Mtu });
        }
        let index = port as usize;
        header(out, frame[6..12].try_into().unwrap(), MAC[index], 0x0800);
        let quote = length.min(28);
        let size = 28 + quote;
        let ip = &mut out[14..14 + size];
        ip[..28].fill(0);
        ip[0] = 0x45;
        set16(ip, 2, size as u16);
        ip[8] = 64;
        ip[9] = 1;
        setip(ip, 12, LAN[index - 1]);
        setip(ip, 16, ipat(original, 12));
        ip[20] = kind;
        ip[21] = code;
        set16(ip, 26, mtu);
        ip[28..].copy_from_slice(&original[..quote]);
        set16(ip, 22, checksum(&ip[20..]));
        set16(ip, 10, checksum(&ip[..20]));
        Ok(Some((port, 14 + size)))
    }

    fn receive_arp(
        &mut self, port: Port, frame: &[u8], now: u64, out: &mut [u8; 1514],
    ) -> Result<Option<(Port, usize)>, Drop> {
        let index = port as usize;
        let (operation, mac, source, target) = arp::parse(frame)?;
        let own = self.address(port)?;
        if index != 0 {
            if let Some(offer) = self.offers[index - 1] {
                if source == offer.address || (source == 0 && target == offer.address) {
                    self.offers[index - 1] = None;
                    self.servers[index - 1].probe_result(offer.address, true, now)?;
                }
            }
        }
        if operation == 2 {
            if target != own || frame[32..38] != MAC[index] || frame[..6] != MAC[index]
                || !self.resolving[index].is_some_and(|(hop, until)| hop == source && until > now)
            {
                return Err(Drop::Policy);
            }
            let entries = &mut self.neighbors[index];
            let slot = entries.iter_mut()
                .find(|entry| entry.is_none_or(|n| n.ip == source || n.until <= now))
                .ok_or(Drop::Full)?;
            *slot = Some(Neighbor { ip: source, mac, until: now.saturating_add(60_000) });
            self.resolving[index] = None;
            if let Some(pending) = self.pending[index].take() {
                out[..pending.length].copy_from_slice(&pending.frame[..pending.length]);
                header(out, mac, MAC[index], 0x0800);
                return Ok(Some((port, pending.length)));
            }
            return Ok(None);
        }
        if target != own {
            return Ok(None);
        }
        if self.local_only && unicast(source) && (source ^ own) & 0xffffff00 == 0
            && source != own && !matches!(source as u8, 0 | 255)
        {
            let entries = &mut self.neighbors[index];
            let slot = entries.iter().position(|e| e.is_some_and(|entry| entry.ip == source))
                .or_else(|| entries.iter().position(|e| e.is_none_or(|entry| entry.until <= now)));
            if let Some(slot) = slot {
                entries[slot] = Some(Neighbor { ip: source, mac, until: now.saturating_add(60_000) });
            }
        }
        arp::request(MAC[index], own, source, out)?;
        header(out, mac, MAC[index], 0x0806);
        set16(out, 20, 2);
        out[32..38].copy_from_slice(&mac);
        Ok(Some((port, 60)))
    }

    fn receive_dhcp(
        &mut self, port: Port, request: &[u8], now: u64, out: &mut [u8; 1514],
    ) -> Result<Option<(Port, usize)>, Drop> {
        let index = port as usize - 1;
        if self.offers[index].is_some() && dhcp::options(request)?.kind == 1 {
            return Ok(None);
        }
        let reply = self.servers[index].receive(port, request, now, &mut self.router)?;
        if matches!(reply, dhcp::Reply::Released(_)) {
            self.pending = [const { None }; 3];
            self.resolving = [None; 3];
            self.neighbors[index + 1] = [None; 32];
        }
        if let dhcp::Reply::Probe(address) = reply {
            self.offers[index] = Some(Offer {
                address,
                until: now.saturating_add(1000),
                request: request[..240].try_into().unwrap(),
            });
            return Ok(Some((port, arp::request(MAC[index + 1], 0, address, out)?)));
        }
        self.dhcp_reply(port, request, reply, out)
    }

    pub fn tick_lan(
        &mut self, port: Port, now: u64, out: &mut [u8; 1514],
    ) -> Result<Option<(Port, usize)>, Drop> {
        if port == Port::Wan {
            return Err(Drop::Policy);
        }
        self.expire_leases(now);
        if self.router.wan.is_some_and(|wan| wan.expires <= now) {
            self.link_down(Port::Wan, now);
        }
        let index = port as usize - 1;
        let Some(offer) = self.offers[index].filter(|offer| now >= offer.until) else {
            return Ok(None);
        };
        self.offers[index] = None;
        let reply = self.servers[index].probe_result(offer.address, false, now)?;
        let mut request = [0; 244];
        request[..240].copy_from_slice(&offer.request);
        request[240..].copy_from_slice(&[53, 1, 1, 255]);
        self.dhcp_reply(port, &request, reply, out)
    }

    fn dhcp_reply(
        &self, port: Port, request: &[u8], reply: dhcp::Reply, out: &mut [u8; 1514],
    ) -> Result<Option<(Port, usize)>, Drop> {
        let index = port as usize - 1;
        let (kind, address) = match reply {
            dhcp::Reply::Offer(ip) => (2, ip),
            dhcp::Reply::Ack(ip) => (5, ip),
            dhcp::Reply::Nak => (6, 0),
            _ => return Ok(None),
        };
        let dns = if self.local_dns_count != 0 || (self.local_only && !self.shared_wan) {
            &self.local_dns[..self.local_dns_count]
        } else {
            self.router.wan.as_ref().map_or(&[][..], |wan| &wan.dns[..wan.dns_count])
        };
        let payload = self.servers[index].response_with_routes(request, kind, address, dns,
            !self.local_only || self.shared_wan, &mut out[42..])?;
        let ciaddr = ipat(request, 12);
        let broadcast = kind == 6 || (ciaddr == 0 && u16at(request, 10) & 0x8000 != 0);
        let destination = if broadcast { u32::MAX } else if ciaddr != 0 { ciaddr } else { address };
        let client = request[28..34].try_into().unwrap();
        header(out, if broadcast { BROADCAST } else { client }, MAC[index + 1], 0x0800);
        let ip = &mut out[14..42 + payload];
        ip[..28].fill(0);
        ip[0] = 0x45;
        set16(ip, 2, (28 + payload) as u16);
        ip[8] = 64;
        ip[9] = 17;
        setip(ip, 12, LAN[index]);
        setip(ip, 16, destination);
        set16(ip, 20, 67);
        set16(ip, 22, 68);
        set16(ip, 24, (8 + payload) as u16);
        set16(ip, 10, checksum(&ip[..20]));
        let udp_checksum = transport_checksum(ip, ip.len());
        set16(ip, 26, if udp_checksum == 0 { u16::MAX } else { udp_checksum });
        Ok(Some((port, 42 + payload)))
    }
}

fn header(frame: &mut [u8], destination: [u8; 6], source: [u8; 6], kind: u16) {
    frame[..6].copy_from_slice(&destination);
    frame[6..12].copy_from_slice(&source);
    set16(frame, 12, kind);
}
