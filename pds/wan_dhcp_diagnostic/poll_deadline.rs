pub const COUNTER_HZ: u64 = 24_000_000;
pub const TICKS_PER_MS: u64 = COUNTER_HZ / 1000;
pub const DEADLINE_TICKS: u64 = 10 * TICKS_PER_MS;

pub fn missed(previous: &mut u64, now: u64) -> Option<u64> {
    let elapsed = now.wrapping_sub(core::mem::replace(previous, now));
    (elapsed > DEADLINE_TICKS).then_some(elapsed)
}

pub fn fault_after_gap(reason: u32, router_running: bool) -> u32 {
    if reason == 0 && !router_running { 2 } else { reason }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measurement_keeps_the_ten_ms_wall_time_limit_without_rounding() {
        let mut previous = 17;
        assert_eq!(missed(&mut previous, 17 + DEADLINE_TICKS), None);
        assert_eq!(missed(&mut previous, 18 + 2 * DEADLINE_TICKS),
                   Some(DEADLINE_TICKS + 1));
    }

    #[test]
    fn replenishment_gaps_are_allowed_and_a_real_stall_is_reported() {
        let mut previous = 0;
        for period in 1..=30 {
            assert_eq!(missed(&mut previous, period * TICKS_PER_MS), None);
        }
        assert_eq!(missed(&mut previous, 43 * TICKS_PER_MS), Some(13 * TICKS_PER_MS));
    }

    #[test]
    fn runtime_router_gap_preserves_the_independent_watchdogs() {
        let mut previous = 0;
        assert_eq!(missed(&mut previous, DEADLINE_TICKS + 1), Some(DEADLINE_TICKS + 1));
        assert_eq!(fault_after_gap(0, false), 2);
        assert_eq!(fault_after_gap(0, true), 0);
        assert_eq!(fault_after_gap(6, true), 6);
    }

    #[test]
    fn elapsed_time_survives_counter_wrap() {
        assert_eq!(missed(&mut (u64::MAX - 5), 8), None);
    }
}

// Contextual detail after fault 2: longest measured segment and independent heartbeat activity.
pub fn detail(previous: u64, after_policy: u64, after_gpio: u64, now: u64, ran: bool) -> u8 {
    let spans = [after_policy.wrapping_sub(previous), after_gpio.wrapping_sub(after_policy),
                 now.wrapping_sub(after_gpio)];
    if spans.iter().any(|&t| t > i64::MAX as u64) { return 7; }
    let mut longest = 0;
    for i in 1..3 { if spans[i] > spans[longest] { longest = i; } }
    1 + longest as u8 + if ran { 3 } else { 0 }
}

#[cfg(test)]
#[test]
fn gap_location_is_independent_of_other_pd_progress() {
    for (policy, gpio, expected) in [(11,12,1), (1,12,2), (1,2,3)] {
        assert_eq!(detail(0,policy,gpio,13,false),expected);
        assert_eq!(detail(0,policy,gpio,13,true),expected+3);
    }
    assert_eq!(detail(10,12,11,30,true),7);
}

pub fn startup_due(selected: u8, sent: u8, elapsed_ms: u64) -> u8 {
    let eligible = if elapsed_ms >= 4000 { 3 } else if elapsed_ms >= 2000 {
        if selected == 2 { 2 } else { 1 }
    } else { 0 };
    selected & eligible & !sent
}

pub fn lan_context(first: u32, second: u32) -> u8 {
    let active = |phase| match phase { 1..=5 | 8..=12 => 1, 6 => 2, 7 => 3, _ => 0 };
    match (active(first), active(second)) {
        (0, 0) => 80,
        (a, 0) => 80+a,
        (0, b) => 83+b,
        _ => 87,
    }
}

#[test]
fn startup_is_staggered_and_does_not_relax_the_deadline() {
    assert_eq!(startup_due(3,0,1999),0);
    assert_eq!(startup_due(3,0,2000),1);
    assert_eq!(startup_due(3,1,4000),2);
    assert_eq!(startup_due(2,0,2000),2);
    assert_eq!(startup_due(0,0,4000),0);
    let mut previous=0;
    for ms in 1..=90_000 {
        assert_eq!(missed(&mut previous, ms*TICKS_PER_MS),None);
    }
    assert!(missed(&mut previous,90_011*TICKS_PER_MS).is_some());
    assert_eq!(lan_context(4,0),81);
    assert_eq!(lan_context(0,4),84);
    assert_eq!(lan_context(6,7),87);
}

pub fn health_evidence(previous: u32, before_poll: u32, at_detection: u32) -> (bool, bool) {
    (previous != before_poll || before_poll != at_detection, before_poll != at_detection)
}

pub fn longest_span(stamps: &[u64; 9]) -> u8 {
    let mut longest = (0, 0);
    for (index, pair) in stamps[..8].windows(2).enumerate() {
        let span = pair[1].wrapping_sub(pair[0]);
        if span > i64::MAX as u64 { return 7; }
        if span > longest.0 { longest = (span, index as u8); }
    }
    longest.1
}

pub fn extension(span: u8, bracket_changed: bool, lan1: u8, lan2: u8) -> [u8; 5] {
    [
        32 + span + 8 * u8::from(bracket_changed),
        48 + (lan1 >> 4), 64 + (lan1 & 15),
        80 + (lan2 >> 4), 112 + (lan2 & 15),
    ]
}

#[test]
fn delayed_health_sample_preserves_deadline_and_flags_uncertain_epoch() {
    let mut previous = 0;
    assert_eq!(missed(&mut previous, 240_001), Some(240_001));
    assert_eq!(health_evidence(42, 42, 43), (true, true));
    assert_eq!(health_evidence(42, 43, 43), (true, false));
    assert_eq!(health_evidence(42, 42, 42), (false, false));
}

#[test]
fn timing_subspans_and_optical_bytes_preserve_wrapped_samples() {
    let stamps = [u64::MAX - 4, u64::MAX - 2, 0, 1, 2, 3, 240_004, 240_005, 240_006];
    assert_eq!(longest_span(&stamps), 5);
    assert_eq!(extension(5, true, 255, 10), [45, 63, 79, 80, 122]);
    assert_eq!(lan_context(0, 10), 84);
}
