use core::sync::atomic::{compiler_fence,Ordering};
pub const POOL:usize=0x60000000;
pub const SIZE:usize=0x40000;
pub fn maintain(op:u32,address:usize,length:usize)->Result<(),u32> {
 if op>2 || length==0 || address<POOL || address%64!=0 || length%64!=0 || address-POOL>=SIZE || length>SIZE-(address-POOL) {return Err(100);}
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
pub fn preflight()->Result<(),u32> {
 for offset in (0..SIZE).step_by(4) {if offset%4096==0 {crate::hw_progress();}unsafe {core::ptr::write_volatile((POOL+offset) as *mut u32,(offset as u32)^0xa55a1234);}}
 maintain(0,POOL,SIZE)?;maintain(2,POOL,SIZE)?;maintain(1,POOL,SIZE)?;
 for offset in (0..SIZE).step_by(4) {if offset%4096==0 {crate::hw_progress();}if unsafe {core::ptr::read_volatile((POOL+offset) as *const u32)}!=(offset as u32)^0xa55a1234 {return Err(101);}}
 Ok(())
}
