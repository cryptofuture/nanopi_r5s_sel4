pub struct Health {start:u32,sequence:u32,changed:u32,cached:Option<[u32;24]>,phase:u32,phase_start:u32,boot:u32,ready:bool}
pub fn age(now:u32,then:u32)->Result<u32,u32> {let age=now.wrapping_sub(then);if age>=0x80000000 {Err(8)}else{Ok(age)}}
impl Health {
 pub fn new(now:u32)->Self {Self {start:now,sequence:0,changed:now,cached:None,phase:0,phase_start:0,boot:0,ready:false}}
 pub fn check(&mut self,snapshot:Option<[u32;24]>,now:u32)->Result<bool,u32> {
  if let Some(v)=snapshot {
   if v[0]==0 || v[0]&1!=0 || v[1]!=1 || v[23]!=2 || v[20]<1 || v[20]>3 || v[2]>=3 || v[3]!=0 {return Err(4);}
   if self.phase!=0 && (v[20]<self.phase || v[22]!=self.boot || (v[20]==self.phase && v[21]!=self.phase_start)) {return Err(4);}
   if v[0]!=self.sequence {self.sequence=v[0];self.changed=now;}
   self.phase=v[20];self.phase_start=v[21];self.boot=v[22];self.cached=Some(v);
  }
  let Some(v)=self.cached else {return if age(now,self.start)?>100 {Err(6)}else{Ok(false)}};
  if v[20]<3 {
   if self.ready || age(now,v[22])?>4500 || age(now,v[21])?>if v[20]==1 {3000}else{1500} {return Err(9);}
   return Ok(false);
  }
  self.ready=true;
  if age(now,self.changed)?>100 || age(now,v[4])?>100 {return Err(6);}
  if v[2]==0 {return Ok(false);}
  if age(now,v[5])?>500 {return Err(5);}
  if v[2]!=2 {return Ok(false);}
  if ![10,100,1000].contains(&v[6]) || v[7]>1 || (v[6]==1000 && v[7]!=1) {return Err(4);}
  Ok(v[12]==1 && v[14]==1)
 }
}

pub struct AppFreshness {
    last_publication: u32,
}

impl AppFreshness {
    pub fn new(start: u32) -> Self {
        Self { last_publication: start }
    }

    pub fn check(&mut self, sample: Option<&[u32; 24]>, now: u32) -> Result<(), u32> {
        if let Some(status) = sample {
            if status[1] != 1 || status[7] != 0 {
                return Err(7);
            }
            self.last_publication = status[2];
        }
        if age(now, self.last_publication)? > 100 {
            return Err(7);
        }
        Ok(())
    }
}
