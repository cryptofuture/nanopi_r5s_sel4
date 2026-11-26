pub const POOL:usize=0x60000000;
pub const SIZE:usize=if matches!(option_env!("R5S_GMAC_RING"), Some(v) if matches!(v.as_bytes(),b"256")) {0x100000} else {0x40000};
pub const PAGE:usize=4096;pub const LINE:usize=64;
#[derive(Clone,Copy,Debug,PartialEq)]
pub struct Failure {pub syscall:u32,pub op:u32,pub start:usize,pub end:usize,pub observed:u32,pub expected:u32,pub readback:bool}
pub trait Backend {fn call(&mut self,op:u32,start:usize,end:usize)->u32;fn write(&mut self,address:usize,value:u32);fn read(&mut self,address:usize)->u32;fn progress(&mut self);}
pub fn maintain(b:&mut impl Backend,op:u32,address:usize,length:usize)->Result<(),Failure> {
 if op>2 || length==0 || address<POOL || address%LINE!=0 || length%LINE!=0 || address-POOL>=SIZE || length>SIZE-(address-POOL) {return Err(Failure {syscall:100,op,start:address,end:address.saturating_add(length),observed:0,expected:0,readback:false});}
 let mut start=address;let mut left=length;
 while left!=0 {let part=left.min(PAGE-start%PAGE);let error=b.call(op,start,start+part);if error!=0 {return Err(Failure {syscall:error,op,start,end:start+part,observed:0,expected:0,readback:false});}start+=part;left-=part;b.progress();}Ok(())
}
pub fn page(b:&mut impl Backend,index:usize)->Result<(),Failure> {
 assert!(index<SIZE/PAGE);let start=POOL+index*PAGE;
 for pass in 0..2 {
  let pattern=if pass==0 {0xa55a1234}else{0x5aa5edcb};
  for offset in (0..PAGE).step_by(4) {b.write(start+offset,((index*PAGE+offset) as u32)^pattern);}
  if pass==0 {maintain(b,0,start,PAGE)?;maintain(b,1,start,PAGE)?;}else{maintain(b,2,start,PAGE)?;}
  for offset in (0..PAGE).step_by(4) {let expected=((index*PAGE+offset) as u32)^pattern;let observed=b.read(start+offset);if observed!=expected {return Err(Failure {syscall:0,op:if pass==0 {1}else{2},start:start+offset,end:start+offset+4,observed,expected,readback:true});}}
  b.progress();
 }Ok(())
}
