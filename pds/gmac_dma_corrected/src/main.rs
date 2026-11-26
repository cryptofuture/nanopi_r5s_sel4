#![no_std]
#![no_main]
#[cfg(feature="lan-gated")] mod wan_led;
mod cache;mod exercise;
#[cfg(feature="rawio")] mod queue;
#[cfg(feature="rawio")] mod rk3568_clock;
#[cfg(feature="zc")] mod zc;
#[cfg(any(feature="tso",feature="rx-gro"))] mod tso;
#[cfg(feature="tso-test")] mod tso_testgen;
use core::sync::atomic::{AtomicU32,Ordering};
use sel4_microkit::{protection_domain,Handler,Infallible};
static PHASE:AtomicU32=AtomicU32::new(1);
static PHASE_MS:AtomicU32=AtomicU32::new(0);
static BOOT_MS:AtomicU32=AtomicU32::new(0);
static EPOCH:AtomicU32=AtomicU32::new(1);
static CFG_ACK:AtomicU32=AtomicU32::new(0);
static DROPS:AtomicU32=AtomicU32::new(0);
static ARM:AtomicU32=AtomicU32::new(0);
static STATE:AtomicU32=AtomicU32::new(0);
static FAULT:AtomicU32=AtomicU32::new(0);
static SPEED:AtomicU32=AtomicU32::new(0);
static DUPLEX:AtomicU32=AtomicU32::new(0);
static READ_MS:AtomicU32=AtomicU32::new(0);
#[cfg(feature="rawio")] static CLOCK_SAMPLE_MS:AtomicU32=AtomicU32::new(u32::MAX);
static PUBLISHED_MS:AtomicU32=AtomicU32::new(u32::MAX);
unsafe extern "C" {
 #[cfg(feature="rawio")] fn dma_allowed(region:u32,offset:u32,value:u32)->i32;
 #[cfg(feature="rawio")] fn dma_start()->i32;
 #[cfg(feature="rawio")] fn dma_stop()->i32;
 #[cfg(feature="rawio")] fn dma_enabled()->i32;
 #[cfg(all(feature="rawio",feature="zc"))] fn dma_rx_channels()->u32;
 #[cfg(all(feature="rawio",feature="zc"))] fn dma_select_rx(channel:u32)->i32;
 #[cfg(all(feature="rawio",feature="zc"))] fn dma_tx_channels()->u32;
 #[cfg(all(feature="rawio",feature="zc"))] fn dma_select_tx(channel:u32)->i32;
 #[cfg(feature="rawio")] fn dma_begin_tx_batch();
 #[cfg(feature="rawio")] fn dma_end_tx_batch()->i32;
 #[cfg(feature="rawio")] fn dma_begin_rx_batch();
 #[cfg(feature="rawio")] fn dma_end_rx_batch()->i32;
 #[cfg(feature="rawio")] fn dma_irq_begin()->u32;
 #[cfg(feature="rawio")] fn dma_irq_end();
 #[cfg(feature="rawio")] static dma_rx_irq:u32;
 #[cfg(all(feature="rawio",feature="zc"))]
 fn dma_take_stopped_refs(slot:u32,rx:*mut u64,tx:*mut u64)->i32;
 #[cfg(feature="rawio")] fn dma_tx_available()->i32;
 #[cfg(all(feature="rawio",feature="zc"))] fn dma_tx_space()->u32;
 #[cfg(all(feature="rawio",feature="zc"))] fn dma_tx_pending()->u32;
 #[cfg(all(feature="rawio",feature="zc",feature="tso"))] fn dma_tx_capacity()->u32;
 #[cfg(feature="rawio")] static link_sample:[u32;6];
 #[cfg(feature="rawio")] fn dma_reclaim()->i32;
 #[cfg(feature="rawio")] fn dma_send(packet:*const u8,length:usize)->i32;
 #[cfg(all(feature="rawio",feature="zc"))] fn dma_send_nat_validated(packet:*const u8,length:usize)->i32;
 #[cfg(feature="rawio")] fn dma_receive(packet:*mut u8,capacity:usize)->i32;
 #[cfg(all(feature="rawio", feature="zc"))] fn dma_reclaim_check()->i32;
 #[cfg(all(feature="rawio", feature="zc"))]
 fn dma_receive_rearm(packet:*mut u8,capacity:usize,rearm_physical:u64)->i32;
 #[cfg(all(feature="rawio", feature="zc"))]
 fn dma_peek(slot:*mut u32,physical:*mut u64,length:*mut usize,rx_protocol:*mut u32,external:*mut i32)->i32;
 #[cfg(all(feature="rawio", feature="zc"))] fn dma_take_ref(slot:u32,physical:u64)->i32;
 #[cfg(all(feature="rawio", feature="zc"))] fn dma_send_ref(physical:u64,length:usize,checksum:u32)->i32;
 #[cfg(all(feature="rawio", feature="tso"))] fn dma_send_tso(request:*const tso::TsoRequest)->i32;
 #[cfg(all(feature="rawio", feature="zc"))] fn dma_reclaim_next(external:*mut u64,tso_id:*mut u32)->i32;
 #[cfg(all(feature="rawio", feature="tso"))] fn dma_tso_available()->i32;
 #[cfg(feature="rawio")] static dma_rx_packets:u64;
 #[cfg(feature="rawio")] static dma_evidence:u32;
 #[cfg(feature="rawio")] static dma_rx_checksum_protocol:u32;
 #[cfg(feature="rawio")] static dma_tx_ipv4_checksums:u32;
 #[cfg(feature="rawio")] static dma_tx_ipv4_checksum_active:u32;
 #[cfg(feature="rawio")] static dma_rx_retired:[u32;2];
 #[cfg(feature="rawio")] static dma_tx_retired:[u32;2];
 #[cfg(feature="rawio")] static dma_hw_features:[u32;4];
 #[cfg(feature="rawio")] static dma_hw_features_valid:u32;
 #[cfg(feature="rawio")] static dma_pbl:u32;
 #[cfg(feature="rawio")] static dma_pbl_requested:u32;
 #[cfg(feature="rawio")] static dma_axi_config:u32;
 #[cfg(feature="rawio")] static dma_tx_packets:u64;
 #[cfg(feature="rawio")] static dma_drops:u64;
 fn link_allowed(region:u32,offset:u32,value:u32)->i32;
 #[cfg(feature="lan-gated")] fn link_configure_jack_leds()->i32;
 fn link_init()->i32;
 fn link_stop();
 fn link_poll(speed:*mut u32,duplex:*mut u32)->i32;
 static link_mac_version:u32;static link_phy_id:u32;static link_clock_hz:u32;
}
fn word(offset:usize)-> &'static AtomicU32 {unsafe {&*((0x40002000+offset) as *const AtomicU32)}}
#[unsafe(no_mangle)] extern "C" fn hw_us()->u64 {let ticks:u64;unsafe {core::arch::asm!("isb","mrs {}, cntvct_el0",out(reg) ticks,options(nostack));}ticks/24}
#[unsafe(no_mangle)] extern "C" fn hw_progress() {
 let now=(hw_us()/1000) as u32;
 let phase=PHASE.load(Ordering::Relaxed);
 let phase_age=now.wrapping_sub(PHASE_MS.load(Ordering::Relaxed));
 let boot_age=now.wrapping_sub(BOOT_MS.load(Ordering::Relaxed));
 let effect=unsafe {&*(0x40001014 as *const AtomicU32)}.load(Ordering::Acquire);
 if FAULT.load(Ordering::Relaxed)==0 && ((phase<3 && (boot_age>4500 || phase_age>if phase==1 {3000}else{1500})) || (effect!=0 && (!cfg!(feature="wan-shutdown") || phase<3))) {
  #[cfg(feature="wan-shutdown")]
  shutdown(if effect!=0 {120}else{121},effect);
  #[cfg(not(feature="wan-shutdown"))]
  {
  FAULT.store(if effect!=0 {120}else{121},Ordering::Relaxed);STATE.store(3,Ordering::Relaxed);unsafe {link_stop();}
  loop {hw_progress();core::hint::spin_loop();}
  }
 }

 if PUBLISHED_MS.load(Ordering::Relaxed)==now {return;}
 PUBLISHED_MS.store(now,Ordering::Relaxed);
 word(0).fetch_add(1,Ordering::AcqRel);
 for (offset,value) in [(4,1),(8,STATE.load(Ordering::Relaxed)),(12,FAULT.load(Ordering::Relaxed)),(16,now),(20,READ_MS.load(Ordering::Relaxed)),(24,SPEED.load(Ordering::Relaxed)),(28,DUPLEX.load(Ordering::Relaxed)),(32,unsafe {link_mac_version}),(36,unsafe {link_phy_id}),(40,unsafe {link_clock_hz})] {word(offset).store(value,Ordering::Relaxed);}
 word(44).store(EPOCH.load(Ordering::Relaxed),Ordering::Relaxed);
 word(48).store(ARM.load(Ordering::Relaxed),Ordering::Relaxed);
 word(52).store(DROPS.load(Ordering::Relaxed),Ordering::Relaxed);
 #[cfg(feature="rawio")] word(56).store(unsafe {dma_enabled()} as u32,Ordering::Relaxed);
 #[cfg(feature="rawio")] {word(72).store(unsafe {dma_evidence},Ordering::Relaxed);word(60).store(unsafe {dma_rx_packets} as u32,Ordering::Relaxed);word(64).store(unsafe {dma_tx_packets} as u32,Ordering::Relaxed);word(68).store(unsafe {dma_drops} as u32,Ordering::Relaxed);}
 word(76).store(CFG_ACK.load(Ordering::Relaxed),Ordering::Relaxed);
 for (offset,value) in [(80,PHASE.load(Ordering::Relaxed)),(84,PHASE_MS.load(Ordering::Relaxed)),(88,BOOT_MS.load(Ordering::Relaxed)),(92,2)] {word(offset).store(value,Ordering::Relaxed);}
 #[cfg(feature="rawio")] for (index,value) in unsafe {link_sample}.iter().enumerate() {word(96+index*4).store(*value,Ordering::Relaxed);}
 #[cfg(feature="rawio")] {
  word(144).store(unsafe {dma_tx_ipv4_checksums},Ordering::Relaxed);
  word(148).store(unsafe {dma_tx_ipv4_checksum_active},Ordering::Relaxed);
  zc::publish_tx_checksum_offload(zc::META_VA, unsafe {dma_tx_ipv4_checksum_active} != 0);
  for (index,value) in unsafe {dma_hw_features}.into_iter().enumerate() {
   word(704+index*4).store(value,Ordering::Relaxed);
  }
  word(720).store(unsafe {dma_hw_features_valid},Ordering::Relaxed);
  word(724).store(unsafe {dma_pbl},Ordering::Relaxed);
  word(800).store(unsafe {dma_pbl_requested},Ordering::Relaxed);
  word(748).store(unsafe {dma_axi_config},Ordering::Relaxed);
  word(752).store(u32::from(unsafe {dma_rx_irq}!=0),Ordering::Relaxed);
  word(756).store(IRQ_EVENTS.load(Ordering::Relaxed),Ordering::Relaxed);
  #[cfg(feature="zc")]
  word(760).store(unsafe {dma_tx_channels()},Ordering::Relaxed);
  #[cfg(feature="zc")]
  word(764).store(unsafe {dma_rx_channels()},Ordering::Relaxed);
  for (index,value) in unsafe {dma_tx_retired}.into_iter().chain(unsafe {dma_rx_retired}).enumerate() {
   word(768+index*4).store(value,Ordering::Relaxed);
  }
  let last=CLOCK_SAMPLE_MS.load(Ordering::Relaxed);
  if last==u32::MAX || now.wrapping_sub(last)>=1000 {
   let clocks=rk3568_clock::snapshot(|offset|hw_read(3,offset));
   for (index,&value) in clocks[..5].iter().enumerate() {
    word(728+4*index).store(value,Ordering::Relaxed);
   }
   word(784).store(clocks[5],Ordering::Relaxed);
   CLOCK_SAMPLE_MS.store(now,Ordering::Relaxed);
  }
 }
 #[cfg(all(feature="rawio", feature="tso"))] {
  // Schema 7 WAN TSO block: nine counters, then active/capability/flags.
  for (index,value) in tso_counters().snapshot().iter().enumerate() {
   word(4*(152+index)).store(*value,Ordering::Relaxed);
  }
  let available=unsafe {dma_tso_available()}!=0;
  word(4*161).store(u32::from(available),Ordering::Relaxed);
  word(4*162).store((unsafe {dma_hw_features}[1] >> 18) & 1,Ordering::Relaxed);
  word(4*163).store(tso::status_flags(tso::TSO_IPV6, tso_counters().hw_completed.load(Ordering::Relaxed)),Ordering::Relaxed);
 }
 word(0).fetch_add(1,Ordering::Release);
}
fn address(region:u32,offset:u32)->usize {
 #[cfg(feature="lan-gated")] if region==6 {assert!(matches!(offset,4|12|0x78));return 0x50007000+offset as usize;}
 let base=[0x50000000usize,0x50002000,0x50003000,0x50004000,0x50005000,0x50006000];
 assert!(region<6 && offset%4==0 && offset<if region==0 {8192}else{4096});base[region as usize]+offset as usize
}
#[unsafe(no_mangle)] extern "C" fn hw_read(region:u32,offset:u32)->u32 {unsafe {core::ptr::read_volatile(address(region,offset) as *const u32)}}
#[unsafe(no_mangle)] extern "C" fn hw_write(region:u32,offset:u32,value:u32) {
 let mut allowed=unsafe {link_allowed(region,offset,value)}==1;
 #[cfg(feature="rawio")] {allowed|=ARM.load(Ordering::Relaxed)==1 && unsafe {dma_allowed(region,offset,value)}==1;}
 #[cfg(feature="lan-gated")] {allowed |= wan_led::allowed(region,offset,value);}
 if !allowed {FAULT.store(99,Ordering::Relaxed);STATE.store(3,Ordering::Relaxed);loop {hw_progress();}}
 unsafe {core::ptr::write_volatile(address(region,offset) as *mut u32,value);core::arch::asm!("dsb sy",options(nostack));}
}
#[protection_domain]
fn init()->impl Handler {
 while unsafe {&*(0x40001008 as *const AtomicU32)}.load(Ordering::Acquire)!=1 {core::hint::spin_loop();}
 let frequency:u64;unsafe {core::arch::asm!("mrs {}, cntfrq_el0",out(reg) frequency,options(nostack));}
 let boot=(hw_us()/1000) as u32;BOOT_MS.store(boot,Ordering::Relaxed);PHASE_MS.store(boot,Ordering::Relaxed);hw_progress();
 let result=if frequency!=24000000 {10} else if let Err(error)=cache::preflight() {100+error as i32} else {
  #[cfg(feature="zc")]
  zc::init(zc::META_VA);
  PHASE.store(2,Ordering::Relaxed);PHASE_MS.store((hw_us()/1000) as u32,Ordering::Relaxed);hw_progress();unsafe {link_init()}
 };
 PHASE.store(3,Ordering::Relaxed);PHASE_MS.store((hw_us()/1000) as u32,Ordering::Relaxed);
 if result==0 {ARM.store(1,Ordering::Release);}

 #[cfg(not(feature="quiet"))] sel4_microkit::debug_println!("GMAC init={} version={:#x} phy={:#x} clock={} DMA=off",result,unsafe {link_mac_version},unsafe {link_phy_id},unsafe {link_clock_hz});
 if result!=0 {unsafe {link_stop();}FAULT.store(result as u32,Ordering::Relaxed);STATE.store(3,Ordering::Relaxed);}
 #[cfg(feature="lan-gated")]
 let indicator_ok=result==0 && wan_led::setup(|r,o|hw_read(r,o),|r,o,v|hw_write(r,o,v));
 #[cfg(feature="lan-gated")] {
  let jack_ok=result==0 && unsafe {link_configure_jack_leds()}==1;
  word(120).store(u32::from(indicator_ok) | (u32::from(jack_ok)<<1),Ordering::Release);
 }
 let mut driver=Driver {
  next:0, previous:(u32::MAX,0,0),
  #[cfg(feature="zc")] next_worker_tx:0,
  #[cfg(feature="zc")] next_rx_channel:0,
  #[cfg(feature="rawio")] packet:[0;1514],
  #[cfg(feature="lan-gated")] indicator_ok,
  #[cfg(feature="lan-gated")] indicator:wan_led::Indicator::default(),
 };
 if !HYBRID_IRQ {
  loop {
   let mut extra_tx_available=true;
   if !driver.serve(&mut extra_tx_available) && option_env!("R5S_IDLE_YIELD")==Some("1") {sel4::r#yield();}
   core::hint::spin_loop();
  }
 }
 driver
}

