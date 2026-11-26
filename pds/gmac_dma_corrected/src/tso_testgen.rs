//! Board-side TSO test source (docs/tso-design.md §9,
//! docs/tso-hardware-validation.md). Synthesizes valid oversized TCP
//! transmit requests — Ethernet+IP+TCP headers and a declared MSS with a
//! payload larger than one segment — without relying on ordinary NAT
//! traffic. Driver PDs built with the `tso-test` feature run it at startup
//! and submit the requests through the same TX interface a router-originated
//! stack would use, so packet capture on a receiving machine can verify
//! actual hardware segmentation.
//!
//! `core`-only outside `#[cfg(test)]`, standalone like `tso.rs`.

#![allow(dead_code)]

use crate::tso::{
    self, GroupMeta, TsoError, TsoFrag, TsoRequest, TSO_MAX_FRAGS,
};

/// Generation parameters for one oversized TCP request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TestSpec {
    /// 4 or 6.
    pub ip_version: u8,
    /// Segment payload bytes (64..=2047).
    pub mss: u16,
    /// Total TCP payload bytes; must exceed `mss` so segmentation is real.
    pub payload_len: u32,
    /// Base TCP sequence number.
    pub seq: u32,
    /// Base IPv4 identification (ignored for IPv6).
    pub ip_id: u16,
    pub src_port: u16,
    pub dst_port: u16,
    /// Nonzero completion identifier.
    pub tso_id: u32,
}

impl TestSpec {
    /// The default board-validation request: IPv4, MSS 1460, 16000 bytes.
    pub fn default_ipv4(tso_id: u32) -> Self {
        Self {
            ip_version: 4,
            mss: 1460,
            payload_len: 16000,
            seq: 0x1000_0000,
            ip_id: 0x2000,
            src_port: 40000,
            dst_port: 5201,
            tso_id,
        }
    }
}

fn put16(out: &mut [u8], at: usize, value: u16) {
    out[at..at + 2].copy_from_slice(&value.to_be_bytes());
}

fn put32(out: &mut [u8], at: usize, value: u32) {
    out[at..at + 4].copy_from_slice(&value.to_be_bytes());
}

