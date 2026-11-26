pub const fn masked(pin: u32, high: bool) -> u32 {
    assert!(pin < 32);
    let bit = 1 << (pin % 16);
    (bit << 16) | if high { bit } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_write_mask() {
        assert_eq!(masked(26, true), 0x04000400);
        assert_eq!(masked(26, false), 0x04000000);
        assert_eq!(masked(15, true), 0x80008000);
        assert_eq!(masked(16, true), 0x00010001);
    }
}