const HYBRID_IRQ:bool=cfg!(feature="rawio") && match option_env!("R5S_GMAC_HYBRID_IRQ") {
 Some(value)=>matches!(value.as_bytes(),b"1"),
 None=>false,
};
#[cfg(feature="rawio")]
static IRQ_EVENTS:AtomicU32=AtomicU32::new(0);

struct Driver {
 next:u64,
 previous:(u32,u32,u32),
 #[cfg(feature="zc")] next_worker_tx:usize,
 #[cfg(feature="zc")] next_rx_channel:u32,
 #[cfg(feature="rawio")] packet:[u8;1514],
 #[cfg(feature="lan-gated")] indicator_ok:bool,
 #[cfg(feature="lan-gated")] indicator:wan_led::Indicator,
}

impl Handler for Driver {
 type Error=Infallible;
 fn notified(&mut self,channels:sel4_microkit::ChannelSet)->Result<(),Self::Error> {
  #[cfg(feature="rawio")]
  if HYBRID_IRQ {
   let irq=sel4_microkit::Channel::new(14);
   #[cfg(feature="zc")]
   if zc::WORKER_TX_EVENT_DRIVEN {
    zc::count(zc::META_VA, zc::WORKER_TX_WAKE_PASSES);
   }
   if unsafe {dma_irq_begin()} & (1 << 6) != 0 {
    IRQ_EVENTS.fetch_add(1,Ordering::Relaxed);
   }
   let mut extra_tx_available=true;
   for _ in 0..4 {
    if !self.serve(&mut extra_tx_available) {break;}
   }
   unsafe {dma_irq_end();}
   if channels.contains(irq) {irq.irq_ack().unwrap();}
  }
  let _=channels;
  Ok(())
 }
}

