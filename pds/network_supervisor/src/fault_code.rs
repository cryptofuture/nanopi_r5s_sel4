pub fn select(reason: u32, driver: Option<[u32; 24]>) -> u8 {
    if reason == 4 {
        if let Some(v) = driver {
            if v[1] == 1 && v[23] == 2 && v[2] == 3 {
                return match v[3] {
                    n @ 1..=10 => 16 + n as u8,
                    28 => 108,
                    n @ 20..=27 => 32 + (n - 20) as u8,
                    40 => 40,
                    99 => 41,
                    n @ 101..=111 => 48 + (n - 101) as u8,
                    200 => 59,
                    201 => 60,
                    120 => 61,
                    121 => 62,
                    _ => 0,
                };
            }
        }
    }
    match reason { 1..=9 => reason as u8, _ => 0 }
}
