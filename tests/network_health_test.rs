#[path="../pds/network_supervisor/src/health.rs"] mod health;
fn status(sequence:u32,phase:u32,time:u32)->[u32;24] {
 let mut v=[0;24];v[0]=sequence;v[1]=1;v[20]=phase;v[21]=0;v[22]=0;v[23]=2;v[4]=time;v[5]=time;v[6]=1000;v[7]=1;v[12]=1;v[14]=1;v
}
#[test] fn noninstant_initialization_and_stalls() {
 for start in [0,40,99] {
  let mut h=health::Health::new(0);assert!(!h.check(None,start).unwrap());
  assert!(!h.check(Some(status(2,1,start)),start).unwrap());
  assert!(!h.check(None,2000).unwrap());
  let mut v=status(4,2,2200);v[21]=2200;assert!(!h.check(Some(v),2200).unwrap());
  v=status(6,3,3000);v[21]=3000;assert!(!h.check(Some(v),3000).unwrap());
  v[0]=8;v[2]=2;v[4]=3100;v[5]=3100;assert!(h.check(Some(v),3100).unwrap());
  assert_eq!(h.check(None,3201),Err(6));
 }
 let mut h=health::Health::new(0);assert_eq!(h.check(None,101),Err(6));
 let mut h=health::Health::new(0);assert!(!h.check(Some(status(2,1,0)),0).unwrap());assert_eq!(h.check(None,3001),Err(9));
 let mut h=health::Health::new(0);let v=status(2,2,0);h.check(Some(v),0).unwrap();assert_eq!(h.check(None,1501),Err(9));
}
#[test] fn time_ordering_abi_and_no_deadline_refresh() {
 assert_eq!(health::age(99,100),Err(8));assert_eq!(health::age(100,100),Ok(0));assert_eq!(health::age(2,u32::MAX-2),Ok(5));
 let mut h=health::Health::new(0);h.check(Some(status(2,1,0)),0).unwrap();
 let mut v=status(4,1,50);v[21]=50;assert_eq!(h.check(Some(v),50),Err(4));
 for field in [1,23] {let mut h=health::Health::new(0);let mut v=status(2,1,0);v[field]=99;assert_eq!(h.check(Some(v),0),Err(4));}
 let mut h=health::Health::new(0);let mut v=status(2,3,10);v[2]=2;v[5]=10;assert!(h.check(Some(v),10).unwrap());
 v[0]=4;v[2]=1;v[4]=20;v[5]=20;assert!(!h.check(Some(v),20).unwrap());
 v[0]=6;v[2]=2;v[4]=30;v[5]=30;assert!(h.check(Some(v),30).unwrap());
}

#[test]
fn preempted_app_writer_does_not_latch_fault() {
    let mut health = health::AppFreshness::new(0);
    let mut app = [0; 24];
    app[1] = 1;
    app[2] = 200;
    assert_eq!(health.check(Some(&app), 200), Ok(()));
    assert_eq!(health.check(None, 201), Ok(()));
    assert_eq!(health.check(None, 300), Ok(()));
    assert_eq!(health.check(None, 301), Err(7));
    app[7] = 1;
    assert_eq!(health.check(Some(&app), 201), Err(7));
}

#[path="../pds/network_supervisor/src/fault_code.rs"] mod fault_code;
#[test]
fn driver_return_and_supervisor_timeout_are_distinct() {
    let mut v = status(2,3,100);
    v[2] = 3;
    v[3] = 22;
    assert_eq!(fault_code::select(4, Some(v)), 34);
    assert_eq!(fault_code::select(6, None), 6);
    v[3] = 255;
    assert_eq!(fault_code::select(4, Some(v)), 0);
}

#[path="../pds/wan_dhcp_freshness/src/link.rs"] mod wan_link;
#[test]
fn wan_keeps_lease_through_an_incomplete_snapshot() {
    assert!(wan_link::observed_link(None, true));
    assert!(!wan_link::observed_link(None, false));
    let mut sample = [0u32;20];
    sample[2] = 1;
    assert!(!wan_link::observed_link(Some(&sample), true));
}
