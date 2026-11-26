#![no_std]
#![no_main]
mod queue;mod shared;mod probe;mod identity;
use core::sync::atomic::{AtomicU32,Ordering};
use sel4_microkit::{protection_domain,Handler,Infallible};
const MAC:[u8;6]=[2,0x52,0x35,0x53,0,1];
fn word(offset:usize)->&'static AtomicU32 {unsafe {&*((0x40004000+offset) as *const AtomicU32)}}
#[protection_domain]
fn init()->impl Handler {
 while unsafe {&*(0x40001008 as *const AtomicU32)}.load(Ordering::Acquire)!=1 {core::hint::spin_loop();}
 let mut service=probe::Probe::new(identity::BUILD_ID);let mut input=[0u8;1514];let mut output=[0u8;1514];
 let mut epoch=0;let mut peer=[0u8;6];let mut fault=0;let mut last=u64::MAX;
 loop {
  let now=shared::millis();
  if let Some(status)=shared::read::<13>(0x40002000) {
   if status[11]!=epoch || status[2]!=2 {service.link_down();peer=[0;6];epoch=status[11];}
   if fault==0 && status[2]==2 {
    for _ in 0..8 {
     let length=match queue::pop(queue::BASE,epoch,&mut input) {Ok(n)=>n,Err(())=>{fault=1;0}};
     if length==0 {break;}
     if length<14+probe::HEADER || input[..6]!=MAC || input[6]&1!=0 || input[6..12]==[0;6] || input[12..14]!=[0x88,0xb5] {continue;}
     let kind=input[19];if kind==3 && input[6..12]!=peer {continue;}
     if let Ok(size)=service.receive(&input[14..length],now,&mut output[14..]) {
      if size!=0 {
       peer.copy_from_slice(&input[6..12]);output[..6].copy_from_slice(&peer);output[6..12].copy_from_slice(&MAC);output[12..14].copy_from_slice(&[0x88,0xb5]);
       if queue::push(queue::BASE+queue::DIRECTION,epoch,&output[..14+size]).is_err() {fault=1;}
      }
     }
    }
   }
  }
  if now!=last {
   last=now;word(0).fetch_add(1,Ordering::AcqRel);
   for (offset,value) in [(4,1),(8,now as u32),(12,service.proof.unwrap_or(0) as u32),(16,u32::from(service.proof.is_some())),(20,epoch),(24,service.rx as u32),(28,fault)] {word(offset).store(value,Ordering::Relaxed);}
   word(0).fetch_add(1,Ordering::Release);
  }
  core::hint::spin_loop();
 }
 #[allow(unreachable_code)] Idle
}
struct Idle;impl Handler for Idle {type Error=Infallible;}
