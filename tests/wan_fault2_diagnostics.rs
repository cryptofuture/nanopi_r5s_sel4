#[path = "../pds/network_supervisor/src/fault_display.rs"]
mod display;
#[path = "../pds/network_supervisor/src/shared.rs"]
mod shared;
#[path = "../pds/wan_dhcp_diagnostic/poll_deadline.rs"]
mod timing;

use core::sync::atomic::{AtomicU32, Ordering};

#[test]
fn status_accepts_published_data_and_rejects_in_progress_snapshot() {
    let page = [AtomicU32::new(0), AtomicU32::new(42)];
    let address = page.as_ptr() as usize;
    assert_eq!(shared::read::<2>(address), None);
    page[0].store(1, Ordering::Release);
    assert_eq!(shared::read::<2>(address), None);
    page[0].store(2, Ordering::Release);
    assert_eq!(shared::read::<2>(address), Some([2, 42]));
}

#[test]
fn fault_frames_use_distinct_flutter_and_round_trip_raw_phases() {
    for phase in 0..=255 {
        for id in timing::extension(2, true, phase, 255 - phase) {
            let mut frame = display::FaultDisplay::new();
            frame.latch(id, 0);
            frame.latch(id ^ 1, 1);
            assert!(!frame.tick(2750));
            assert!(frame.tick_with_flutter(2750, 3000));
            let mut bits = 0u8;
            for i in 0..8 {
                let time = display::TRANSITION_MS + display::FAULT_MS + i * display::SLOT_MS
                    + if i >= 4 { display::GROUP_SEPARATOR_MS } else { 0 };
                assert!(frame.tick_with_flutter(time, 3000));
                bits = (bits << 1) | u8::from(frame.tick_with_flutter(time + 1000, 3000));
            }
            assert_eq!(display::decode(bits), Some(id));
        }
    }
}

#[test]
fn every_timing_bucket_and_inactive_phase_is_distinguishable() {
    for span in 0..7 {
        let mut stamps = [0u64; 9];
        stamps[span + 1..].fill(240_001);
        assert_eq!(timing::longest_span(&stamps), span as u8);
    }
    assert_eq!(timing::longest_span(&[2, 1, 2, 3, 4, 5, 6, 7, 8]), 7);
    assert_eq!(timing::lan_context(255, 0), 80);
    assert_eq!(timing::lan_context(255, 8), 84);
    assert_eq!(timing::lan_context(12, 11), 87);
}
