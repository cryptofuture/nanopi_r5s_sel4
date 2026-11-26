pub fn reply(frame: &[u8], mac: [u8; 6]) -> Option<[u8; 42]> {
    const IP: [u8; 4] = [10, 44, 1, 1];
    let arp = frame.get(14..42)?;
    if frame[..6] != [255; 6]
        || frame[12..14] != [8, 6]
        || arp[..8] != [0, 1, 8, 0, 6, 4, 0, 1]
        || arp[24..28] != IP
        || frame[6..12] != arp[8..14]
        || frame[6] & 1 != 0
        || frame[6..12] == [0; 6]
        || mac == [0; 6]
        || mac[0] & 1 != 0
    {
        return None;
    }
    let mut out = [0; 42];
    out[..6].copy_from_slice(&frame[6..12]);
    out[6..12].copy_from_slice(&mac);
    out[12..22].copy_from_slice(&frame[12..22]);
    out[21] = 2;
    out[22..28].copy_from_slice(&mac);
    out[28..32].copy_from_slice(&IP);
    out[32..42].copy_from_slice(&arp[8..18]);
    Some(out)
}

#[test]
fn broadcast_request_has_matching_unicast_reply() {
    let mut request = [0u8; 60];
    let peer = [2, 1, 2, 3, 4, 5];
    let mac = [2, 6, 7, 8, 9, 10];
    request[..6].fill(255);
    request[6..12].copy_from_slice(&peer);
    request[12..22].copy_from_slice(&[8, 6, 0, 1, 8, 0, 6, 4, 0, 1]);
    request[22..28].copy_from_slice(&peer);
    request[28..32].copy_from_slice(&[10, 44, 1, 2]);
    request[38..42].copy_from_slice(&[10, 44, 1, 1]);
    let reply = reply(&request, mac).unwrap();
    assert_eq!(reply[..6], peer);
    assert_eq!(reply[22..28], mac);
    assert_eq!(reply[28..32], [10, 44, 1, 1]);
    assert_eq!(reply[32..42], request[22..32]);
    assert_eq!(reply[21], 2);
    for index in [0, 6, 12, 18, 21, 41] {
        request[index] ^= 1;
        assert!(self::reply(&request, mac).is_none());
        request[index] ^= 1;
    }
    assert!(self::reply(&request[..41], mac).is_none());
}
