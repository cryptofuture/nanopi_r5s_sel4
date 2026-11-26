include!("fault_timing.rs");
pub fn word(id:u8)->Option<u8> {
 if id>127 {return None;}
 Some((id<<1)|((id.count_ones()&1) as u8))
}
pub fn decode(bits:u8)->Option<u8> {
 if bits.count_ones()&1!=0 {None}else{Some(bits>>1)}
}
pub struct FaultDisplay {latched:Option<(u8,u64)>}
impl FaultDisplay {
 pub const fn new()->Self {Self {latched:None}}
 pub fn latch(&mut self,id:u8,now:u64) {
  if self.latched.is_none() {self.latched=Some((if id<=127 {id}else{0},now));}
 }
 pub fn tick(&self,now:u64)->bool {self.tick_with_flutter(now,2000)}
 pub fn tick_with_flutter(&self,now:u64,flutter_ms:u64)->bool {
  let Some((id,start))=self.latched else{return false};
  let elapsed=now.saturating_sub(start);
  if elapsed<TRANSITION_MS {return false;}
  let t=(elapsed-TRANSITION_MS)%FRAME_MS;
  if t<FAULT_MS {return t<flutter_ms && t%250<125;}
  let first_end=FAULT_MS+4*SLOT_MS;
  let second_start=first_end+GROUP_SEPARATOR_MS;
  let second_end=second_start+4*SLOT_MS;
  let (index,slot)=if t<first_end {((t-FAULT_MS)/SLOT_MS,(t-FAULT_MS)%SLOT_MS)}
   else if t<second_start {return false}
   else if t<second_end {(4+(t-second_start)/SLOT_MS,(t-second_start)%SLOT_MS)}
   else{return false};
  let bits=word(id).unwrap_or(0);
  slot<if bits&(1<<(7-index))!=0 {LONG_ON_MS}else{SHORT_ON_MS}
 }
}
