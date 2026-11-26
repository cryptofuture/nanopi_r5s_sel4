//! Single-owner Ethernet forwarding service. Each driver queue has one producer
//! and consumer. The adapter is responsible for release/acquire publication and
//! for discarding frames whose generation no longer matches the link.

use crate::{ethernet::EthernetRouter, Drop, Port, Wan};

const PORTS: [Port; 3] = [Port::Wan, Port::Lan1, Port::Lan2];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    pub up: bool,
    pub epoch: u32,
}

pub trait Frames {
    /// None represents a temporarily inconsistent snapshot.
    fn link(&mut self, port: Port) -> Option<Link>;
    fn take(&mut self, port: Port, epoch: u32, out: &mut [u8; 1514]) -> Result<usize, Drop>;
    fn send(&mut self, port: Port, epoch: u32, frame: &[u8]) -> Result<(), Drop>;
}

pub struct Service {
    router: EthernetRouter,
    links: [Option<Link>; 3],
    ingress: [u8; 1514],
    egress: [u8; 1514],
}

impl Service {
    pub fn new(seed: u32) -> Self {
        Self {
            router: EthernetRouter::new(seed),
            links: [None; 3],
            ingress: [0; 1514],
            egress: [0; 1514],
        }
    }

    /// Only a current WAN generation can publish a lease from the DHCP owner.
    pub fn lease(&mut self, epoch: u32, config: Option<Wan>, now: u64) -> Result<(), Drop> {
        if self.links[0] != Some(Link { up: true, epoch }) {
            return Err(Drop::State);
        }
        self.router.configure_wan(config, now)
    }

    pub fn poll(&mut self, io: &mut impl Frames, now: u64) -> [Option<Drop>; 3] {
        let mut failures = [None; 3];
        let mut samples = [None; 3];
        for port in PORTS {
            let index = port as usize;
            samples[index] = io.link(port);
            if let Some(sample) = samples[index] {
                if self.links[index] != Some(sample) {
                    self.router.link_down(port, now);
                    self.links[index] = Some(sample);
                }
            }
        }

        for port in PORTS {
            let index = port as usize;
            let Some(Link { up: true, epoch }) = samples[index] else { continue };
            let result = (|| {
                let length = io.take(port, epoch, &mut self.ingress)?;
                let output = if length == 0 {
                    if port == Port::Wan {
                        None
                    } else {
                        self.router.tick_lan(port, now, &mut self.egress)?
                    }
                } else {
                    let frame = self.ingress.get(..length).ok_or(Drop::Malformed)?;
                    self.router.receive(port, frame, now, &mut self.egress)?
                };
                if let Some((destination, len)) = output {
                    let frame = &self.egress[..len];
                    let sent = match samples[destination as usize] {
                        Some(Link { up: true, epoch }) => io.send(destination, epoch, frame),
                        _ => Err(Drop::State),
                    };
                    if sent.is_err() {
                        self.router.transmission_failed(destination, frame);
                    }
                    sent?;
                }
                Ok(())
            })();
            failures[index] = result.err();
        }
        failures
    }
}
