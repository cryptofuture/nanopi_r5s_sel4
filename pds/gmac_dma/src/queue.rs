use core::sync::atomic::{AtomicU32,Ordering};
pub const BASE:usize=0x61000000;
pub const SLOT:usize=1536;
pub const COUNT:u32=8;
pub const DIRECTION:usize=0x4000;
fn atomic(base:usize,offset:usize)->&'static AtomicU32 {unsafe {&*((base+offset) as *const AtomicU32)}}
pub fn push(base:usize,generation:u32,data:&[u8])->Result<bool,()> {
 if !(14..=1514).contains(&data.len()) {return Err(());}
 let head=atomic(base,0).load(Ordering::Relaxed);let tail=atomic(base,4).load(Ordering::Acquire);
 if head.wrapping_sub(tail)>COUNT {return Err(());}
 if head.wrapping_sub(tail)==COUNT {return Ok(false);}
 let address=base+64+(head%COUNT) as usize*SLOT;
 unsafe {core::ptr::write_volatile(address as *mut u32,data.len() as u32);core::ptr::write_volatile((address+4) as *mut u32,generation);core::ptr::copy_nonoverlapping(data.as_ptr(),(address+8) as *mut u8,data.len());}
 atomic(base,0).store(head.wrapping_add(1),Ordering::Release);Ok(true)
}
pub fn pop(base:usize,generation:u32,out:&mut [u8])->Result<usize,()> {
 let tail=atomic(base,4).load(Ordering::Relaxed);let head=atomic(base,0).load(Ordering::Acquire);
 if head.wrapping_sub(tail)>COUNT {return Err(());}
 if head==tail {return Ok(0);}
 let address=base+64+(tail%COUNT) as usize*SLOT;
 let size=unsafe {core::ptr::read_volatile(address as *const u32)} as usize;
 let epoch=unsafe {core::ptr::read_volatile((address+4) as *const u32)};
 if !(14..=1514).contains(&size) || size>out.len() {return Err(());}
 if epoch==generation {unsafe {core::ptr::copy_nonoverlapping((address+8) as *const u8,out.as_mut_ptr(),size);}}
 atomic(base,4).store(tail.wrapping_add(1),Ordering::Release);
 Ok(if epoch==generation {size}else{0})
}
