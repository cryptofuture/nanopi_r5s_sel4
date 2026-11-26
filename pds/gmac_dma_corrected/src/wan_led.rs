const CLOCK: (u32,u32) = (3,0x37c);
const MUX: (u32,u32) = (1,0x30);
const DATA: (u32,u32) = (6,4);
const DIRECTION: (u32,u32) = (6,12);
const BIT: u32 = 2;

pub fn allowed(region:u32, offset:u32, value:u32) -> bool {
    match (region,offset) {
        CLOCK => value == 0x00100000,
        MUX => value == 0x00f00000,
        DATA => value == BIT << 16 || value == (BIT << 16 | BIT),
        DIRECTION => value == (BIT << 16 | BIT),
        _ => false,
    }
}

pub fn setup(mut read: impl FnMut(u32,u32)->u32, mut write: impl FnMut(u32,u32,u32)) -> bool {
    write(CLOCK.0,CLOCK.1,0x00100000);
    if read(CLOCK.0,CLOCK.1) & 16 != 0 || matches!(read(6,0x78),0|u32::MAX) { return false; }
    write(DATA.0,DATA.1,BIT << 16);
    write(DIRECTION.0,DIRECTION.1,BIT << 16 | BIT);
    if read(DATA.0,DATA.1) & BIT != 0 || read(DIRECTION.0,DIRECTION.1) & BIT == 0 { return false; }
    write(MUX.0,MUX.1,0x00f00000);
    read(MUX.0,MUX.1) & 0xf0 == 0
}

#[derive(Default)]
pub struct Indicator { next:u64, packets:u64, on:bool }
impl Indicator {
    pub fn update(&mut self, now:u64, link:bool, packets:u64) -> Option<u32> {
        if now < self.next { return None; }
        self.next = now.saturating_add(100);
        let on = link && (packets == self.packets || !self.on);
        self.packets = packets;
        if on == self.on { return None; }
        self.on = on;
        Some(BIT << 16 | if on { BIT } else { 0 })
    }
}
