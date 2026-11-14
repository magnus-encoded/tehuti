use core::time::Duration;
use tehuti_core::tap_hold::TapHold;

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

#[test]
fn a_hold_expires_once_and_releases_keys_before_modifiers() {
    let mut taps = TapHold::default();
    assert_eq!(taps.tap(ms(0), 1, &[2]), [(2, true), (1, true)]);
    assert!(taps.tick(ms(39)).is_empty());
    assert_eq!(taps.tick(ms(40)), [(1, false), (2, false)]);
    assert!(taps.tick(ms(100)).is_empty());
}

#[test]
fn repeats_restart_the_gap_then_hold_from_the_actual_press() {
    let mut taps = TapHold::default();
    assert_eq!(taps.tap(ms(0), 1, &[]), [(1, true)]);
    assert_eq!(taps.tap(ms(5), 1, &[]), [(1, false)]);
    assert!(taps.tap(ms(10), 1, &[]).is_empty());
    assert!(taps.tick(ms(26)).is_empty());
    assert_eq!(taps.tick(ms(30)), [(1, true)]);
    assert!(taps.tick(ms(69)).is_empty());
    assert_eq!(taps.tick(ms(70)), [(1, false)]);
}

#[test]
fn shared_modifiers_wait_for_the_later_chord() {
    let mut taps = TapHold::default();
    assert_eq!(taps.tap(ms(0), 1, &[3]), [(3, true), (1, true)]);
    assert_eq!(taps.tap(ms(30), 2, &[3]), [(2, true)]);
    assert_eq!(taps.tick(ms(40)), [(1, false)]);
    assert_eq!(taps.tick(ms(70)), [(2, false), (3, false)]);
}

#[test]
fn focus_flush_releases_old_holds_and_keeps_new_holds() {
    let mut taps = TapHold::default();
    assert_eq!(taps.tap(ms(0), 1, &[3]), [(3, true), (1, true)]);
    taps.focus_changed();
    assert_eq!(taps.tap(ms(5), 2, &[4]), [(4, true), (2, true)]);
    assert_eq!(taps.flush_focus(), [(1, false), (3, false)]);
    assert!(taps.flush_focus().is_empty());
    assert_eq!(taps.tick(ms(45)), [(2, false), (4, false)]);
}

#[test]
fn focus_flush_cancels_deferred_repeats_without_a_second_release() {
    let mut taps = TapHold::default();
    assert_eq!(taps.tap(ms(0), 1, &[]), [(1, true)]);
    assert_eq!(taps.tap(ms(5), 1, &[]), [(1, false)]);
    taps.focus_changed();
    assert!(taps.flush_focus().is_empty());
    assert!(taps.tick(ms(100)).is_empty());
}

#[test]
fn failed_timer_releases_every_key_and_allows_another_timer() {
    let mut taps = TapHold::default();
    assert_eq!(taps.tap(ms(0), 1, &[2]), [(2, true), (1, true)]);
    assert_eq!(taps.take_release_request(ms(5)), Some(ms(35)));
    assert_eq!(taps.take_release_request(ms(6)), None);
    assert_eq!(taps.timer_failed(), [(1, false), (2, false)]);
    assert_eq!(taps.timer_fired(ms(40)), (vec![], None));
    assert_eq!(taps.tap(ms(50), 1, &[]), [(1, true)]);
    assert_eq!(taps.take_release_request(ms(50)), Some(ms(40)));
}

#[test]
fn early_timer_rearms_and_cancelled_gaps_never_press() {
    let mut taps = TapHold::default();
    assert_eq!(taps.tap(ms(0), 1, &[]), [(1, true)]);
    assert_eq!(taps.take_release_request(ms(0)), Some(ms(40)));
    assert_eq!(taps.timer_fired(ms(20)), (vec![], Some(ms(20))));
    assert_eq!(taps.tap(ms(21), 1, &[]), [(1, false)]);
    assert!(taps.release_all().is_empty());
    assert_eq!(taps.timer_fired(ms(100)), (vec![], None));
}

#[test]
fn instant_chords_release_modifiers_in_reverse_order() {
    let mut taps = TapHold::default();
    taps.instant_taps();
    assert_eq!(
        taps.tap(ms(0), 1, &[2, 3]),
        [
            (2, true),
            (3, true),
            (1, true),
            (1, false),
            (3, false),
            (2, false)
        ]
    );
    assert_eq!(taps.take_release_request(ms(0)), None);
}