/// Fill `buffers` with one oversized TCP packet and return its request
/// description. `addrs[i]` becomes `frags[i].physical` — device-visible
/// bus addresses for hardware submission, CPU-visible addresses for the
/// software fallback's reader. `buffers[0]` receives the headers followed
/// by the first payload bytes; the remaining buffers receive payload
/// continuation in order. Every filled fragment length is at most its
/// buffer length and at least 1, matching the shared validation rules.
pub fn build(
    spec: &TestSpec,
    buffers: &mut [&mut [u8]],
    addrs: &[u64],
) -> Result<TsoRequest, TsoError> {
    if buffers.is_empty() || buffers.len() > TSO_MAX_FRAGS || addrs.len() != buffers.len() {
        return Err(TsoError::BadFragCount);
    }
    if spec.mss < tso::TSO_MIN_MSS || spec.mss > tso::TSO_MAX_MSS {
        return Err(TsoError::BadMss);
    }
    if spec.payload_len <= u32::from(spec.mss) {
        return Err(TsoError::BadPayload);
    }
    let tcp_hdr_len: u8 = 20;
    let ip_hdr_len: u8 = if spec.ip_version == 6 { 40 } else if spec.ip_version == 4 { 20 }
        else { return Err(TsoError::BadIpVersion) };
    let head_len = 14usize + usize::from(ip_hdr_len) + usize::from(tcp_hdr_len);
    let total = head_len + spec.payload_len as usize;
    let capacity: usize = buffers.iter().map(|b| b.len()).sum();
    if capacity < total {
        return Err(TsoError::BadFragLength);
    }

    // Flatten the packet into the fragment buffers: headers, then payload
    // pattern bytes (index & 0xff) so captures can verify segmentation.
    let mut frags = [TsoFrag { physical: 0, length: 0 }; TSO_MAX_FRAGS];
    let mut written = 0usize;
    let mut remaining = total;
    for (i, buffer) in buffers.iter_mut().enumerate() {
        let take = remaining.min(buffer.len());
        for (offset, byte) in buffer[..take].iter_mut().enumerate() {
            let at = written + offset;
            *byte = if at < head_len { 0 } else { (at - head_len) as u8 };
        }
        frags[i] = TsoFrag { physical: addrs[i], length: take as u32 };
        written += take;
        remaining -= take;
    }

    // Headers live contiguously at the start of the first fragment.
    let head = &mut buffers[0][..head_len];
    head[..6].copy_from_slice(&[0x02, 0, 0, 0, 0, 0x02]); // destination MAC
    head[6..12].copy_from_slice(&[0x02, 0, 0, 0, 0, 0x01]); // source MAC
    if spec.ip_version == 4 {
        head[12..14].copy_from_slice(&[0x08, 0x00]);
        head[14] = 0x45;
        head[15] = 0;
        put16(head, 16, (20 + tcp_hdr_len as u16 + spec.mss) as u16);
        put16(head, 18, spec.ip_id);
        put16(head, 20, 0x4000); // don't fragment
        head[22] = 64;
        head[23] = 6; // TCP
        head[24] = 0;
        head[25] = 0;
        head[26..30].copy_from_slice(&[10, 44, 1, 1]);
        head[30..34].copy_from_slice(&[10, 44, 1, 2]);
    } else {
        head[12..14].copy_from_slice(&[0x86, 0xdd]);
        head[14] = 0x60;
        head[15] = 0;
        put16(head, 18, (tcp_hdr_len as u16 + spec.mss) as u16);
        head[20] = 6; // next header: TCP
        head[21] = 64;
        head[22..38].copy_from_slice(&[0x20, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        head[38..54].copy_from_slice(&[0x20, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]);
    }
    let tcp = 14 + ip_hdr_len as usize;
    put16(head, tcp, spec.src_port);
    put16(head, tcp + 2, spec.dst_port);
    put32(head, tcp + 4, spec.seq);
    put32(head, tcp + 8, 0); // acknowledgement
    head[tcp + 12] = 5 << 4; // data offset: 5 words, no options
    head[tcp + 13] = 0x18;
    put16(head, tcp + 14, 0xffff); // window
    put16(head, tcp + 16, 0); // checksum: recomputed by hardware/fallback
    put16(head, tcp + 18, 0); // urgent

    let meta = GroupMeta {
        mss: spec.mss,
        ipv6: spec.ip_version == 6,
        tcp_hdr_len,
    };
    let mut request = tso::request_from_group(&meta, &frags, spec.tso_id)?;
    if option_env!("R5S_TSO_TEST_SOFTWARE") == Some("1") {
        request.flags |= tso::PROBE_SOFTWARE;
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::RefCell;

    /// Aligned backing store for fragment buffers: DMA addresses must be
    /// 64-byte aligned, so the tests expose 2048-byte chunks at aligned
    /// addresses of one contiguous allocation.
    struct Pool {
        _raw: Vec<u8>,
        base: usize,
        chunks: usize,
    }

    impl Pool {
        fn new(chunks: usize) -> Self {
            let raw = vec![0u8; chunks * 2048 + 64];
            let base = (raw.as_ptr() as usize + 63) & !63;
            Self { _raw: raw, base, chunks }
        }
        fn addrs(&self) -> Vec<u64> {
            (0..self.chunks).map(|i| (self.base + i * 2048) as u64).collect()
        }
        fn buffers(&self) -> Vec<&mut [u8]> {
            (0..self.chunks)
                .map(|i| unsafe {
                    core::slice::from_raw_parts_mut((self.base + i * 2048) as *mut u8, 2048)
                })
                .collect()
        }
        fn chunk(&self, i: usize) -> &[u8] {
            unsafe { core::slice::from_raw_parts((self.base + i * 2048) as *const u8, 2048) }
        }
    }

    #[test]
    fn builds_a_valid_ipv4_request_over_pool_buffers() {
        let spec = TestSpec::default_ipv4(1);
        let pool = Pool::new(8);
        let addrs = pool.addrs();
        let mut buffers = pool.buffers();
        let request = build(&spec, &mut buffers, &addrs).unwrap();
        assert_eq!(request.ip_version, 4);
        assert_eq!(request.head_len, 54);
        assert_eq!(request.payload_len, 16000);
        assert_eq!(request.mss, 1460);
        assert_eq!(request.nfrags, 8);
        assert_eq!(request.flags & tso::PROBE_SOFTWARE != 0, option_env!("R5S_TSO_TEST_SOFTWARE") == Some("1"));
        assert_eq!(request.flags & tso::GRO_SOFTWARE, 0);
        // Headers land at the start of fragment 0.
        let head = &pool.chunk(0)[..54];
        assert_eq!(&head[..6], &[0x02, 0, 0, 0, 0, 0x02]);
        assert_eq!(&head[12..14], &[0x08, 0x00]);
        assert_eq!(head[23], 6);
        // Payload pattern starts right after the headers.
        assert_eq!(pool.chunk(0)[54], 0);
        assert_eq!(pool.chunk(0)[55], 1);
    }

    #[test]
    fn build_rejects_short_capacity_and_bad_specs() {
        let spec = TestSpec::default_ipv4(1);
        let mut small = vec![0u8; 2048];
        let mut refs: Vec<&mut [u8]> = vec![small.as_mut_slice()];
        assert_eq!(build(&spec, &mut refs, &[0x8200_0000]), Err(TsoError::BadFragLength));
        let mut ok = vec![0u8; 2048];
        let mut refs: Vec<&mut [u8]> = vec![ok.as_mut_slice()];
        let mut bad = spec;
        bad.mss = 63;
        assert_eq!(build(&bad, &mut refs, &[0x8200_0000]), Err(TsoError::BadMss));
        let mut bad = spec;
        bad.payload_len = 1460;
        assert_eq!(build(&bad, &mut refs, &[0x8200_0000]), Err(TsoError::BadPayload));
        let mut bad = spec;
        bad.ip_version = 5;
        assert_eq!(build(&bad, &mut refs, &[0x8200_0000]), Err(TsoError::BadIpVersion));
    }

    #[test]
    fn generated_request_segments_into_mtu_frames_with_correct_checksums() {
        let spec = TestSpec::default_ipv4(2);
        let pool = Pool::new(8);
        let addrs = pool.addrs();
        let mut buffers = pool.buffers();
        let request = build(&spec, &mut buffers, &addrs).unwrap();
        drop(buffers);
        let mut scratch = vec![0u8; 14 + 40 + 60 + 2047];
        let emitted = RefCell::new(Vec::new());
        let count = tso::segment(&request, &mut scratch, &mut |frame: &[u8]| {
            emitted.borrow_mut().push(frame.to_vec());
            Ok(())
        })
        .unwrap();
        assert_eq!(count, (16000 + 1459) / 1460);
        let frames = emitted.borrow();
        for (k, frame) in frames.iter().enumerate() {
            assert!(frame.len() <= 1514);
            let payload = &frame[54..];
            assert_eq!(payload.len(), if k + 1 == count as usize { 16000 - 1460 * k as usize }
                      else { 1460 });
            // Sequence advances by MSS; IDs advance by 1 per segment.
            assert_eq!(&frame[38..42], &(0x1000_0000 + 1460 * k as u32).to_be_bytes());
            assert_eq!(&frame[18..20], &(0x2000u16.wrapping_add(k as u16)).to_be_bytes());
            assert_eq!(frame[47], if k + 1 == count as usize { 0x18 } else { 0x10 });
            // IP header checksum verifies (sum including checksum == 0xffff).
            let sum: u32 = frame[14..34].chunks_exact(2)
                .map(|p| u32::from(u16::from_be_bytes([p[0], p[1]]))).sum();
            let sum = (sum & 0xffff) + (sum >> 16);
            assert_eq!(sum + (sum >> 16), 0xffff);
        }
    }

    #[test]
    fn generated_ipv6_request_segments_with_pseudo_header_checksums() {
        let mut spec = TestSpec::default_ipv4(3);
        spec.ip_version = 6;
        spec.mss = 1440; // IPv6: 14 + 40 + 20 + 1440 = 1514
        let pool = Pool::new(8);
        let addrs = pool.addrs();
        let mut buffers = pool.buffers();
        let request = build(&spec, &mut buffers, &addrs).unwrap();
        drop(buffers);
        assert_eq!(request.head_len, 74);
        let mut scratch = vec![0u8; 14 + 40 + 60 + 2047];
        let emitted = RefCell::new(Vec::new());
        let count = tso::segment(&request, &mut scratch, &mut |frame: &[u8]| {
            emitted.borrow_mut().push(frame.to_vec());
            Ok(())
        })
        .unwrap();
        assert_eq!(count, (16000 + 1439) / 1440);
        let frames = emitted.borrow();
        for frame in frames.iter() {
            assert!(frame.len() <= 1514);
            // TCP checksum verifies over the IPv6 pseudo-header: the
            // one's-complement sum of source+destination addresses, the
            // TCP length, the next-header word and the TCP segment with
            // checksum included equals 0xffff.
            let tcp_len = (frame.len() - 54) as u32;
            let mut sum = 0u32;
            let body = &frame[22..];
            for pair in body[..body.len() / 2 * 2].chunks_exact(2) {
                sum += u32::from(u16::from_be_bytes([pair[0], pair[1]]));
            }
            if body.len() % 2 != 0 {
                sum += u32::from(body[body.len() - 1]) << 8;
            }
            sum += tcp_len >> 16;
            sum += tcp_len & 0xffff;
            sum += 6;
            while sum > 0xffff {
                sum = (sum & 0xffff) + (sum >> 16);
            }
            assert_eq!(sum, 0xffff);
        }
    }
}
