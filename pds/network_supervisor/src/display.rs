#[derive(Clone, Copy, PartialEq, Debug)]
pub enum State { Wait, Ready, IoVerified, Fault }
const WAIT: &[(bool,u64)] = &[(true,2000),(false,2000)];
const READY: &[(bool,u64)] = &[(true,200),(false,3800)];
const IO_VERIFIED: &[(bool,u64)] = &[(true,2500),(false,500),(true,200),(false,2800)];
const FAULT: &[(bool,u64)] = &[(true,125),(false,125),(true,125),(false,125),(true,125),(false,125),(true,125),(false,125),(true,125),(false,125),(true,125),(false,125),(true,125),(false,125),(true,125),(false,4125)];
pub const HEALTH_MS:u64=100;
pub const LEASE_MS:u64=5000;
pub const POLL_MS:u64=10;
pub fn select(now:u64, health:u64, link:bool, proof:Option<u64>, fault:bool)->State {
    if fault || now.saturating_sub(health)>HEALTH_MS {State::Fault}
    else if !link {State::Wait}
    else if proof.is_some_and(|t| t<=now && now-t<LEASE_MS) {State::IoVerified}
    else {State::Ready}
}
pub struct Display {pub state:State, start:u64}
impl Display {
    pub fn new(now:u64)->Self {Self{state:State::Wait,start:now}}
    pub fn tick(&mut self, state:State, now:u64)->bool {
        if state!=self.state {self.state=state;self.start=now;}
        let elapsed=now.saturating_sub(self.start);
        if elapsed<500 {return false;}
        let pattern=match state {State::Wait=>WAIT,State::Ready=>READY,State::IoVerified=>IO_VERIFIED,State::Fault=>FAULT};
        let mut position=(elapsed-500)%pattern.iter().map(|p|p.1).sum::<u64>();
        for &(on,duration) in pattern {if position<duration {return on;} position-=duration;}
        false
    }
}