impl Driver {
 fn serve(&mut self,_extra_tx_available:&mut bool)->bool {
   hw_progress();let now=hw_us()/1000;
   #[cfg(feature="wan-shutdown")]
   {
    // DMA drain calls hw_progress, so stop requests are handled at the loop boundary.
    let request=unsafe {&*(0x40001014 as *const AtomicU32)}.load(Ordering::Acquire);
    if request!=0 && FAULT.load(Ordering::Relaxed)==0 { shutdown(120,request); }
   }
  #[cfg(feature="rawio")] {
   let request=unsafe {&*(0x40004020 as *const AtomicU32)}.load(Ordering::Acquire);
   if request!=CFG_ACK.load(Ordering::Relaxed) && STATE.load(Ordering::Relaxed)!=3 {
    let result=stop_dma();if result<0 {fatal((-result) as u32);}
    EPOCH.fetch_add(1,Ordering::Relaxed);CFG_ACK.store(request,Ordering::Release);self.next=0;
   }
  }
  if STATE.load(Ordering::Relaxed)!=3 && now>=self.next {
   #[cfg(all(feature="rawio",feature="zc"))] let was_running=unsafe {dma_enabled()}!=0;
   let mut speed=0;let mut duplex=0;let result=unsafe {link_poll(&mut speed,&mut duplex)};
   #[cfg(all(feature="rawio",feature="zc"))]
   if was_running && unsafe {dma_enabled()}==0 {release_stopped_refs();}
   if result<0 {
    #[cfg(feature="wan-shutdown")]
    fatal((-result) as u32);
    #[cfg(not(feature="wan-shutdown"))]
    {unsafe {link_stop();}FAULT.store((-result) as u32,Ordering::Relaxed);STATE.store(3,Ordering::Relaxed);}
   }
   else {SPEED.store(speed,Ordering::Relaxed);DUPLEX.store(duplex,Ordering::Relaxed);READ_MS.store((hw_us()/1000) as u32,Ordering::Relaxed);STATE.store(if result==1 {2}else{1},Ordering::Relaxed);}
   let current=(STATE.load(Ordering::Relaxed),speed,duplex);
   let changed=current!=self.previous;
   if changed {#[cfg(not(feature="quiet"))] sel4_microkit::debug_println!("GMAC state={} speed={} duplex={} fault={}",current.0,speed,duplex,FAULT.load(Ordering::Relaxed));self.previous=current;}
   #[cfg(feature="rawio")] if result>=0 {
    let result=if result==1 && unsafe {dma_enabled()}==0 {unsafe {dma_start()}} else if result==0 && unsafe {dma_enabled()}!=0 {stop_dma()} else {0};
    if result<0 {fatal((-result) as u32);}
    if changed {EPOCH.fetch_add(1,Ordering::Relaxed);}
   }
   self.next=hw_us()/1000+200;
  }
  #[cfg(feature="lan-gated")]
  if self.indicator_ok {
   let count=unsafe { dma_rx_packets.wrapping_add(dma_tx_packets) };
   if let Some(value)=self.indicator.update(now, STATE.load(Ordering::Relaxed)==2,count) { hw_write(6,4,value); }
  }
  #[cfg(feature="rawio")]
  let mut processed=false;
  #[cfg(feature="rawio")]
  let packet=&mut self.packet;
  #[cfg(feature="rawio")] if STATE.load(Ordering::Relaxed)!=3 && unsafe {dma_enabled()}!=0 {
   let epoch=EPOCH.load(Ordering::Relaxed);
   #[cfg(feature="tso-test")]
   generate_tso_test();
   #[cfg(feature="zc")]
   {
    let budget=queue::batch(option_env!("R5S_DATAPATH_BATCH"),8);
    let mut rx_remaining=budget;
    let mut tx_budget_hit=false;
    for _ in 0..2 {
     for channel in 0..unsafe {dma_tx_channels()} {
      if unsafe {dma_select_tx(channel)}!=0 {fatal(21);}
      let status=unsafe {dma_reclaim_check()};
      if status!=0 {fatal((-status) as u32);}
      let mut retired=0;
      for _ in 0..budget {
       let mut external=0u64;
       let mut tso_id=0u32;
       let result=unsafe {dma_reclaim_next(&mut external,&mut tso_id)};
       if result<0 {fatal((-result) as u32);}
       if result==0 {break;}
       retired+=1;
       processed=true;
       if external!=0 && let Some(r)=pool_ref(external,zc::TX_DMA) {
        let _=zc::complete(zc::META_VA,&r);
       }
       #[cfg(feature="tso")]
       if tso_id!=0 {complete_tso_request(tso_id);}
      }
      zc::note_wan_dma_completion(channel as usize,retired);
     }
     if unsafe {dma_select_tx(0)}!=0 {fatal(21);}
     let (tx_progress,hit)=service_tx(&mut self.next_worker_tx,epoch,now,budget/2);
     processed|=tx_progress;
     tx_budget_hit|=hit;
     let rx_before=rx_remaining;
     let rx_revision=zc::wan_fast_path().then(||zc::fast_wan_tag(zc::FAST_WAN_CONTROL,epoch)).flatten();
     let channels=unsafe {dma_rx_channels()};
     for _ in 0..channels {
      if rx_remaining==0 {break;}
      let channel=self.next_rx_channel % channels;
      self.next_rx_channel=(channel+1)%channels;
      if unsafe {dma_select_rx(channel)}!=0 {fatal(21);}
      unsafe {dma_begin_rx_batch();}
      #[cfg(feature="rx-gro")]
      let mut ingress=zc::IngressBatch::new(zc::META_VA);
      for _ in 0..rx_remaining.min(if tx_progress {budget/2} else {budget}) {
       #[cfg(feature="rx-gro")]
       {
        let drops=ingress.flush_expired(||hw_us());
        if drops!=0 {DROPS.fetch_add(drops,Ordering::Relaxed);}
       }
       let mut slot=0u32;let mut physical=0u64;let mut length=0usize;let mut protocol=0u32;let mut external=0i32;
       let peek=unsafe {dma_peek(&mut slot,&mut physical,&mut length,&mut protocol,&mut external)};
       if peek==-26 {
        rx_remaining-=1;
        processed=true;
        if unsafe {dma_take_ref(slot,if external != 0 {physical} else {0})} != 0 {
         #[cfg(feature="rx-gro")]
         drop(ingress);
         fatal(41);
        }
        DROPS.fetch_add(1,Ordering::Relaxed);
        continue;
       }
       if peek< -1 {
        #[cfg(feature="rx-gro")]
        drop(ingress);
        fatal((-peek) as u32);
       }
       if peek==1 {
        rx_remaining-=1;
        processed=true;
        let fresh=zc::alloc(zc::META_VA,zc::PARTITION_WAN);
        if let Some(r)=fresh {
        if zc::maintain_payload(&r, zc::PayloadCache::BeforeRx).is_err() {fatal(20);}
       }
       let rearm=fresh.and_then(|r|zc::buffer_physical(zc::POOL_PA,r.id())).unwrap_or(0);
        if unsafe {dma_take_ref(slot,rearm)}!=0 {
         #[cfg(feature="rx-gro")]
         drop(ingress);
         fatal(41);
        }
        if let Some(mut r)=pool_ref(physical,zc::RX_DMA) {
         r.len=length as u16;r.meta=protocol;r.epoch=epoch;
         let buffer=unsafe {core::slice::from_raw_parts((zc::POOL_VA+r.id() as usize*zc::BUFFER) as *const u8,length)};
         if let Some((destination,_,revision))=wan_rx_queue(buffer,rx_revision) {
          if let Some(revision)=revision { r.epoch=revision;r.flags=zc::FLAG_WAN; }
          let published=zc::publish(zc::META_VA,&r).is_ok();
          let queued=published && {
           #[cfg(feature="rx-gro")]
           if revision.is_some() {
            let first=tso::gro_starts(buffer);
            let drops=unsafe {ingress.offer_pool(destination,r,first,hw_us(),|a,b|tso::gro_pair(a,b).is_some())};
            if drops!=0 {DROPS.fetch_add(drops,Ordering::Relaxed);}
            true
           } else {
            let drops=ingress.flush();
            if drops!=0 {DROPS.fetch_add(drops,Ordering::Relaxed);}
            zc::RefQueue::push(destination,&r)==Ok(true)
           }
           #[cfg(not(feature="rx-gro"))]
           {zc::RefQueue::push(destination,&r)==Ok(true)}
          };
          if !queued {
           if published {let _=zc::reject(zc::META_VA,&r);}
           DROPS.fetch_add(1,Ordering::Relaxed);
          }
         } else {
          let _=zc::recycle(zc::META_VA,&r);
          DROPS.fetch_add(1,Ordering::Relaxed);
         }
        } else {DROPS.fetch_add(1,Ordering::Relaxed);}
        continue;
       }
       // Private completion (or nothing at the cursor): the copy path, which
       // migrates the slot onto the shared pool when a buffer is available.
       let migrate=peek==0 && length!=0;
       let fresh=if migrate {zc::alloc(zc::META_VA,zc::PARTITION_WAN)}else{None};
       if let Some(r)=fresh {
        if zc::maintain_payload(&r, zc::PayloadCache::BeforeRx).is_err() {fatal(20);}
       }
       let rearm=fresh.and_then(|r|zc::buffer_physical(zc::POOL_PA,r.id())).unwrap_or(0);
       let result=unsafe {dma_receive_rearm(packet.as_mut_ptr(),packet.len(),rearm)};
       if result< -1 {
        #[cfg(feature="rx-gro")]
        drop(ingress);
        fatal((-result) as u32);
       }
       if result==0 {
        if let Some(r)=fresh {let _=zc::recycle(zc::META_VA,&r);}
        break;
       }
       rx_remaining-=1;
       processed=true;
       if result>0 {
        #[cfg(feature="rx-gro")]
        {
         let drops=ingress.flush();
         if drops!=0 {DROPS.fetch_add(drops,Ordering::Relaxed);}
        }
        zc::count_rx_copy(zc::PARTITION_WAN, result as usize);
        let frame=&packet[..result as usize];
        let Some((refs,copies,revision))=wan_rx_queue(frame,rx_revision) else {
         DROPS.fetch_add(1,Ordering::Relaxed);
         continue;
        };
        let tag=revision.map_or(epoch,|rev|(rev<<queue::FORWARD_TAG_SHIFT)|1);
        match zc::RefQueue::push_copy(refs,||queue::push_meta(copies,tag,frame,unsafe {dma_rx_checksum_protocol})) {
         Err(())=>fatal(40),Ok(false)=>{DROPS.fetch_add(1,Ordering::Relaxed);},Ok(true)=>{}
        }
       }
      }
      #[cfg(feature="rx-gro")]
      {
       let drops=ingress.flush();
       if drops!=0 {DROPS.fetch_add(drops,Ordering::Relaxed);}
      }
      if unsafe {dma_end_rx_batch()} != 0 {fatal(20);}
     }
     if unsafe {dma_select_rx(0)}!=0 {fatal(21);}
     notify_rx_workers();
     if rx_remaining==rx_before && tx_progress {
      let (progress,hit)=service_tx(&mut self.next_worker_tx,epoch,now,budget/2);
      processed|=progress;
      tx_budget_hit|=hit;
     }
    }
    let extra_tx=queue::batch(option_env!("R5S_GMAC_TX_EXTRA_BATCH"),0);
    if extra_tx!=0 && *_extra_tx_available && tx_budget_hit && zc::wan_active_lanes()!=0 {
     *_extra_tx_available=false;
     processed|=service_tx(&mut self.next_worker_tx,epoch,now,extra_tx).0;
    }
   }
   #[cfg(not(feature="zc"))]
   for _ in 0..queue::batch(option_env!("R5S_DATAPATH_BATCH"),8) {
    let result=unsafe {dma_reclaim()};
    if result<0 {fatal((-result) as u32);}
    if result==0 {break;}
    processed=true;
   }
   #[cfg(not(feature="zc"))]
   {
    unsafe {dma_begin_rx_batch();}
    for _ in 0..queue::batch(option_env!("R5S_DATAPATH_BATCH"), 8) {
     let result=unsafe {dma_receive(packet.as_mut_ptr(),packet.len())};
     if result< -1 {fatal((-result) as u32);}if result==0 {break;}
     processed=true;
     if result>0 {match queue::push_meta(queue::BASE,epoch,&packet[..result as usize],unsafe {dma_rx_checksum_protocol}) {Err(())=>fatal(40),Ok(false)=>{DROPS.fetch_add(1,Ordering::Relaxed);},Ok(true)=>{}}}
    }
    if unsafe {dma_end_rx_batch()} != 0 {fatal(20);}
   }
   #[cfg(not(feature="zc"))]
   let batch_tx=option_env!("R5S_GMAC_TX_BATCH")==Some("1");
   #[cfg(not(feature="zc"))]
   if batch_tx {unsafe {dma_begin_tx_batch();}}
   #[cfg(not(feature="zc"))]
   for _ in 0..queue::batch(option_env!("R5S_DATAPATH_BATCH"), 8) {
    if unsafe {dma_tx_available()}==0 {break;}
    let result=unsafe {queue::consume_in_place(queue::BASE+queue::DIRECTION, |frame,tag| {
     (tag==epoch).then(|| dma_send(frame.as_ptr(),frame.len()))
    })};
    let result=match result {Ok(Some(Some(value)))=>value,Ok(_)=>break,Err(())=>fatal(40)};
    if result<0 {fatal((-result) as u32);}
    if result==0 {DROPS.fetch_add(1,Ordering::Relaxed);}
    processed=true;
   }
   #[cfg(not(feature="zc"))]
   if batch_tx && unsafe {dma_end_tx_batch()} != 0 {fatal(20);}
  }
  #[cfg(all(feature="rawio",feature="zc"))]
  if zc::worker_wan_egress() && unsafe {dma_enabled()}==0 {
   for lane in 0..zc::WORKER_TX_LANES {
    processed |= tx_worker_wan(lane,false,now,queue::batch(option_env!("R5S_DATAPATH_BATCH"),8),None,None) != 0;
    zc::publish_wan_tx_consumed(zc::worker_wan_refs(lane));
   }
  }
  #[cfg(not(feature="rawio"))]
  let processed=false;
  #[cfg(feature="zc")]
  zc::flush_counts(zc::META_VA);
  processed
 }
}
#[cfg(all(feature="rawio",feature="zc"))]
fn wan_rx_queue(frame:&[u8],revision:Option<u32>)->Option<(usize,usize,Option<u32>)> {
 if zc::wan_fast_path() && let Some(worker)=zc::fast_wan_owner(frame) {
  let revision=revision?;
  Some((zc::fast_wan_requests(worker),zc::fast_wan_frames(worker),Some(revision)))
 } else {
  Some((zc::link_queue(zc::LINKS_VA,zc::Q_GMAC_RX),queue::BASE,None))
 }
}

