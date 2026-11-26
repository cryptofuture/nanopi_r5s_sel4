#[path="../pds/gmac_dma_corrected/src/wan_led.rs"] mod led;
use std::cell::RefCell;
use std::collections::BTreeMap;

#[test]
fn only_the_wan_pin_and_its_gate_change() {
    let original=BTreeMap::from([((1,0x30),0xffff),((3,0x37c),0xffff),((6,4),0xffff),((6,12),0),((6,0x78),1)]);
    let regs=RefCell::new(original.clone());
    assert!(led::setup(|r,o|regs.borrow()[&(r,o)],|r,o,v| {
        assert!(led::allowed(r,o,v));
        let mut map=regs.borrow_mut();
        let entry=map.get_mut(&(r,o)).unwrap();
        *entry=(*entry & !(v>>16)) | (v & (v>>16));
    }));
    for (key,mask) in [((1,0x30),0xf0),((3,0x37c),16),((6,4),2),((6,12),2)] {
        assert_eq!(regs.borrow()[&key]&!mask,original[&key]&!mask);
    }
    assert!(!led::allowed(6,4,0xffff0000));
    assert!(!led::setup(|_,_|u32::MAX,|r,o,v|assert_eq!((r,o,v),(3,0x37c,0x100000))));
}
#[test]
fn activity_is_rate_limited_and_link_loss_goes_dark() {
    let mut i=led::Indicator::default();
    assert_eq!(i.update(0,true,0),Some(0x20002));
    assert_eq!(i.update(1,true,1),None);
    assert_eq!(i.update(100,true,1),Some(0x20000));
    assert_eq!(i.update(200,true,1),Some(0x20002));
    assert_eq!(i.update(300,true,1),None);
    assert_eq!(i.update(400,false,1),Some(0x20000));
}
