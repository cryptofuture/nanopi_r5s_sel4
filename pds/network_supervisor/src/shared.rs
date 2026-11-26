use core::sync::atomic::{AtomicU32,Ordering,fence};
pub fn read<const N:usize>(address:usize)->Option<[u32;N]> {
 let data=unsafe {&*(address as *const [AtomicU32;N])};
 for _ in 0..2 {
  let before=data[0].load(Ordering::Acquire);if before==0 || before&1!=0 {continue;}
  let mut result=[0;N];for i in 0..N {result[i]=data[i].load(Ordering::Relaxed);}
  fence(Ordering::Acquire);
  if data[0].load(Ordering::Acquire)==before {result[0]=before;return Some(result);}
 }
 None
}
pub fn millis()->u64 {let ticks:u64;unsafe {core::arch::asm!("isb","mrs {},cntvct_el0",out(reg)ticks,options(nostack));}ticks/24000}