#[cfg(all(feature="rawio",feature="zc"))]
fn tx_batch(refs:usize,copies:usize,epoch:u32,budget:u32) -> bool {
 let mut processed=false;
 for _ in 0..budget {
  #[cfg(feature="tso")]
  if let Ok(Some(head))=zc::RefQueue::peek(refs) {
   if head.flags & tso::FLAG_TSO_FIRST != 0 {
    if let Some(group)=tso::queued_group(refs,head) {
     let stale=head.flags & tso::FLAG_TSO_GRO!=0 && head.epoch!=epoch;
     let hardware=unsafe {dma_tso_available()}!=0
      && (head.flags & tso::FLAG_TSO_GRO==0 || option_env!("R5S_GRO_TSO")==Some("1"));
     let needed=if hardware {group.gmac_slots} else {1};
     if stale || needed>unsafe {dma_tx_capacity()} {
      for _ in 0..group.fragments {
       let Ok(Some(record))=zc::RefQueue::pop(refs) else {fatal(40)};
       let _=zc::reject(zc::META_VA,&record);
       if stale {zc::count(zc::META_VA,zc::C_GMAC_STALE);}
      }
      if !stale {tso_counters().note_invalid();}
      processed=true;
      continue;
     }
     if unsafe {dma_tx_space()} < needed || (hardware && outstanding().iter().all(Option::is_some)) {
      zc::count(zc::META_VA,zc::C_GMAC_GROUP_DEFERRED);
      break;
     }
    } else {
     let _=zc::RefQueue::pop(refs);
     let _=zc::reject(zc::META_VA,&head);
     tso_counters().note_invalid();
     processed=true;
     continue;
    }
   }
  }
  if unsafe {dma_tx_available()}==0 {break;}
  let Ok(Some(r))=zc::RefQueue::pop(refs) else {break};
  processed=true;
  if r.flags & zc::FLAG_COPY == 0 {
    #[cfg(feature="tso")]
    if r.flags & tso::FLAG_TSO_MASK != 0 {
     if r.flags & tso::FLAG_TSO_FIRST != 0 {
      handle_tso_group(refs,r,None);
     } else {
      let _=zc::reject(zc::META_VA,&r);
      tso_counters().note_invalid();
     }
     continue;
    }
    if r.epoch!=epoch {let _=zc::reject(zc::META_VA,&r);zc::count(zc::META_VA,zc::C_GMAC_STALE);continue;}
    if !send_tx_ref(r,None) {break;}
   continue;
  }
    let result=unsafe {queue::consume_in_place(copies, |frame,tag| {
     (tag==epoch).then(|| dma_send(frame.as_ptr(),frame.len()))
    })};
    let result=match result {Ok(Some(Some(value)))=>value,Ok(Some(None))=>continue,Ok(None)=>fatal(40),Err(())=>fatal(40)};
    if result<0 {fatal((-result) as u32);}
    if result==0 {DROPS.fetch_add(1,Ordering::Relaxed);}
 }
 processed
}
#[cfg(all(feature="rawio",feature="zc"))]
fn send_tx_ref(r:zc::Ref,next_hop:Option<[u8;6]>) -> bool {
    if zc::status(zc::META_VA,r.id())!=(zc::CPU,r.generation()) {
     zc::count(zc::META_VA,zc::C_OWNERSHIP_ERRORS);return true;
    }
    let mut length=r.len as usize;
    let physical=match zc::buffer_physical(zc::POOL_PA,r.id()) {
     Some(physical) if (14..=1514).contains(&length) => physical,
     _ => {let _=zc::reject(zc::META_VA,&r);return true;}
    };
    let buffer=unsafe {core::slice::from_raw_parts_mut((zc::POOL_VA+r.id() as usize*zc::BUFFER) as *mut u8,zc::BUFFER)};
    // A prepared (FLAG_WAN_READY) payload was finalised and cache-maintained by
    // the worker core; only write (and re-clean) when something must change.
    let mut dirty=false;
    if let Some(mac)=next_hop {
     if r.flags & zc::FLAG_WAN_READY != 0 {
      if buffer[..6] != mac {
       buffer[..6].copy_from_slice(&mac);
       dirty=true;
      }
     } else {
     let Some(size)=zc::worker_wan_header(&mut buffer[..length], mac) else {
      let _=zc::reject(zc::META_VA,&r);
      DROPS.fetch_add(1,Ordering::Relaxed);
      return true;
     };
     length=size;
     dirty=true;
     }
    }
    if length<60 && r.flags & zc::FLAG_WAN_READY == 0 {
     // Prepared payloads are already padded (and cleaned) by the worker core.
     buffer[length..60].fill(0);
     dirty=true;
    }
    let checksum = if zc::TX_CHECKSUM_REQUESTED && unsafe {dma_tx_ipv4_checksum_active} != 0 {
     if r.flags & zc::FLAG_WAN_TRANSPORT_READY != 0 {
      3 << 16
     } else {
      let kind = zc::prepare_nat_tx_checksum(&mut buffer[..length], r.flags, false);
      if kind != zc::TxChecksum::None { dirty=true; }
      match kind {
       zc::TxChecksum::None => 0,
       zc::TxChecksum::Ipv4 => 1 << 16,
       zc::TxChecksum::Tcp | zc::TxChecksum::Udp => 3 << 16,
      }
     }
    } else {0};
    // Unprepared payloads are cleaned here as before; prepared ones only when
    // this core wrote (destination refresh) after the worker's maintenance.
    if (r.flags & zc::FLAG_WAN_READY == 0 || dirty)
        && zc::maintain_payload(&r, zc::PayloadCache::BeforeTx(length.max(60))).is_err() {fatal(20);}
    let result=unsafe {dma_send_ref(physical,length,checksum)};
    if result<=0 {fatal(if result==0 {21} else {(-result) as u32});}
    let _=zc::submit(zc::META_VA,&r);
 true
}

