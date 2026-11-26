use core::sync::atomic::{AtomicU32,Ordering,fence};
use crate::{contract,registry};
pub const WORDS:usize=24;
pub fn read(w:&[AtomicU32;WORDS])->Option<[u32;WORDS]> {
 for _ in 0..2 {let seq=w[0].load(Ordering::Acquire);if seq==0 || seq&1!=0 {continue;}
  let mut v=[0;WORDS];for i in 1..WORDS {v[i]=w[i].load(Ordering::Relaxed);}fence(Ordering::Acquire);
  if w[0].load(Ordering::Acquire)==seq {v[0]=seq;return Some(v);}
 }None
}
pub fn publish(w:&[AtomicU32;WORDS],v:&[u32;WORDS]) {
 w[0].fetch_add(1,Ordering::AcqRel);for i in 1..WORDS {w[i].store(v[i],Ordering::Relaxed);}w[0].fetch_add(1,Ordering::Release);
}
pub fn producer(v:&[u32;contract::N],id:[u32;4])->Option<u8> {
 if v[1]!=contract::ABI || v[2]!=contract::PROFILE || v[23..27]!=id || v[14]==0 {return None;}
 let (reason,op,result)=(v[14],v[16],v[15]);
 Some(if [1,7,8,9,10].contains(&reason) {registry::lookup(reason,99,99)}else{registry::lookup(reason,op,result)})
}
pub fn select(snapshot:Option<[u32;contract::N]>,monitor:Option<[u32;WORDS]>,effect:u32,id:[u32;4])->u8 {
 if let Some(v)=snapshot {if let Some(code)=producer(&v,id) {if v[14]!=9 {return code;}}}
 if let Some(v)=monitor {
  if v[1]!=1 || v[17..21]!=id || v[21]!=contract::PROFILE {return registry::lookup(202,0,0);}
  let code=registry::lookup(200,v[2],v[3]);return if code==0 {registry::lookup(201,0,0)}else{code};
 }
 if effect==121 {return registry::lookup(202,0,0);}
 let code=registry::lookup(100,0,effect);if code!=0 {code}else{registry::lookup(203,0,0)}
}
