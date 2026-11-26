use core::sync::atomic::{compiler_fence,Ordering};
pub use crate::exercise::{POOL,SIZE};
pub fn maintain(op:u32,address:usize,length:usize)->Result<(),u32> {
 if op>2 || length==0 || address<POOL || address%64!=0 || length%64!=0 || address-POOL>=SIZE || length>SIZE-(address-POOL) {return Err(100);}
 // The kernel and GMAC PD map this arena with matching Normal-NC attributes.
 if matches!(option_env!("R5S_GMAC_DMA_CACHE"), Some(v) if matches!(v.as_bytes(), b"nc")) {
  unsafe {core::arch::asm!("dsb sy",options(nostack));}
  return Ok(());
 }
 compiler_fence(Ordering::SeqCst);
 let mut start=address;let mut left=length;
 while left!=0 {
  let part=left.min(4096-start%4096);
  let error=sel4::with_ipc_buffer_mut(|buffer| {
   let raw=buffer.inner_mut();
   match op {
    0=>raw.seL4_ARM_VSpace_Clean_Data(3,start as _,(start+part) as _),
    1=>raw.seL4_ARM_VSpace_Invalidate_Data(3,start as _,(start+part) as _),
    _=>raw.seL4_ARM_VSpace_CleanInvalidate_Data(3,start as _,(start+part) as _),
   }
  });
  if error!=0 {return Err(error as u32);}
  start+=part;left-=part;crate::hw_progress();
 }
 unsafe {core::arch::asm!("dsb sy",options(nostack));}
 compiler_fence(Ordering::SeqCst);Ok(())
}
struct Scratch;
impl crate::exercise::Backend for Scratch {
 fn call(&mut self,op:u32,start:usize,end:usize)->u32 {
  match maintain(op,start,end-start) {Ok(())=>0,Err(e)=>e}
 }
 fn write(&mut self,address:usize,value:u32) {unsafe {core::ptr::write_volatile(address as *mut u32,value);}}
 fn read(&mut self,address:usize)->u32 {unsafe {core::ptr::read_volatile(address as *const u32)}}
 fn progress(&mut self) {crate::hw_progress();}
}
pub fn preflight()->Result<(),u32> {
 for page in 0..SIZE/4096 {if let Err(e)=crate::exercise::page(&mut Scratch,page) {return Err(if e.readback {101}else{e.syscall});}}
 Ok(())
}