#[cfg(all(feature="rawio",feature="zc"))]
fn notify_rx_workers() {
 if zc::wan_fast_path() {
  for worker in 0..4 {
   let refs=zc::fast_wan_requests(worker);
   if zc::RefQueue::depth(refs).is_ok_and(|depth| depth!=0) && zc::RefQueue::claim_wake(refs) {
    sel4_microkit::Channel::new(zc::FAST_WAN_CHANNEL+worker).notify();
   }
  }
 }
}

#[cfg(all(feature="rawio",feature="zc"))]
const FAST_TX_LANE: usize = zc::WORKER_TX_LANES;
#[cfg(all(feature="rawio",feature="zc"))]
const APP_TX_LANE: usize = FAST_TX_LANE + 1;
#[cfg(all(feature="rawio",feature="zc"))]
const TX_LANES: usize = APP_TX_LANE + 1;

#[cfg(all(feature="rawio",feature="zc"))]
fn service_tx(next:&mut usize,epoch:u32,now:u64,budget:u32)->(bool,bool) {
 let batched=option_env!("R5S_GMAC_TX_BATCH")==Some("1");
 let channels=unsafe {dma_tx_channels()};
 if unsafe {dma_select_tx(0)}!=0 {fatal(21);}
 #[cfg(feature="tso")]
 zc::publish_wan_tso(unsafe {dma_tso_available()}!=0);
 if batched {unsafe {dma_begin_tx_batch();}}
 let app_refs=zc::link_queue(zc::LINKS_VA,zc::Q_APP_TX);
 let mut progress=false;
 for control in [zc::WAN_CONTROL_TX, zc::WAN_NEIGHBOR_TX] {
  if unsafe {dma_tx_available()}==0 {break;}
  let sent=unsafe {queue::consume_in_place(control, |frame,tag| {
   (tag==epoch).then(||dma_send(frame.as_ptr(),frame.len()))
  })};
  match sent {
   Ok(Some(Some(result))) if result<0=>fatal((-result) as u32),
   Ok(Some(Some(0)))=>fatal(21),
   Ok(Some(_))=>progress=true,
   Ok(None)=>{},
   Err(())=>fatal(40),
  }
 }
 let revision=zc::fast_wan_tag(zc::FAST_WAN_CONTROL,epoch);
 let mac=revision.and_then(|revision|zc::wan_route(zc::WORKER_WAN_ROUTE,revision,now));
 let mut ready=zc::wan_active_lanes() | (1<<FAST_TX_LANE) | (1<<APP_TX_LANE);
 let mut spent=0;
 let mut worker_drained=0;
 while spent<budget {
  let Some(lane)=zc::next_ready_lane(ready,*next) else {break};
  *next=(lane+1)%TX_LANES;
  let bit=1<<lane;
  let worker=lane<zc::WORKER_TX_LANES;
  if (lane==FAST_TX_LANE && !zc::wan_fast_path()) || (worker && !zc::worker_wan_egress()) {
   ready&=!bit;
   continue;
  }
  let refs=match lane {FAST_TX_LANE=>zc::FAST_WAN_TX,APP_TX_LANE=>app_refs,_=>zc::worker_wan_refs(lane)};
  let channel=if worker {(lane / zc::TX_FLOW_LANES) as u32 % channels} else {0};
  #[cfg(feature="tso")]
  if software_tx(lane).is_some() {
   if unsafe {dma_select_tx(channel)}!=0 {fatal(21);}
   if batched {unsafe {dma_begin_tx_batch();}}
   let before=unsafe {dma_tx_pending()};
   let sent=service_software(lane,epoch,now,1);
   let pending=unsafe {dma_tx_pending()};
   if worker && pending>before {zc::record_wan_tx_fence(refs,channel as usize,pending);}
   spent+=sent;
   progress|=sent!=0;
   if software_tx(lane).is_none() {
    if worker {
     zc::publish_wan_tx_consumed(refs);
     if zc::RefQueue::depth(refs)==Ok(0) {zc::clear_idle_wan_lane(lane,refs);}
    }
   } else if sent==0 {ready&=!bit;}
   continue;
  }
  let depth=zc::RefQueue::depth(refs).unwrap_or(0);
  if depth==0 {
   if worker {zc::clear_idle_wan_lane(lane,refs);}
   ready&=!bit;
   continue;
  }
  if unsafe {dma_select_tx(channel)}!=0 {fatal(21);}
  let before=unsafe {dma_tx_space()};
  if before==0 {
   ready&=!bit;
   if worker {zc::count(zc::META_VA,zc::WORKER_TX_RING_STALL);}
   continue;
  }
  if batched {unsafe {dma_begin_tx_batch();}}
  // Adaptive per-visit quantum: bounded by budget, free descriptors and the
  // lane's own backlog; the ceiling scales with how many lanes are runnable so
  // a single deep lane can drain quickly while several lanes stay fair.
  let quantum=(budget-spent).min(before).min(depth).min(if ready.count_ones()>1 {8} else {32});
  let sent=match lane {
   FAST_TX_LANE=>u32::from(tx_batch(refs,zc::FAST_WAN_FRAMES+queue::DIRECTION,epoch,1)),
   APP_TX_LANE=>u32::from(tx_batch(refs,queue::BASE+queue::DIRECTION,epoch,1)),
   _=>tx_worker_wan(lane,true,now,quantum,revision,mac),
  };
  if worker && sent!=0 {
   let remaining=unsafe {dma_tx_space()};
   if remaining<before {
    zc::record_wan_tx_fence(refs,channel as usize,unsafe {dma_tx_pending()});
   }
   #[cfg(feature="tso")]
   let drained=software_tx(lane).is_none();
   #[cfg(not(feature="tso"))]
   let drained=true;
   if drained {
    zc::publish_wan_tx_consumed(refs);
    if zc::RefQueue::depth(refs)==Ok(0) {zc::clear_idle_wan_lane(lane,refs);}
   }
  }
  if sent!=0 {
   spent+=before.saturating_sub(unsafe {dma_tx_space()}).max(sent);
   if worker {worker_drained+=sent;}
   progress=true;
  } else {
   ready&=!bit;
  }
 }
 zc::add_worker_tx_drained(worker_drained);
 let budget_hit=spent>=budget && worker_drained!=0;
 if budget_hit {zc::count(zc::META_VA,zc::WORKER_TX_BUDGET);}
 for channel in 0..channels {
  if unsafe {dma_select_tx(channel)}!=0 {fatal(21);}
  let pending=unsafe {dma_tx_pending()};
  let peak=zc::counter(zc::META_VA,zc::C_GMAC_TX_PEAK+channel as usize);
  if pending>peak.load(Ordering::Relaxed) {peak.store(pending,Ordering::Relaxed);}
  if batched && unsafe {dma_end_tx_batch()} != 0 {fatal(20);}
 }
 if unsafe {dma_select_tx(0)}!=0 {fatal(21);}
 (progress,budget_hit && mac.is_some())
}
#[cfg(all(feature="rawio",feature="zc"))]
fn tx_worker_wan(lane:usize,enabled:bool,now:u64,limit:u32,revision:Option<u32>,gateway:Option<[u8;6]>) -> u32 {
 let refs=zc::worker_wan_refs(lane);
 let copies=zc::worker_wan_frames(lane);
 let mut processed=0;
 for _ in 0..limit {
  if enabled && unsafe {dma_tx_available()}==0 {break;}
  let Ok(Some(head))=zc::RefQueue::peek(refs) else {break};
  #[cfg(feature="tso")]
  if head.flags & tso::FLAG_TSO_MASK != 0 {
   let group=(head.flags & tso::FLAG_TSO_MASK == tso::FLAG_TSO_FIRST)
    .then(|| tso::queued_group(refs,head)).flatten();
   let Some(group)=group else {
    let _=zc::RefQueue::pop(refs);
    let _=zc::reject(zc::META_VA,&head);
    tso_counters().note_invalid();
    processed+=1;
    continue;
   };
   let created=zc::RefQueue::peek_at(refs,1).ok().flatten().map_or(0,|next| next.meta);
   let stale=!enabled || (now as u32).wrapping_sub(created)>=1000
     || revision.is_some_and(|current| current!=head.epoch);
   let gateway=(!stale && revision==Some(head.epoch)).then_some(gateway).flatten();
   if !stale && gateway.is_none() {
    zc::count(zc::META_VA,zc::C_GMAC_ROUTE_BLOCKED);
    break;
   }
   let hardware=unsafe {dma_tso_available()}!=0 && option_env!("R5S_GRO_TSO")==Some("1");
   let needed=if hardware {group.gmac_slots} else {1};
   if !stale && needed<=unsafe {dma_tx_capacity()} {
    if unsafe {dma_tx_space()}<needed || (hardware && outstanding().iter().all(Option::is_some)) {
     zc::count(zc::META_VA,zc::C_GMAC_GROUP_DEFERRED);
     break;
    }
    let Ok(Some(first))=zc::RefQueue::pop(refs) else {fatal(40)};
    handle_tso_group(refs,first,gateway);
   } else {
    for _ in 0..group.fragments {
     let Ok(Some(record))=zc::RefQueue::pop(refs) else {fatal(40)};
     let _=zc::reject(zc::META_VA,&record);
     if stale {zc::count(zc::META_VA,zc::C_GMAC_STALE);}
    }
    if !stale {tso_counters().note_invalid();}
   }
   processed+=1;
   break;
  }
  let expired=!enabled || (now as u32).wrapping_sub(head.meta)>=1000;
  let matches=revision==Some(head.epoch);
  let mac=if matches && !expired {gateway}else{None};
  if mac.is_none() && !expired && revision.is_none_or(|current| current==head.epoch) {
   zc::count(zc::META_VA,zc::C_GMAC_ROUTE_BLOCKED);
   break;
  }
  if revision.is_some() && !matches {zc::count(zc::META_VA,zc::C_GMAC_STALE);}
  let Ok(Some(record))=zc::RefQueue::pop(refs) else {fatal(40);};
  processed+=1;
  if record.flags & zc::FLAG_COPY!=0 {
   let result=unsafe {queue::consume_in_place(copies, |frame,tag| {
    if tag!=record.epoch {return None;}
    let size=zc::worker_wan_header(frame,mac?)?;
    // The direct worker queue only accepts packets after successful NAT validation.
    if option_env!("R5S_GMAC_COPIED_CHECKSUM") == Some("1") {
     zc::count(zc::META_VA, zc::C_NAT_COPY_CHECKSUM_BYPASS);
     Some(dma_send_nat_validated(frame.as_ptr(),size))
    } else {
     Some(dma_send(frame.as_ptr(),size))
    }
   })};
   match result {
    Ok(Some(Some(sent))) if sent>0 => {},
    Ok(Some(Some(sent))) if sent<0 => fatal((-sent) as u32),
    Ok(Some(_)) => {DROPS.fetch_add(1,Ordering::Relaxed);},
    _ => fatal(40),
   }
  } else if mac.is_none() {
   let _=zc::reject(zc::META_VA,&record);
   DROPS.fetch_add(1,Ordering::Relaxed);
  } else if !send_tx_ref(record,mac) {break;}
 }
 processed
}
/* ---- TSO glue: ref-group submission, hardware requests, software
 * fallback and request-level buffer release (docs/tso-design.md §4-§7).
 * The driver PD is the only descriptor-ring owner; this is the single
 * serialization point for every TSO request. ---- */
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
struct Outstanding { id:u32, refs:[Option<zc::Ref>;tso::TSO_MAX_FRAGS] }
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
struct OutstandingTable(core::cell::UnsafeCell<[Option<Outstanding>;16]>);
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
unsafe impl Sync for OutstandingTable {}
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
static OUTSTANDING:OutstandingTable=OutstandingTable(core::cell::UnsafeCell::new([const {None};16]));
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
struct Scratch(core::cell::UnsafeCell<[u8;2161]>);
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
unsafe impl Sync for Scratch {}
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
static SCRATCH:Scratch=Scratch(core::cell::UnsafeCell::new([0;2161]));

