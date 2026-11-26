pub fn next(stage: u8, bits: u32, cursor: &mut u8) -> u8 {
    let visible = ([1u32, 3, 5, 13][stage as usize]) | if stage == 3 { (bits & 0x3fff) << 4 } else { 0 };
    loop {
        let candidate = *cursor;
        *cursor = (candidate + 1) % 18;
        if visible & (1 << candidate) != 0 {
            return if candidate < 12 {116 + candidate} else {109 + candidate - 12};
        }
    }
}

#[cfg(test)]
#[test]
fn live_lease_and_echo_are_separate_reports() {
    let mut cursor = 0;
    assert_eq!((0..8).map(|_| next(3, 0b1011, &mut cursor)).collect::<Vec<_>>(),
               [116, 118, 119, 120, 121, 123, 116, 118]);
    assert_eq!(next(3, 32, &mut 9), 125);
    assert_eq!(next(1, 255, &mut 9), 116);
}

#[cfg(any(feature="lan-gated",test))]
pub fn gated(stage:u8, bits:u32, cursor:&mut u8, router_health:bool, leds:u32) -> (bool,u8) {
    let visible = [1u32,3,5,13][stage as usize] | if stage==3 {(bits & 0x3fff)<<4} else {0};
    loop {
        let index = *cursor;
        *cursor = (index+1)%21;
        match index {
            18 => return (true, if router_health {1} else {3}),
            19 if leds&1!=0 => return (true,2),
            20 if leds&2!=0 => return (true,4),
            0..=17 if visible & (1<<index)!=0 =>
                return (false, if index<12 {116+index} else {109+index-12}),
            _ => {}
        }
    }
}

#[cfg(test)]
#[test]
fn banked_reports_preserve_every_legacy_word() {
    let mut cursor=0;
    let words=(0..20).map(|_|gated(3,0x3fff,&mut cursor,true,3)).collect::<Vec<_>>();
    for id in (116..=127).filter(|v|*v!=117).chain(109..=114) { assert!(words.contains(&(false,id))); }
    assert!(words.ends_with(&[(true,1),(true,2),(true,4)]));
    assert_eq!(gated(0,0,&mut 18,false,0),(true,3));
}
