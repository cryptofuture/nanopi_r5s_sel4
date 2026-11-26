use r5s_router_core::{dhcp, ipat, transport_checksum, u16at, validate_ip};

#[derive(Default)]
pub struct Progress {
    pub bits: u32,
    xid: Option<u32>,
    request: Option<(u32, Option<u32>, u32)>,
    server: Option<u32>,
}

impl Progress {
    pub fn accepts(&self, packet: &[u8], opt: &dhcp::Options) -> bool {
        let xid = ipat(packet, 4);
        if !opt.server.is_some_and(r5s_router_core::unicast) { return false; }
        match opt.kind {
            2 => self.xid == Some(xid) && self.request.is_none(),
            5 | 6 => self.request.is_some_and(|(sent, server, address)| {
                xid == sent && (server.is_none() || server == opt.server)
                    && (opt.kind == 6 || ipat(packet, 16) == address)
            }),
            _ => false,
        }
    }

    pub fn observe(&mut self, frame: &[u8], outgoing: bool, mac: [u8; 6]) {
        if frame.get(12..14) != Some(&[8, 0]) {
            return;
        }
        let ip = &frame[14..];
        let Ok(len) = validate_ip(ip) else { return };
        let ports = if outgoing { (68, 67) } else { (67, 68) };
        if ip[9] != 17 || len < 268 || (u16at(ip, 20), u16at(ip, 22)) != ports
            || u16at(ip, 24) as usize != len - 20
            || (u16at(ip, 26) != 0 && transport_checksum(ip, len) != 0)
        {
            return;
        }
        let packet = &ip[28..len];
        if packet[28..34] != mac {
            return;
        }
        let options = if outgoing { dhcp::options(packet) } else { dhcp::reply_options(packet) };
        let Ok(options) = options else { return };
        let xid = ipat(packet, 4);
        match (outgoing, options.kind) {
            (true, 1) => {
                self.xid = Some(xid);
                self.request = None; self.server = None;
                self.bits |= 1;
            }
            (false, 2) if self.xid == Some(xid) && options.server.is_some() => self.bits |= 2,
            (true, 3) => {
                if options.server.is_some() { self.server = options.server; }
                let selected = if options.server.is_none() && ipat(ip,16)==u32::MAX {None} else {self.server};
                self.request = Some((xid, selected, options.requested.unwrap_or_else(|| ipat(packet,12))));
                self.bits |= 4;
            }
            (false, 5) if self.accepts(packet,&options) => {
                self.server = options.server;
                #[cfg(feature="lan-status")]
                { self.bits |= 1 << 8; }
            }
            _ => {}
        }
    }
}