#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
fn tso_counters() -> &'static tso::TsoCounters {
 static COUNTERS:tso::TsoCounters=tso::TsoCounters::new();
 &COUNTERS
}
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
fn outstanding() -> &'static mut [Option<Outstanding>;16] {unsafe {&mut *OUTSTANDING.0.get()}}
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
fn tso_next_id() -> u32 {
 use core::sync::atomic::AtomicU32;
 static NEXT:AtomicU32=AtomicU32::new(1);
 loop {let id=NEXT.fetch_add(1,Ordering::Relaxed);if id!=0 {return id;}}
}
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
fn complete_tso_request(id:u32) {
 let table=outstanding();
 if let Some(pos)=table.iter().position(|e| e.as_ref().map_or(false,|e| e.id==id)) {
  if let Some(entry)=table[pos].take() {
   let _=tso_counters().hw_completed.fetch_update(Ordering::Relaxed,Ordering::Relaxed,|value| value.checked_add(1));
   for r in entry.refs.iter().flatten() {let _=zc::complete(zc::META_VA,r);}
  }
 } else {tso_counters().note_completion_failure();}
}
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
fn free_refs(refs:&[Option<zc::Ref>;tso::TSO_MAX_FRAGS]) {
 for r in refs.iter().flatten() {let _=zc::reject(zc::META_VA,r);}
}
/* Software fallback (contract §5): segment with fully computed checksums
 * and send each frame through the ordinary copy path. Source buffers never
 * enter DMA. The ring-local cursor keeps them alive until segmentation ends. */
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
fn fallback_tso(lane:usize,request:&tso::TsoRequest,refs:&[Option<zc::Ref>;tso::TSO_MAX_FRAGS],route:Option<(u32,[u8;6])>) -> bool {
 let mut cpu=*request;
 cpu.mss=cpu.mss.min(1514u32.saturating_sub(cpu.head_len) as u16);
 for f in cpu.frags[..request.nfrags as usize].iter_mut() {
  let pool_end=zc::POOL_PA as u64+zc::POOL_BYTES as u64;
  if f.physical>=zc::POOL_PA as u64 && f.physical<pool_end {
   f.physical=(zc::POOL_VA+(f.physical-zc::POOL_PA as u64) as usize) as u64;
  }
 }
 let pending=software_tx(lane);
 if pending.is_some() {fatal(40);}
 *pending=Some(SoftwareTx {request:cpu,refs:*refs,next:0,epoch:EPOCH.load(Ordering::Relaxed),route,started:hw_us()/1000});
 true
}

