#![no_std]
pub const HEADER:usize=68;
pub const MAX_DATA:usize=1400;
#[derive(Debug,PartialEq)]
pub enum Error {Length,Header,Identity,Stale}
pub struct Message<'a> {pub kind:u8,pub nonce:[u8;16],pub seq:u64,pub build:[u8;16],pub rx:u64,pub tx:u64,pub data:&'a [u8]}
pub fn parse(bytes:&[u8])->Result<Message<'_>,Error> {
    if bytes.len()<HEADER {return Err(Error::Length);}
    let length=u16::from_be_bytes(bytes[6..8].try_into().unwrap()) as usize;
    if length>MAX_DATA || bytes.len()!=HEADER+length {return Err(Error::Length);}
    if &bytes[..4]!=b"R5SN" || bytes[4]!=1 || !(1..=3).contains(&bytes[5]) || bytes[48..52]!=[0;4] {return Err(Error::Header);}
    Ok(Message {kind:bytes[5],nonce:bytes[8..24].try_into().unwrap(),seq:u64::from_be_bytes(bytes[24..32].try_into().unwrap()),build:bytes[32..48].try_into().unwrap(),rx:u64::from_be_bytes(bytes[52..60].try_into().unwrap()),tx:u64::from_be_bytes(bytes[60..68].try_into().unwrap()),data:&bytes[HEADER..]})
}
pub struct Probe {build:[u8;16],pending:[u8;HEADER+MAX_DATA],length:usize,sent:u64,session:Option<([u8;16],u64)>,pub proof:Option<u64>,pub rx:u64,pub tx:u64}
impl Probe {
    pub const fn new(build:[u8;16])->Self {Self {build,pending:[0;HEADER+MAX_DATA],length:0,sent:0,session:None,proof:None,rx:0,tx:0}}
    pub fn link_down(&mut self) {self.length=0;self.proof=None;}
    pub fn receive(&mut self,input:&[u8],now:u64,out:&mut [u8])->Result<usize,Error> {
        let m=parse(input)?;
        if m.build!=self.build {return Err(Error::Identity);}
        if m.kind==3 {
            if self.length==0 || now.saturating_sub(self.sent)>=2000 || input.len()!=self.length || input[..5]!=self.pending[..5] || input[6..]!=self.pending[6..self.length] {return Err(Error::Stale);}
            self.proof=Some(now);self.length=0;return Ok(0);
        }
        if m.kind!=1 || out.len()<input.len() {return Err(Error::Header);}
        if self.session.is_some_and(|(nonce,seq)| nonce==m.nonce && m.seq<=seq) {return Err(Error::Stale);}
        self.session=Some((m.nonce,m.seq));
        self.rx=self.rx.saturating_add(1);self.tx=self.tx.saturating_add(1);
        out[..input.len()].copy_from_slice(input);out[5]=2;
        out[52..60].copy_from_slice(&self.rx.to_be_bytes());out[60..68].copy_from_slice(&self.tx.to_be_bytes());
        self.pending[..input.len()].copy_from_slice(&out[..input.len()]);self.length=input.len();self.sent=now;
        Ok(input.len())
    }
}
