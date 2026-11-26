use core::sync::atomic::{AtomicU32,Ordering};
pub const BASE:usize=0x61000000;
pub const SLOT:usize=1536;
pub const COUNT:u32=8;
pub const DIRECTION:usize=0x4000;
pub const FORWARD_TAG_SHIFT:u32=4;
fn atomic(base:usize,offset:usize)->&'static AtomicU32 {unsafe {&*((base+offset) as *const AtomicU32)}}
const PADDED: bool = match option_env!("R5S_QUEUE_PAD") {
    Some(value) => matches!(value.as_bytes(), b"1"),
    None => false,
};
const TAIL: usize = if PADDED {64}else{4};
const ENTRIES: usize = if PADDED {128}else{64};
const _: () = assert!(ENTRIES + COUNT as usize * SLOT <= 0x3100);
const METADATA: usize = 1528;

pub fn push(base:usize,generation:u32,data:&[u8])->Result<bool,()> {
 push_meta(base,generation,data,0)
}

pub fn push_meta(base:usize,generation:u32,data:&[u8],metadata:u32)->Result<bool,()> {
 if !(14..=1514).contains(&data.len()) {return Err(());}
 let head=atomic(base,0).load(Ordering::Relaxed);let tail=atomic(base,TAIL).load(Ordering::Acquire);
 if head.wrapping_sub(tail)>COUNT {return Err(());}
 if head.wrapping_sub(tail)==COUNT {return Ok(false);}
 let address=base+ENTRIES+(head%COUNT) as usize*SLOT;
 unsafe {core::ptr::write_volatile(address as *mut u32,data.len() as u32);core::ptr::write_volatile((address+4) as *mut u32,generation);core::ptr::copy_nonoverlapping(data.as_ptr(),(address+8) as *mut u8,data.len());core::ptr::write_volatile((address+METADATA) as *mut u32,metadata);}
 atomic(base,0).store(head.wrapping_add(1),Ordering::Release);Ok(true)
}
pub fn pop(base:usize,generation:u32,out:&mut [u8])->Result<usize,()> {
 Ok(take(base, Some(generation), out)?.0)
}

pub fn pop_tagged(base: usize, out: &mut [u8]) -> Result<(usize, u32), ()> {
 take(base, None, out).map(|(n,tag,_)|(n,tag))
}
pub fn pop_meta(base:usize,generation:u32,out:&mut [u8])->Result<(usize,u32),()> {
 take(base,Some(generation),out).map(|(n,_,metadata)|(n,metadata))
}

fn take(base:usize,generation:Option<u32>,out:&mut [u8])->Result<(usize,u32,u32),()> {
 let tail=atomic(base,TAIL).load(Ordering::Relaxed);let head=atomic(base,0).load(Ordering::Acquire);
 if head.wrapping_sub(tail)>COUNT {return Err(());}
 if head==tail {return Ok((0,0,0));}
 let address=base+ENTRIES+(tail%COUNT) as usize*SLOT;
 let size=unsafe {core::ptr::read_volatile(address as *const u32)} as usize;
 let epoch=unsafe {core::ptr::read_volatile((address+4) as *const u32)};
 if !(14..=1514).contains(&size) || size>out.len() {return Err(());}
 let accept=generation.is_none_or(|expected|expected==epoch);
 if accept {unsafe {core::ptr::copy_nonoverlapping((address+8) as *const u8,out.as_mut_ptr(),size);}}
 let metadata=if accept {unsafe {core::ptr::read_volatile((address+METADATA) as *const u32)}} else {0};
 atomic(base,TAIL).store(tail.wrapping_add(1),Ordering::Release);
 Ok((if accept {size}else{0},epoch,metadata))
}

/// The SPSC consumer holds this slot until the callback returns.
pub unsafe fn consume_in_place<T>(
    base: usize, consume: impl FnOnce(&mut [u8], u32) -> T,
) -> Result<Option<T>, ()> {
    unsafe {consume_in_place_meta(base, |frame,tag,_|consume(frame,tag))}
}

/// The caller must own the SPSC consumer endpoint and keep it mapped for the call.
pub unsafe fn consume_in_place_meta<T>(
    base: usize, consume: impl FnOnce(&mut [u8], u32, u32) -> T,
) -> Result<Option<T>, ()> {
    let tail=atomic(base,TAIL).load(Ordering::Relaxed);
    let head=atomic(base,0).load(Ordering::Acquire);
    if head.wrapping_sub(tail)>COUNT {return Err(());}
    if head==tail {return Ok(None);}
    let address=base+ENTRIES+(tail%COUNT) as usize*SLOT;
    let size=unsafe {core::ptr::read_volatile(address as *const u32)} as usize;
    let tag=unsafe {core::ptr::read_volatile((address+4) as *const u32)};
    if !(14..=1514).contains(&size) {return Err(());}
    let frame=unsafe {core::slice::from_raw_parts_mut((address+8) as *mut u8,size)};
    let metadata=unsafe {core::ptr::read_volatile((address+METADATA) as *const u32)};
    let result=consume(frame,tag,metadata);
    atomic(base,TAIL).store(tail.wrapping_add(1),Ordering::Release);
    Ok(Some(result))
}

