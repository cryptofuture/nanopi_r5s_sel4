#[path="../pds/preflight_fault_id/src/contract.rs"] mod contract;
#[path="../pds/preflight_fault_id/src/registry.rs"] mod registry;
#[path="../pds/preflight_fault_id/src/report.rs"] mod report;
#[path="../pds/preflight_fault_id/src/fault_display.rs"] mod fault_display;
use std::sync::atomic::{AtomicU32,Ordering};
const ID:[u32;4]=[1,2,3,4];
#[test] fn returned_errors_and_exceptions_are_distinct() {
 let p=contract::Producer::new();p.begin(1,ID);p.phase(contract::CACHE,2);
 let mut seen=std::collections::BTreeSet::new();
 for op in 0..3 {for error in 1..=10 {
  let p=contract::Producer::new();p.begin(1,ID);p.phase(contract::CACHE,2);p.fail(3,3,error,op,0x60000000,0x60001000,0,0);
  let code=report::select(Some(p.snapshot()),None,103,ID);assert_ne!(code,0);assert!(seen.insert(code));
  let mut d=fault_display::FaultDisplay::new();d.latch(code,0);d.latch(0,1);
  let bits=fault_display::word(code).unwrap();assert_eq!(fault_display::decode(bits),Some(code));
 }}
 let mut exception=[0;report::WORDS];exception[1]=1;exception[17..21].copy_from_slice(&ID);exception[21]=contract::PROFILE;exception[2]=2;exception[3]=6;
 let code=report::select(None,Some(exception),103,ID);assert_ne!(code,0);assert!(!seen.contains(&code));
 assert_ne!(code,report::select(None,None,103,ID));
 p.fail(3,3,2,0,0,0,0,0);let first=report::select(Some(p.snapshot()),Some(exception),103,ID);
 assert_eq!(first,registry::lookup(3,0,2));
}
#[test] fn partial_publications_are_bounded() {
 let wire=[const {AtomicU32::new(0)};report::WORDS];assert!(report::read(&wire).is_none());
 wire[0].store(1,Ordering::Release);assert!(report::read(&wire).is_none());
 wire[0].store(0,Ordering::Release);let mut data=[0;report::WORDS];data[1]=1;data[17..21].copy_from_slice(&ID);data[21]=contract::PROFILE;data[2]=2;data[3]=8;
 report::publish(&wire,&data);assert_eq!(report::read(&wire).unwrap()[1..],data[1..]);
 assert_ne!(report::select(None,report::read(&wire),103,ID),report::select(None,None,103,ID));
}