#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
struct SoftwareTx {
 request:tso::TsoRequest,
 refs:[Option<zc::Ref>;tso::TSO_MAX_FRAGS],
 next:u32,
 epoch:u32,
 route:Option<(u32,[u8;6])>,
 started:u64,
}
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
struct SoftwareRings(core::cell::UnsafeCell<[Option<SoftwareTx>;TX_LANES]>);
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
unsafe impl Sync for SoftwareRings {}
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
fn software_tx(lane:usize)->&'static mut Option<SoftwareTx> {
 static RINGS:SoftwareRings=SoftwareRings(core::cell::UnsafeCell::new([const {None};TX_LANES]));
 unsafe {&mut (*RINGS.0.get())[lane]}
}
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
fn service_software(lane:usize,epoch:u32,now:u64,budget:u32)->u32 {
 let pending=software_tx(lane);
 let Some(request)=pending.as_mut() else {return 0};
 if request.epoch!=epoch {
  free_refs(&pending.take().unwrap().refs);
  return 1;
 }
 if let Some((revision,mac))=request.route {
  let current=zc::fast_wan_tag(zc::FAST_WAN_CONTROL,epoch);
  let gateway=current.and_then(|current|zc::wan_route(zc::WORKER_WAN_ROUTE,current,now));
  if current.is_some_and(|current|current!=revision) || gateway.is_some_and(|next|next!=mac)
     || (gateway.is_none() && now.saturating_sub(request.started)>=1000) {
   free_refs(&pending.take().unwrap().refs);
   return 1;
  }
  if gateway.is_none() {return 0;}
 }
 let scratch=unsafe {&mut *SCRATCH.0.get()};
 let before=request.next;
 let limit=budget.min(unsafe {dma_tx_space()});
 let result=tso::segment_window(&request.request,scratch,&mut request.next,limit,&mut |frame:&[u8]| {
  let sent=unsafe {dma_send_nat_validated(frame.as_ptr(),frame.len())};
  if sent<0 {fatal((-sent) as u32);}
  if sent==0 {Err(tso::TsoError::Busy)} else {Ok(())}
 });
 let sent=request.next-before;
 match result {
  Ok(false)|Err(tso::TsoError::Busy)=>{},
  Ok(true)=>{
   let done=pending.take().unwrap();
   tso_counters().note_fallback(done.next);
   if done.request.flags & tso::GRO_SOFTWARE!=0 {
    zc::counter(zc::META_VA,zc::C_GRO_SW_SEGMENTS).fetch_add(done.next,Ordering::Relaxed);
   }
   free_refs(&done.refs);
  }
  Err(_)=>{
   tso_counters().note_invalid();
   free_refs(&pending.take().unwrap().refs);
  }
 }
 sent
}
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
fn submit_tso_request(lane:usize,request:&tso::TsoRequest,refs:&[Option<zc::Ref>;tso::TSO_MAX_FRAGS],route:Option<(u32,[u8;6])>) -> bool {
 tso_counters().note_request();
 let segs=(request.payload_len+u32::from(request.mss)-1)/u32::from(request.mss);
 if request.flags & tso::SOFTWARE_SEGMENTATION == 0 && unsafe {dma_tso_available()}!=0
  && request.head_len as u32+request.payload_len.min(u32::from(request.mss))<=1514
  && let Some(slot)=outstanding().iter().position(Option::is_none)
 {
  for r in refs.iter().flatten() {
   if zc::maintain_payload(r, zc::PayloadCache::BeforeTx(zc::BUFFER)).is_err() {fatal(20);}
  }
  outstanding()[slot]=Some(Outstanding{id:request.tso_id,refs:*refs});
  let result=unsafe {dma_send_tso(request as *const tso::TsoRequest)};
  if result>0 {
   tso_counters().note_hw(segs);
   for r in refs.iter().flatten() {let _=zc::submit(zc::META_VA,r);}
   return true;
  }
  if result<0 && result!=-30 && result!=-27 {
   tso_counters().note_desc_error();
   fatal((-result) as u32);
  }
  outstanding()[slot]=None;
  if result==-27 {tso_counters().note_invalid();free_refs(refs);return false;}
 }
 fallback_tso(lane,request,refs,route)
}
/* One TSO ref group from the bounded egress queue (contract §6). */
#[cfg(all(feature="rawio",feature="zc",feature="tso"))]
fn handle_tso_group(queue:usize,first:zc::Ref,next_hop:Option<[u8;6]>) {
 let mut collector=tso::GroupCollector::new();
 let mut refs:[Option<zc::Ref>;tso::TSO_MAX_FRAGS]=[None;tso::TSO_MAX_FRAGS];
 let mut n=0usize;let mut complete=false;let mut ok=true;let mut cur=Some(first);
 while let Some(r)=cur.take() {
  if n>=tso::TSO_MAX_FRAGS {let _=zc::reject(zc::META_VA,&r);ok=false;break;}
  refs[n]=Some(r);n+=1;
  if zc::status(zc::META_VA,r.id())!=(zc::CPU,r.generation()) {
   zc::count(zc::META_VA,zc::C_OWNERSHIP_ERRORS);
   ok=false;
   break;
  }
  let physical=zc::buffer_physical(zc::POOL_PA,r.id()).unwrap_or(0);
  match collector.push(tso::GroupRef{flags:r.flags,meta:r.meta,len:r.len},physical) {
   Ok(true) => {complete=true;break;}
   Ok(false) => {
    cur=match zc::RefQueue::peek(queue) {
     Ok(Some(next)) if next.flags & (tso::FLAG_TSO_CONT|tso::FLAG_TSO_LAST)!=0 => {
      let _=zc::RefQueue::pop(queue);
      Some(next)
     }
     _ => {ok=false;break;}
    };
   }
   Err(_) => {ok=false;break;}
  }
 }
 if ok && complete && let Some(mac)=next_hop {
  let frame=unsafe {core::slice::from_raw_parts_mut(
   (zc::POOL_VA+first.id() as usize*zc::BUFFER) as *mut u8,usize::from(first.len))};
  ok=zc::worker_wan_header(frame,mac).is_some();
 }
 if ok && complete {
  if let Ok(request)=collector.request(tso_next_id()) {
   let lane=if queue==zc::FAST_WAN_TX {FAST_TX_LANE} else if next_hop.is_none() {APP_TX_LANE} else {
    (0..zc::WORKER_TX_LANES).find(|&lane|zc::worker_wan_refs(lane)==queue).unwrap_or_else(||fatal(40))
   };
   if submit_tso_request(lane,&request,&refs,next_hop.map(|mac|(first.epoch,mac))) {return;}
   return; /* submit frees the refs on every failure path */
  }
  tso_counters().note_invalid();
 } else {tso_counters().note_invalid();}
 free_refs(&refs);
}
/* Board-side TSO test source (docs/tso-hardware-validation.md): three
 * oversized IPv4 TCP requests through the ordinary TX interface, once the
 * link is up and DMA is running. */