pub fn produced(base: usize) -> u32 { atomic(base,0).load(Ordering::Acquire) }
pub fn consumed(base: usize) -> u32 { atomic(base,TAIL).load(Ordering::Acquire) }

pub fn depth(base:usize)->Result<u32,()> {
 let head=atomic(base,0).load(Ordering::Acquire);
 let tail=atomic(base,TAIL).load(Ordering::Acquire);
 let used=head.wrapping_sub(tail);
 if used>COUNT {Err(())} else {Ok(used)}
}

pub fn producer_has_space(base: usize) -> Result<bool, ()> {
    let head = atomic(base, 0).load(Ordering::Relaxed);
    let tail = atomic(base, TAIL).load(Ordering::Acquire);
    let pending = head.wrapping_sub(tail);
    if pending > COUNT { return Err(()); }
    Ok(pending < COUNT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn producer_waits_for_space_across_index_wrap() {
        let queue = [const { AtomicU32::new(0) }; DIRECTION / 4];
        let base = queue.as_ptr() as usize;
        atomic(base, 0).store(u32::MAX, Ordering::Relaxed);
        atomic(base, TAIL).store(u32::MAX, Ordering::Relaxed);
        for marker in 0..COUNT as u8 {
            assert_eq!(producer_has_space(base), Ok(true));
            assert_eq!(push(base, 1, &[marker; 60]), Ok(true));
        }
        assert_eq!(producer_has_space(base), Ok(false));
        let mut frame = [0xff; 60];
        assert_eq!(pop(base, 1, &mut frame), Ok(60));
        assert_eq!(frame, [0; 60]);
        assert_eq!(producer_has_space(base), Ok(true));
        atomic(base, TAIL).store(u32::MAX - 1, Ordering::Relaxed);
        assert_eq!(producer_has_space(base), Err(()));
    }
}

#[cfg(test)]
#[test]
fn tagged_cross_thread_queue_preserves_order_and_payload() {
    let memory = [const { AtomicU32::new(0) }; DIRECTION / 4];
    let base = memory.as_ptr() as usize;
    atomic(base,0).store(u32::MAX-3,Ordering::Relaxed);
    atomic(base,TAIL).store(u32::MAX-3,Ordering::Relaxed);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for tag in 0..10_000 {
                let frame=[tag as u8;60];
                while !push(base,tag,&frame).unwrap() {std::thread::yield_now();}
            }
        });
        let mut out=[0;60];
        for expected in 0..10_000 {
            loop {
                let (length,tag)=pop_tagged(base,&mut out).unwrap();
                if length!=0 {
                    assert_eq!((length,tag),(60,expected));
                    assert_eq!(out,[expected as u8;60]);
                    break;
                }
                std::thread::yield_now();
            }
        }
    });
    assert_eq!(produced(base),consumed(base));
}

#[cfg(test)]
#[test]
fn in_place_consumer_keeps_slot_until_callback_finishes() {
    let memory = [const { AtomicU32::new(0) }; DIRECTION / 4];
    let base = memory.as_ptr() as usize;
    for tag in 0..COUNT { assert_eq!(push(base,tag,&[tag as u8;60]),Ok(true)); }
    let released=unsafe {consume_in_place(base, |frame,tag| {
        assert_eq!(tag,0);
        assert_eq!(frame,&[0;60]);
        assert_eq!(push(base,91,&[91;60]),Ok(false));
        frame[14]=42;
        frame[14]
    })};
    assert_eq!(released,Ok(Some(42)));
    assert_eq!(push(base,91,&[91;60]),Ok(true));
    let mut out=[0;60];
    assert_eq!(pop_tagged(base,&mut out),Ok((60,1)));
    assert_eq!(out,[1;60]);
}


#[cfg(test)]
#[test]
fn checksum_metadata_is_bound_to_slot_and_epoch() {
    let memory=[const {AtomicU32::new(0)}; DIRECTION/4];
    let base=memory.as_ptr() as usize;
    let mut frame=[0;1514];
    assert_eq!(push_meta(base,7,&[0;60],17),Ok(true));
    assert_eq!(pop_meta(base,8,&mut frame),Ok((0,0)));
    assert_eq!(push_meta(base,8,&[3;1514],6),Ok(true));
    assert_eq!(unsafe {consume_in_place_meta(base,|packet,epoch,meta|
        (packet.len(),packet[1513],epoch,meta))},Ok(Some((1514,3,8,6))));
    assert_eq!(push(base,8,&[0;60]),Ok(true));
    assert_eq!(pop_meta(base,8,&mut frame),Ok((60,0)));
}

pub const fn batch(setting: Option<&str>, default: u32) -> u32 {
    let Some(setting) = setting else { return default };
    match setting.as_bytes() {
        b"8" => 8,
        b"16" => 16,
        b"32" => 32,
        b"64" => 64,
        _ => default,
    }
}
