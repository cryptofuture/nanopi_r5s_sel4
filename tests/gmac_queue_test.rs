#[path="../pds/gmac_dma/src/queue.rs"] mod queue;
#[test] fn wrap_full_epoch_and_malformed_metadata() {
 let mut memory=vec![0u32;queue::DIRECTION/4];let base=memory.as_mut_ptr() as usize;
 let data=[0xa5;1514];let mut output=[0;1514];
 for _ in 0..1000 {for _ in 0..queue::COUNT {assert_eq!(queue::push(base,7,&data),Ok(true));}assert_eq!(queue::push(base,7,&data),Ok(false));for _ in 0..queue::COUNT {assert_eq!(queue::pop(base,7,&mut output),Ok(1514));assert_eq!(output,data);}assert_eq!(queue::pop(base,7,&mut output),Ok(0));}
 assert_eq!(queue::push(base,6,&data),Ok(true));assert_eq!(queue::pop(base,7,&mut output),Ok(0));
 memory.fill(0);memory[0]=u32::MAX;memory[1]=u32::MAX;assert_eq!(queue::push(base,7,&data),Ok(true));assert_eq!(queue::pop(base,7,&mut output),Ok(1514));
 memory.fill(0);assert_eq!(queue::push(base,7,&data),Ok(true));memory[16]=u32::MAX;assert_eq!(queue::pop(base,7,&mut output),Err(()));
 memory.fill(0);memory[0]=queue::COUNT+1;assert_eq!(queue::push(base,7,&data),Err(()));assert_eq!(queue::pop(base,7,&mut output),Err(()));
 assert_eq!(queue::push(base,7,&data[..13]),Err(()));
}
