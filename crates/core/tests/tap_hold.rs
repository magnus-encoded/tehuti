use core::time::Duration;
use tehuti_core::tap_hold::TapHold;

const fn millis(value: u64) -> Duration {
    Duration::from_millis(value)
}

#[test]
fn a_hold_expires_once_and_releases_keys_before_modifiers() {
    let mut taps = TapHold::default();
    assert_eq!(taps.tap(millis(0), 1, &[2]), [(2, true), (1, true)]);
    assert_eq!(taps.tick(millis(39)), Vec::<(i32, bool)>::new());
    assert_eq!(taps.tick(millis(40)), [(1, false), (2, false)]);
    assert_eq!(taps.tick(millis(100)), Vec::<(i32, bool)>::new());
}

#[test]
fn repeats_restart_the_gap_then_hold_from_the_actual_press() {
    let mut taps = TapHold::default();
    assert_eq!(taps.tap(millis(0), 1, &[]), [(1, true)]);
    assert_eq!(taps.tap(millis(5), 1, &[]), [(1, false)]);
    assert_eq!(taps.tap(millis(10), 1, &[]), Vec::<(i32, bool)>::new());
    assert_eq!(taps.tick(millis(26)), Vec::<(i32, bool)>::new());
    assert_eq!(taps.tick(millis(30)), [(1, true)]);
    assert_eq!(taps.tick(millis(69)), Vec::<(i32, bool)>::new());
    assert_eq!(taps.tick(millis(70)), [(1, false)]);
}

#[test]
fn shared_modifiers_wait_for_the_later_chord() {
    let mut taps = TapHold::default();
    assert_eq!(taps.tap(millis(0), 1, &[3]), [(3, true), (1, true)]);
    assert_eq!(taps.tap(millis(30), 2, &[3]), [(2, true)]);
    assert_eq!(taps.tick(millis(40)), [(1, false)]);
    assert_eq!(taps.tick(millis(70)), [(2, false), (3, false)]);
}

#[test]
fn focus_flush_releases_old_holds_and_keeps_new_holds() {
    let mut taps = TapHold::default();
    assert_eq!(taps.tap(millis(0), 1, &[3]), [(3, true), (1, true)]);
    taps.focus_changed();
    assert_eq!(taps.tap(millis(5), 2, &[4]), [(4, true), (2, true)]);
    assert_eq!(taps.flush_focus(), [(1, false), (3, false)]);
    assert_eq!(taps.flush_focus(), Vec::<(i32, bool)>::new());
    assert_eq!(taps.tick(millis(45)), [(2, false), (4, false)]);
}

#[test]
fn focus_flush_cancels_deferred_repeats_without_a_second_release() {
    let mut taps = TapHold::default();
    assert_eq!(taps.tap(millis(0), 1, &[]), [(1, true)]);
    assert_eq!(taps.tap(millis(5), 1, &[]), [(1, false)]);
    taps.focus_changed();
    assert_eq!(taps.flush_focus(), Vec::<(i32, bool)>::new());
    assert_eq!(taps.tick(millis(100)), Vec::<(i32, bool)>::new());
}

#[test]
fn failed_timer_releases_every_key_and_allows_another_timer() {
    let mut taps = TapHold::default();
    assert_eq!(taps.tap(millis(0), 1, &[2]), [(2, true), (1, true)]);
    assert_eq!(taps.take_release_request(millis(5)), Some(millis(35)));
    assert_eq!(taps.take_release_request(millis(6)), None);
    assert_eq!(taps.timer_failed(), [(1, false), (2, false)]);
    assert_eq!(taps.timer_fired(millis(40)), (vec![], None));
    assert_eq!(taps.tap(millis(50), 1, &[]), [(1, true)]);
    assert_eq!(taps.take_release_request(millis(50)), Some(millis(40)));
}

#[test]
fn early_timer_rearms_and_cancelled_gaps_never_press() {
    let mut taps = TapHold::default();
    assert_eq!(taps.tap(millis(0), 1, &[]), [(1, true)]);
    assert_eq!(taps.take_release_request(millis(0)), Some(millis(40)));
    assert_eq!(taps.timer_fired(millis(20)), (vec![], Some(millis(20))));
    assert_eq!(taps.tap(millis(21), 1, &[]), [(1, false)]);
    assert_eq!(taps.release_all(), Vec::<(i32, bool)>::new());
    assert_eq!(taps.timer_fired(millis(100)), (vec![], None));
}

#[test]
fn instant_chords_release_modifiers_in_reverse_order() {
    let mut taps = TapHold::default();
    taps.instant_taps();
    assert_eq!(
        taps.tap(millis(0), 1, &[2, 3]),
        [
            (2, true),
            (3, true),
            (1, true),
            (1, false),
            (3, false),
            (2, false)
        ]
    );
    assert_eq!(taps.take_release_request(millis(0)), None);
}

#[test]
fn a_deferred_retap_after_a_focus_change_presses_into_the_new_focus() {
    let mut taps = TapHold::default();
    assert_eq!(taps.tap(millis(0), 1, &[]), [(1, true)]);
    assert_eq!(taps.tap(millis(5), 1, &[]), [(1, false)]);
    taps.focus_changed();
    // The gap passes before the focus flush: the press belongs to the new focus
    // and the flush must not release it.
    assert_eq!(taps.tick(millis(30)), [(1, true)]);
    assert_eq!(taps.flush_focus(), Vec::<(i32, bool)>::new());
    assert_eq!(taps.tick(millis(70)), [(1, false)]);
}

#[test]
fn focus_flush_survives_epoch_wraparound() {
    let mut taps = TapHold::default();
    for _ in 0..u32::MAX {
        taps.focus_changed();
    }
    assert_eq!(taps.tap(millis(0), 1, &[]), [(1, true)]);
    taps.focus_changed();
    taps.focus_changed();
    assert_eq!(taps.flush_focus(), [(1, false)]);
}
