#[path="../pds/iodomain_observe/src/fault_display.rs"] mod fault_display;
#[path="../pds/iodomain_observe/src/observation.rs"] mod observation;
unsafe extern "C" {fn mmap(a:*mut u8,n:usize,p:i32,f:i32,fd:i32,o:isize)->*mut u8;}
fn regs(n:u8)->[u32;3] { [((n as u32&1)<<1)|(((n as u32>>1)&1)<<5),(((n as u32>>2)&1)<<1)|(((n as u32>>3)&1)<<4)|(((n as u32>>4)&1)<<6),(((n as u32>>5)&1)<<1)|(((n as u32>>6)&1)<<4)|(((n as u32>>7)&1)<<6)] }
fn record(before:u8,after:u8,valid:bool,stage:u32) {
 unsafe {
  let h=core::ptr::addr_of_mut!(observation::IO_HANDOFF).cast::<u32>();
  for i in 0..64 {h.add(i).write(0);}
  h.write(0x494f5232);h.add(1).write(1);
  for (i,v) in regs(before).iter().enumerate() {h.add(2+i).write(*v);}
  for i in 0..4 {let r=h.add(8+i*10);r.write(stage);for (j,v) in [0x20,0x50,2].iter().enumerate() {r.add(6+j).write(*v);}}
  let mut checksum=0u32;for i in 0..63 {checksum=checksum.rotate_left(1)^h.add(i).read();}h.add(63).write(if valid {checksum}else{checksum^1});
  let n=0x40002300 as *mut u32;n.write(0x494f5331);for (i,v) in regs(after).iter().enumerate() {n.add(1+i).write(*v);}
 }
}
#[test] fn actual_snapshot_and_renderer() {
 unsafe {assert_eq!(mmap(0x40002000 as *mut u8,4096,3,0x32,-1,0),0x40002000 as *mut u8);}
 for before in 0..=255u8 {for after in 0..=255u8 {
  record(before,after,true,5);let w=observation::words().unwrap();assert_eq!(w[0],43);
  assert_eq!(w[1]|((w[3]&1)<<7),before);assert_eq!(w[2]|(((w[3]>>1)&1)<<7),after);assert_eq!((w[3]>>2)&15,15);assert_eq!(w[4],0);
 }}
 for stage in 0..5 {record(58,0,true,stage);let w=observation::words().unwrap();assert_eq!((w[3]>>2)&15,0);assert_eq!(w[4],1+stage as u8);}
 for (field,value,detail) in [(4,5,7),(5,5,7),(9,5,8),(7,0,8)] {
  record(58,0,true,5);
  unsafe {let h=core::ptr::addr_of_mut!(observation::IO_HANDOFF).cast::<u32>();h.add(8+field).write(value);if field==7 {h.add(8+6).write(0);}let mut sum=0u32;for i in 0..63 {sum=sum.rotate_left(1)^h.add(i).read();}h.add(63).write(sum);}
  assert_eq!(observation::words().unwrap()[4],detail);
 }
 record(58,0,false,5);assert_eq!(observation::words().unwrap()[3]&64,0);
 record(58,0,true,5);let codes=observation::words().unwrap();let mut d=observation::Observation::new();assert_eq!(d.tick(0),Some(false));
 for t in (0..445000).step_by(10) {
  let phase=t%222500;
  let expected=if phase<500 || phase>=210500 {false}else{let p=phase-500;let mut f=fault_display::FaultDisplay::new();f.latch(codes[(p/42000) as usize],0);f.tick(p%42000+500)};
  assert_eq!(d.tick(t),Some(expected));
 }
 record(0,0,false,0);assert_eq!(d.tick(222500+6500),Some(true));
 for id in 0..128 {let w=fault_display::word(id).unwrap();for bit in 0..8 {assert!(fault_display::decode(w^(1<<bit)).is_none());}}
}
