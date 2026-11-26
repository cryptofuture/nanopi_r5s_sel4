#![no_std]
#![no_main]
mod poll_deadline;mod milestones;mod fault_code;mod fault_display;mod display;mod health;mod shared;mod policy;
use core::sync::atomic::{AtomicU32,Ordering};
use sel4_microkit::{protection_domain,Handler,Infallible};
fn word(offset:usize)->&'static AtomicU32 {unsafe {&*((0x40001000+offset) as *const AtomicU32)}}
fn led(on:bool) {unsafe {core::ptr::write_volatile(0x40000004 as *mut u32,0x04000000|if on {0x400}else{0});core::arch::asm!("dsb sy",options(nostack));}}
#[cfg(feature="supervised")]
include!("lan_selection.rs");

#[cfg(feature="supervised")]
fn lan_led_init() -> bool {
    const CLK: *mut u32 = 0x4000837c as *mut u32;
    const MUX: *mut u32 = 0x4000705c as *mut u32;
    const DATA: *mut u32 = 0x40006004 as *mut u32;
    const DIR: *mut u32 = 0x4000600c as *mut u32;
    unsafe {
        CLK.write_volatile(0x00400000);
        if CLK.read_volatile() & 0x40 != 0 { return false; }
        DATA.write_volatile(0xc0000000);
        DIR.write_volatile(0xc000c000);
        MUX.write_volatile(0xff000000);
        core::arch::asm!("dsb sy", options(nostack));
        MUX.read_volatile() & 0xff00 == 0 && DIR.read_volatile() & 0xc000 == 0xc000
    }
}

