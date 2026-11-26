include!("observation_timing.rs");
use core::sync::atomic::{AtomicU32,Ordering};
#[used]
#[unsafe(no_mangle)]
pub static mut IO_HANDOFF:[u32;64]=[0x494f5231;64];
pub fn bits(a:u32,b:u32,c:u32)->u8 {
 ((a>>1)&1) as u8 | (((a>>5)&1)<<1) as u8 | (((b>>1)&1)<<2) as u8 | (((b>>4)&1)<<3) as u8 | (((b>>6)&1)<<4) as u8 | (((c>>1)&1)<<5) as u8 | (((c>>4)&1)<<6) as u8 | (((c>>6)&1)<<7) as u8
}
pub fn words()->Option<[u8;5]> {
 let native=unsafe {&*(0x40002300 as *const [AtomicU32;4])};
 if native[0].load(Ordering::Acquire)!=0x494f5331 {return None;}
 let n=bits(native[1].load(Ordering::Relaxed),native[2].load(Ordering::Relaxed),native[3].load(Ordering::Relaxed));
 let mut h=[0u32;64];for i in 0..64 {h[i]=unsafe {core::ptr::read_volatile(core::ptr::addr_of!(IO_HANDOFF).cast::<u32>().add(i))};}
 let valid=h[0]==0x494f5232 && h[1]==1 && h[63]==h[..63].iter().fold(0u32,|a,b|a.rotate_left(1)^b);
 let before=if valid {bits(h[2],h[3],h[4])}else{0};
 let mut completed=0u8;let mut detail=0u8;
 for i in 0..4 {
  let r=&h[8+i*10..8+i*10+10];
  let matches=match i {0=>r[6]&0x22==0x20,1=>r[7]&2==0 && r[8]&2==2,2=>r[7]&16==16 && r[8]&16==0,_=>r[7]&64==64 && r[8]&64==0};
  let ok=valid && r[0]==5 && r[1]==0 && r[4]==0 && r[5]==0 && r[9]==0 && matches;
  if ok {completed|=1<<i;} else if detail==0 {
   let stage=if !valid {0}else if r[0]>5 {7}else if r[0]==5 && (r[4]!=0 || r[5]!=0) {6}else if r[0]==5 && (r[9]!=0 || !matches) {7}else{r[0] as u8};
   detail=1+(i as u8)*8+stage;
  }
 }
 Some([43,before&127,n&127,(before>>7)|((n>>7)<<1)|(completed<<2)|((valid as u8)<<6),detail])
}
pub struct Observation {latched:Option<([u8;5],u64)>}
impl Observation {
 pub fn new()->Self {Self {latched:None}}
 pub fn tick(&mut self,now:u64)->Option<bool> {
  if self.latched.is_none() {self.latched=Some((words()?,now));}
  let (codes,start)=self.latched.unwrap();let t=now.saturating_sub(start)%SEQUENCE_MS;
  if t<DELIMITER_MS {return Some(false);}
  let t=t-DELIMITER_MS;if t>=5*WORD_MS {return Some(false);}
  let index=(t/WORD_MS) as usize;let mut display=crate::fault_display::FaultDisplay::new();
  display.latch(codes[index],0);Some(display.tick(t%WORD_MS+DELIMITER_MS))
 }
}
