pub struct Health {last_sequence:u32,last_change:u64,cached:Option<[u32;11]>}
impl Health {
 pub fn new(now:u64)->Self {Self {last_sequence:0,last_change:now,cached:None}}
 pub fn check(&mut self,snapshot:Option<[u32;11]>,now:u64)->Result<bool,u32> {
  if let Some(value)=snapshot {
   if value[0]==0 || value[0]&1!=0 {return Err(4);}
   if value[0]!=self.last_sequence {self.last_sequence=value[0];self.last_change=now;}
   self.cached=Some(value);
  }
  if now.saturating_sub(self.last_change)>100 {return Err(6);}
  let Some(value)=self.cached else {return Ok(false)};
  if value[1]!=1 || value[2]>=3 || value[3]!=0 {return Err(4);}
  if (now as u32).wrapping_sub(value[4])>100 {return Err(6);}
  if value[2]!=0 && (now as u32).wrapping_sub(value[5])>500 {return Err(5);}
  if value[2]!=2 {return Ok(false);}
  if ![10,100,1000].contains(&value[6]) || value[7]>1 || (value[6]==1000 && value[7]!=1) {return Err(4);}
  Ok(true)
 }
}