fn counter()->u64 {
 let value:u64;
 unsafe {core::arch::asm!("isb","mrs {},cntvct_el0",out(reg)value,options(nostack));}
 value
}
#[protection_domain]
fn init()->impl Handler {
 while word(0).load(Ordering::Acquire)!=1 {core::hint::spin_loop();}
 unsafe {core::ptr::write_volatile(0x4000000c as *mut u32,0x04000400);}
 #[cfg(feature="supervised")]
 let mut lan_leds = (LAN_FRONT_LEDS && lan_led_init()).then_some((0u64, [0u32; 2], 0u32));
 let mut last_poll=counter();let start=last_poll/poll_deadline::TICKS_PER_MS;let mut health=health::Health::new(start as u32);
 let mut after_policy=last_poll;let mut after_gpio=last_poll;let mut gap_detail=0;let mut gap_start=0;
 let mut gap_count=0u32;
 let mut router_running=false;
 let mut report_time=None;let mut report_index=0;let mut report=fault_display::FaultDisplay::new();
 #[cfg(any(feature="lan-gated",feature="lan1-only"))] let mut banked=false;
 #[cfg(feature="lan1-only")] let (mut lan_turn, mut lan_index, mut lan_phase, mut lan_seen) = (0usize, [0u32;2], [0u8;2], [0u32;2]);
 #[cfg(feature="supervised")] let (mut sent, mut gap_lan, mut extension)=(0u8,0u8,[0u8;5]);
 let mut optical= fault_display::FaultDisplay::new();
 let mut app_health=health::AppFreshness::new(start as u32);
 let mut last_beat=start;let mut last_led=None;let mut beat=word(4).load(Ordering::Acquire);let mut seen=false;
 let mut frequency:u64;unsafe {core::arch::asm!("mrs {},cntfrq_el0",out(reg)frequency,options(nostack));}
 let mut reason=if frequency==poll_deadline::COUNTER_HZ {0}else{1};
 word(12).store(1,Ordering::Release);word(8).store(1,Ordering::Release);
 let mut next_gmac_poll=0;
 loop {
  #[cfg(feature="supervised")] let iteration=counter();
  let driver=shared::read::<24>(0x40002000);
  #[cfg(feature="supervised")] let driver_done=counter();
  let app=shared::read::<25>(0x40004000);
  #[cfg(feature="supervised")] let app_done=counter();
  let count=word(4).load(Ordering::Acquire);
  #[cfg(feature="supervised")] let health_done=counter();
  let ticks=counter();let time=ticks/poll_deadline::TICKS_PER_MS;
  if option_env!("R5S_GMAC_HYBRID_IRQ")==Some("1") && time>=next_gmac_poll {
   next_gmac_poll=time+1;
   sel4_microkit::Channel::new(2).notify();
  }
  let previous=last_poll;
  if let Some(gap)=poll_deadline::missed(&mut last_poll,ticks) {
   if reason==0 && gap_count==0 {
    word(28).store(gap.min(u64::from(u32::MAX)) as u32,Ordering::Relaxed);
    word(32).store(driver.map_or(0,|d|d[20]),Ordering::Relaxed);
    #[cfg(not(feature="supervised"))]
    {gap_detail=poll_deadline::detail(previous,after_policy,after_gpio,ticks,count!=beat);}
    #[cfg(feature="supervised")] {
     let health_at_detection=word(4).load(Ordering::Acquire);
     let stamps=[previous,after_policy,after_gpio,iteration,driver_done,app_done,health_done,ticks,counter()];
     let (advanced,bracket_changed)=poll_deadline::health_evidence(beat,count,health_at_detection);
     gap_detail=poll_deadline::detail(previous,after_policy,after_gpio,ticks,advanced);
     let phase=|port:usize| unsafe {&*((0x41000000+port*0x100000+28) as *const AtomicU32)}.load(Ordering::Acquire);
     let first=phase(0);
     let second=phase(1);
     word(36).store(first | (second<<8),Ordering::Relaxed);
     gap_lan=poll_deadline::lan_context(first,second);
     extension=poll_deadline::extension(poll_deadline::longest_span(&stamps),bracket_changed,first as u8,second as u8);
     for (index,stamp) in stamps.into_iter().enumerate() {
      word(40+index*8).store(stamp as u32,Ordering::Relaxed);
      word(44+index*8).store((stamp>>32) as u32,Ordering::Relaxed);
     }
     word(112).store(beat,Ordering::Relaxed);
     word(116).store(count,Ordering::Relaxed);
     word(120).store(health_at_detection,Ordering::Relaxed);
     word(128).store(u32::from(gap_detail),Ordering::Relaxed);
    }
    gap_start=time;
   }
   gap_count=gap_count.saturating_add(1);
   word(124).store(gap_count,Ordering::Release);
   reason=poll_deadline::fault_after_gap(reason,router_running);
  }
  let mut link=match health.check(driver,time as u32) {
   Ok(link)=>{
    router_running |= cfg!(feature="router-runtime") && driver.is_some_and(|d|d[20]==3);
    link
   },
   Err(e)=>{if reason==0 {reason=e;}false}
  };
  if count!=beat {beat=count;last_beat=time;seen=true;}
  #[cfg(feature="supervised")]
  if let Some((next, last_packets, last_on)) = lan_leds.as_mut() {
   if time >= *next {
    *next = time.saturating_add(100);
    let mut on = 0u32;
    for (port, last) in last_packets.iter_mut().enumerate() {
     let status = unsafe { &*((0x41000000 + port * 0x100000) as *const [AtomicU32; 44]) };
     let bit = 1 << (14 + port);
     let packets = status[33].load(Ordering::Relaxed).wrapping_add(status[39].load(Ordering::Relaxed));
     if status[32].load(Ordering::Acquire) == 1 && (*last == packets || *last_on & bit == 0) { on |= bit; }
     *last = packets;
    }
    if on != *last_on {
     unsafe { (0x40006004 as *mut u32).write_volatile(0xc0000000 | on); }
     *last_on = on;
    }
   }
  }
  if time.saturating_sub(last_beat)>100 && reason==0 {reason=3;}
  #[cfg(feature="supervised")]
  if reason==0 && seen {
   let due=poll_deadline::startup_due(LAN_SELECTION,sent,time.saturating_sub(start));
   for port in 0..2 {
    if due & (1<<port)!=0 {
     sel4_microkit::Channel::new(port).notify();
     sent |= 1<<port;
    }
   }
  }
  let mut proof=None;
  if let Err(code)=app_health.check(app.as_ref().map(|a|a[..24].try_into().unwrap()),time as u32) {if reason==0 {reason=code;}}
  match app {
   Some(a)=>{
    link &= driver.is_some_and(|d|d[11]==a[5]);
    if policy::WAN {link &= a[9]==1 && driver.is_some_and(|d|d[19]==a[8]);}
    else if link && a[4]==1 {if let Ok(age)=health::age(time as u32,a[3]) {if age<5000 {proof=Some(time.saturating_sub(age as u64));}}}
   },None=>{link=false;}
  }
  let state=if !seen && reason==0 {display::State::Wait}else{display::select(time,last_beat,link,proof,reason!=0)};
  word(16).store(match state {display::State::Wait=>0,display::State::Ready=>1,display::State::IoVerified=>2,display::State::Fault=>3},Ordering::Release);
  word(20).store(reason,Ordering::Release);word(24).store(time as u32,Ordering::Release);
  let on=if state==display::State::Fault {
   if reason==2 {
    let cycle=fault_display::TRANSITION_MS+fault_display::FRAME_MS;
    let elapsed=time.saturating_sub(gap_start);
    #[cfg(not(feature="supervised"))]
    let id=if (elapsed/cycle)%2==0 {2}else{100+gap_detail};
    #[cfg(feature="supervised")]
    let slot=(elapsed/cycle)%9;
    #[cfg(feature="supervised")]
    let id=match slot {0=>2,1=>100+gap_detail,2=>gap_lan,3=>100,n=>extension[(n-4) as usize]};
    let mut frame=fault_display::FaultDisplay::new();
    frame.latch(id,time-elapsed%cycle);
    #[cfg(feature="supervised")]
    {frame.tick_with_flutter(time,if slot>=3 {3000}else{2000})}
    #[cfg(not(feature="supervised"))]
    {frame.tick(time)}
   }else{optical.latch(fault_code::select(reason,driver),time);optical.tick(time)}
  }else{
   if report_time.is_none_or(|start| time.saturating_sub(start)>=fault_display::FRAME_MS+fault_display::TRANSITION_MS) {
    let stage=driver.map_or(0,|d|if d[20]!=3 {0}else if d[2]!=2 {1}else if d[14]!=1 {2}else{3});
    let flags=driver.map_or(0,|d|d[18]&31);
    let progress=app.filter(|a|driver.is_some_and(|d|a[5]==d[11] && a[8]==d[19])).map_or(0,|a|a[24]);
    let waiting=u32::from(progress&1!=0 && flags&16==0);
    #[cfg(feature="lan1-only")] let old_index=report_index;
    #[cfg(not(feature="lan-gated"))]
    let id=milestones::next(stage,progress|(flags<<8)|(waiting<<13),&mut report_index);
    #[cfg(feature="lan-gated")]
    let id={
     let sample=shared::read::<4>(0x40005000);
     let healthy=sample.is_some_and(|s|s[1]==1 && ((counter()/poll_deadline::TICKS_PER_MS) as u32).wrapping_sub(s[2])<=100);
     let configured=unsafe {&*(0x40002078 as *const AtomicU32)}.load(Ordering::Acquire);
     let selected=milestones::gated(stage,progress|(flags<<8)|(waiting<<13),&mut report_index,healthy,configured);
     banked=selected.0;selected.1
    };
    #[cfg(feature="lan1-only")]
    let id = {
     let port=usize::from(lan_turn==2);
     let status = |i:usize| unsafe { &*((0x41000000 + port*0x100000 + i*4) as *const AtomicU32) }.load(Ordering::Acquire);
     let last = status(1).max(if status(0)>=65 {status(0)} else {0});
     let bits = status(2) & 0x7fff_ffff;
     banked = lan_turn!=1 && (bits!=0 || last!=0);
     if banked {report_index=old_index;}
     lan_turn = (lan_turn+1) % if cfg!(feature="lan-dhcp") {3} else {2};
     if !banked { id } else { (if port==1 {50} else {0}) + {
      let prior=status(3);
      let code=|raw|match raw {65..=75=>raw as u8,160..=166|168..=173=>(raw-128) as u8,167=>13,_=>0};
      let terminal=code(last);
      let phase=lan_phase[port];
      lan_phase[port]=(lan_phase[port]+1)%3;
      if phase==0 && terminal!=0 {terminal}
      else if phase==1 && code(prior)!=0 {code(prior)}
      else if bits!=lan_seen[port] {lan_seen[port]=bits;(32-bits.leading_zeros()) as u8}
      else if bits!=0 {
       let mut n=lan_index[port];
       for _ in 0..31 {if bits & (1<<n)!=0 {break;} n=(n+1)%31;}
       lan_index[port]=(n+1)%31;
       (n+1) as u8
      } else {terminal}
     }}
    };
    report=fault_display::FaultDisplay::new();report.latch(id,time);report_time=Some(time);
   }
   let on=report.tick(time);
   #[cfg(any(feature="lan-gated",feature="lan1-only"))]
   let on=if banked && time-report_time.unwrap()<6500 {(500..5500).contains(&(time-report_time.unwrap()))} else {on};
   on
  };
  after_policy=counter();
  if last_led!=Some(on) {led(on);last_led=Some(on);}
  after_gpio=counter();
  core::hint::spin_loop();
 }
 #[allow(unreachable_code)] Idle
}
struct Idle;impl Handler for Idle {type Error=Infallible;}