#[cfg(all(feature="rawio",feature="zc",feature="tso-test"))]
fn generate_tso_test() {
 static DONE:AtomicU32=AtomicU32::new(0);
 if DONE.swap(1,Ordering::Relaxed)!=0 {return;}
 for test_id in 1..=3u32 {
  let mut refs:[Option<zc::Ref>;tso::TSO_MAX_FRAGS]=[None;tso::TSO_MAX_FRAGS];
  let mut addrs=[0u64;tso::TSO_MAX_FRAGS];let mut count=0usize;let mut failed=false;
  while count<tso::TSO_MAX_FRAGS {
   match zc::alloc(zc::META_VA,zc::PARTITION_WAN) {
    Some(r) => {
     if zc::publish(zc::META_VA,&r).is_err() {fatal(41);}
     addrs[count]=zc::buffer_physical(zc::POOL_PA,r.id()).unwrap_or(0);
     refs[count]=Some(r);count+=1;
    }
    None => {failed=true;break;}
   }
  }
  if failed || addrs.iter().any(|&a| a==0) {free_refs(&refs);return;}
  let mut buffers:[&mut [u8];tso::TSO_MAX_FRAGS]=core::array::from_fn(|i| unsafe {
   core::slice::from_raw_parts_mut(
    (zc::POOL_VA+refs[i].unwrap().id() as usize*zc::BUFFER) as *mut u8,zc::BUFFER)
  });
  let spec=tso_testgen::TestSpec::default_ipv4(test_id);
  match tso_testgen::build(&spec,&mut buffers,&addrs) {
   Ok(request) => {
    zc::barrier::dma_publish();
    submit_tso_request(APP_TX_LANE,&request,&refs,None);
   }
   Err(_) => {free_refs(&refs);tso_counters().note_invalid();}
  }
 }
}


#[cfg(feature="rawio")]
fn fatal(reason:u32)->! {
 #[cfg(feature="wan-shutdown")]
 { shutdown(reason,unsafe {&*(0x40001014 as *const AtomicU32)}.load(Ordering::Acquire)); }
 #[cfg(not(feature="wan-shutdown"))]
 {
 unsafe {link_stop();}EPOCH.fetch_add(1,Ordering::Relaxed);
 FAULT.store(reason,Ordering::Relaxed);STATE.store(3,Ordering::Relaxed);
 loop {hw_progress();}
 }
}
#[cfg(all(feature="rawio", feature="zc"))]
#[unsafe(no_mangle)]
extern "C" fn dma_ref_invalidate(physical: u64) -> i32 {
 match pool_ref(physical, zc::RX_DMA) {
  Some(r) if zc::maintain_payload(&r, zc::PayloadCache::AfterRx(zc::BUFFER)).is_ok() => 0,
  _ => 1,
 }
}
#[cfg(feature="wan-shutdown")]
fn shutdown(fault:u32,request:u32)->! {
 let started_ms=(hw_us()/1000) as u32;
 let dma_was_running=unsafe {dma_enabled()}!=0;
 FAULT.store(fault,Ordering::Relaxed);
 STATE.store(3,Ordering::Relaxed);
 EPOCH.fetch_add(1,Ordering::Relaxed);
 let result=stop_dma();
 unsafe {link_stop();}
 word(128).store(request,Ordering::Relaxed);
 word(132).store(result as u32,Ordering::Relaxed);
 word(136).store(u32::from(dma_was_running),Ordering::Relaxed);
 word(140).store(started_ms,Ordering::Relaxed);
 word(124).store(1,Ordering::Release);
 loop {hw_progress();core::hint::spin_loop();}
}
#[unsafe(no_mangle)] extern "C" fn dma_virtual()->usize {cache::POOL}
#[unsafe(no_mangle)] extern "C" fn dma_physical()->u64 {0x24000000}
#[unsafe(no_mangle)] extern "C" fn dma_cache(op:u32,address:usize,length:usize)->i32 {match cache::maintain(op,address,length) {Ok(())=>0,Err(code)=>code as i32}}
/// CPU virtual address of a device-visible buffer address known to the DMA
/// backend: a private-pool offset or a shared zero-copy pool buffer.
#[unsafe(no_mangle)] extern "C" fn dma_ref_virtual(physical:u64)->usize {
 #[cfg(feature="zc")]
 {
  if physical>=0x24000000 && physical<0x24000000+cache::SIZE as u64 {
   return cache::POOL+(physical-0x24000000) as usize;
  }
  if physical>=zc::POOL_PA && physical<zc::POOL_PA+zc::POOL_BYTES as u64 {
   return zc::POOL_VA+(physical-zc::POOL_PA) as usize;
  }
 }
 let _=physical;
 0
}
/// Reconstruct the reference of a pool buffer from its device-visible
/// address while the driver owns it in `expected`.
#[cfg(feature="zc")]
fn pool_ref(physical:u64,expected:u32)->Option<zc::Ref> {
 let offset=physical.checked_sub(zc::POOL_PA)?;
 if offset%zc::BUFFER as u64!=0 {return None;}
 let id=(offset/zc::BUFFER as u64) as u32;
 if id>=zc::BUFFERS as u32 {return None;}
 let (state,generation)=zc::status(zc::META_VA,id);
 if state!=expected {
  zc::count(zc::META_VA,zc::C_OWNERSHIP_ERRORS);
  return None;
 }
 Some(zc::Ref::new(id,generation))
}

#[cfg(feature="rawio")]
fn stop_dma()->i32 {
 let result=unsafe {dma_stop()};
 #[cfg(feature="zc")]
 if result==0 {release_stopped_refs();}
 result
}

#[cfg(all(feature="rawio",feature="zc"))]
fn release_stopped_refs() {
 zc::publish_wan_tso(false);
 let mut slot=0;
 loop {
  let (mut rx,mut tx)=(0,0);
  if unsafe {dma_take_stopped_refs(slot,&mut rx,&mut tx)}==0 {break;}
  if rx!=0 && let Some(r)=pool_ref(rx,zc::RX_DMA) {let _=zc::recycle(zc::META_VA,&r);}
  if tx!=0 && let Some(r)=pool_ref(tx,zc::TX_DMA) {let _=zc::cancel_stopped_tx(zc::META_VA,&r);}
  slot+=1;
 }
 #[cfg(feature="tso")]
 for lane in 0..TX_LANES {
  if let Some(request)=software_tx(lane).take() {free_refs(&request.refs);}
 }
 #[cfg(feature="tso")]
 for entry in outstanding().iter_mut().filter_map(Option::take) {
  for r in entry.refs.iter().flatten() {
   if zc::status(zc::META_VA,r.id())==(zc::TX_DMA,r.generation()) {let _=zc::cancel_stopped_tx(zc::META_VA,r);}
   else {let _=zc::reject(zc::META_VA,r);}
  }
  tso_counters().note_completion_failure();
 }
 if zc::worker_wan_egress() {
  for lane in 0..zc::WORKER_TX_LANES { zc::clear_wan_tx_fence(zc::worker_wan_refs(lane)); }
 }
}
