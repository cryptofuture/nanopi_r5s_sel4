#[path="../pds/preflight_fault_id/src/fault_display.rs"] mod display;
use display::*;
#[test] fn all_words_and_corruption() {
 for id in 0..128 {let bits=word(id).unwrap();assert_eq!(decode(bits),Some(id));
  for bit in 0..8 {assert_eq!(decode(bits^(1<<bit)),None);}
 }
 assert_eq!(word(128),None);
}
#[test] fn complete_frames_and_latch() {
 for id in 0..128 {
  let mut d=FaultDisplay::new();assert!(!d.tick(0));d.latch(id,100);d.latch(id^1,200);
  for elapsed in 0..FRAME_MS*3 {
   let t=elapsed%FRAME_MS;
   let expected=if t<6000 {t<2000 && t%250<125}
    else if (6000..18000).contains(&t) || (22000..34000).contains(&t) {
     let (n,pos)=if t<18000 {((t-6000)/3000,(t-6000)%3000)}else{(4+(t-22000)/3000,(t-22000)%3000)};
     pos<if word(id).unwrap()&(1<<(7-n))!=0 {2000}else{400}
    }else{false};
   assert_eq!(d.tick(600+elapsed),expected);
  }
 }
}
