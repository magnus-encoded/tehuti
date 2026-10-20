use super::*;
use proptest::prelude::*;

fn opened() -> Keyboard {
    let mut keyboard = Keyboard::default();
    keyboard.open();
    keyboard
}

fn tap(keyboard: &mut Keyboard, name: ButtonName) -> Response {
    let response = keyboard.button(name, ButtonState::Pressed, true);
    let _release = keyboard.button(name, ButtonState::Released, true);
    response
}

fn press(keyboard: &mut Keyboard, name: ButtonName) {
    let _response = tap(keyboard, name);
}

fn with_draft(text: &str) -> Keyboard {
    let mut keyboard = opened();
    keyboard.dictate(text);
    keyboard
}

fn go_to(keyboard: &mut Keyboard, row: usize, index: usize) {
    while keyboard.highlight().0 != row {
        press(keyboard, ButtonName::DpadDown);
    }
    while keyboard.highlight().1 != index {
        press(keyboard, ButtonName::DpadRight);
    }
}

fn flick(keyboard: &mut Keyboard, x: i16) {
    keyboard.left_stick(x);
    keyboard.left_stick(0);
}

const ALL_BUTTONS: [ButtonName; 16] = [
    ButtonName::South,
    ButtonName::East,
    ButtonName::North,
    ButtonName::West,
    ButtonName::LeftBumper,
    ButtonName::RightBumper,
    ButtonName::Select,
    ButtonName::Record,
    ButtonName::Start,
    ButtonName::Guide,
    ButtonName::LeftStick,
    ButtonName::RightStick,
    ButtonName::DpadUp,
    ButtonName::DpadDown,
    ButtonName::DpadLeft,
    ButtonName::DpadRight,
];

fn button_events() -> impl Strategy<Value = Vec<(ButtonName, bool)>> {
    prop::collection::vec(
        (prop::sample::select(ALL_BUTTONS.to_vec()), prop::bool::ANY),
        0..60,
    )
}

fn dpad() -> impl Strategy<Value = ButtonName> {
    prop::sample::select(vec![
        ButtonName::DpadUp,
        ButtonName::DpadDown,
        ButtonName::DpadLeft,
        ButtonName::DpadRight,
    ])
}

#[test]
fn opening_starts_with_an_empty_draft() {
    let keyboard = opened();
    assert!(keyboard.is_open());
    assert_eq!(keyboard.draft(), "");
    assert_eq!(keyboard.highlighted_key(), Some(Key::Char('q')));
}

#[test]
fn every_row_spans_the_same_width_on_both_layers() {
    for rows in [opened().rows(), {
        let mut keyboard = opened();
        go_to(&mut keyboard, 1, 0);
        press(&mut keyboard, ButtonName::South);
        keyboard.rows()
    }] {
        for row in rows {
            assert_eq!(row.iter().map(|slot| slot.width).sum::<u8>(), ROW_UNITS);
        }
    }
}

proptest! {
    #[test]
    fn highlight_always_lands_on_a_key(moves in prop::collection::vec(dpad(), 0..80)) {
        let mut keyboard = opened();
        for name in moves {
            press(&mut keyboard, name);
            prop_assert!(keyboard.highlighted_key().is_some());
        }
    }

    #[test]
    fn a_full_lap_of_a_row_wraps_back_to_the_start(row in 0usize..3, start in 0usize..11, right in prop::bool::ANY) {
        let mut keyboard = opened();
        go_to(&mut keyboard, row, start);
        let length = keyboard.rows()[row].len();
        let name = if right { ButtonName::DpadRight } else { ButtonName::DpadLeft };
        for _ in 0..length {
            press(&mut keyboard, name);
        }
        prop_assert_eq!(keyboard.highlight(), (row, start));
    }

    #[test]
    fn three_vertical_moves_return_to_the_same_row(index in 0usize..11, upward in prop::bool::ANY) {
        let mut keyboard = opened();
        go_to(&mut keyboard, 0, index);
        let name = if upward { ButtonName::DpadUp } else { ButtonName::DpadDown };
        for _ in 0..3 {
            press(&mut keyboard, name);
        }
        prop_assert_eq!(keyboard.highlight().0, 0);
    }

    #[test]
    fn sticks_never_move_the_highlight(samples in prop::collection::vec(any::<i16>(), 0..40)) {
        let mut keyboard = with_draft("one two three");
        let before = keyboard.highlight();
        for x in samples {
            keyboard.left_stick(x);
        }
        prop_assert_eq!(keyboard.highlight(), before);
    }

    #[test]
    fn a_closed_keyboard_passes_every_button(events in button_events()) {
        let mut keyboard = Keyboard::default();
        for (name, pressed) in events {
            let state = if pressed { ButtonState::Pressed } else { ButtonState::Released };
            prop_assert_eq!(keyboard.button(name, state, true), Response::Pass);
        }
    }

    #[test]
    fn guide_push_to_talk_and_view_always_pass(events in button_events(), name in prop::sample::select(vec![ButtonName::Guide, ButtonName::Record, ButtonName::Select])) {
        let mut keyboard = opened();
        for (other, pressed) in events {
            let state = if pressed { ButtonState::Pressed } else { ButtonState::Released };
            let _response = keyboard.button(other, state, true);
            keyboard.open();
            prop_assert_eq!(keyboard.button(name, ButtonState::Pressed, true), Response::Pass);
            prop_assert_eq!(keyboard.button(name, ButtonState::Released, true), Response::Pass);
        }
    }

    #[test]
    fn a_release_is_consumed_exactly_when_its_press_was(events in button_events()) {
        let mut keyboard = opened();
        let mut consumed = std::collections::HashSet::new();
        for (name, pressed) in events {
            if pressed {
                if consumed.contains(&name) {
                    continue;
                }
                if keyboard.button(name, ButtonState::Pressed, true) != Response::Pass {
                    let _held = consumed.insert(name);
                }
            } else {
                let response = keyboard.button(name, ButtonState::Released, true);
                prop_assert_eq!(response != Response::Pass, consumed.remove(&name));
            }
        }
    }

    #[test]
    fn the_caret_stays_on_a_character_boundary(text in "[a-zé ]{0,20}", moves in prop::collection::vec(prop::sample::select(vec![ButtonName::LeftBumper, ButtonName::RightBumper, ButtonName::West, ButtonName::North]), 0..40)) {
        let mut keyboard = with_draft(&text);
        for name in moves {
            press(&mut keyboard, name);
            prop_assert!(keyboard.draft().is_char_boundary(keyboard.caret()));
            prop_assert!(keyboard.caret() <= keyboard.draft().len());
        }
    }
}

#[test]
fn holding_a_direction_repeats_after_a_delay() {
    let mut keyboard = opened();
    let _pressed = keyboard.button(ButtonName::DpadRight, ButtonState::Pressed, true);
    assert_eq!(keyboard.highlight(), (0, 2));
    keyboard.tick(REPEAT_DELAY.saturating_sub(Duration::from_millis(1)));
    assert_eq!(keyboard.highlight(), (0, 2));
    keyboard.tick(Duration::from_millis(1));
    assert_eq!(keyboard.highlight(), (0, 3));
    keyboard.tick(REPEAT_INTERVAL.saturating_mul(2));
    assert_eq!(keyboard.highlight(), (0, 5));
    let _released = keyboard.button(ButtonName::DpadRight, ButtonState::Released, true);
    keyboard.tick(Duration::from_secs(1));
    assert_eq!(keyboard.highlight(), (0, 5));
}

#[test]
fn south_types_the_highlighted_key_and_shift_lasts_one_letter() {
    let mut keyboard = opened();
    press(&mut keyboard, ButtonName::South);
    go_to(&mut keyboard, 0, 0);
    press(&mut keyboard, ButtonName::South);
    go_to(&mut keyboard, 0, 2);
    press(&mut keyboard, ButtonName::South);
    press(&mut keyboard, ButtonName::South);
    assert_eq!(keyboard.draft(), "qWw");
}

#[test]
fn x_deletes_and_y_types_a_space() {
    let mut keyboard = with_draft("ab");
    press(&mut keyboard, ButtonName::West);
    press(&mut keyboard, ButtonName::North);
    assert_eq!(keyboard.draft(), "a ");
}

#[test]
fn shoulders_move_the_caret_one_character() {
    let mut keyboard = with_draft("abc");
    press(&mut keyboard, ButtonName::LeftBumper);
    press(&mut keyboard, ButtonName::LeftBumper);
    press(&mut keyboard, ButtonName::RightBumper);
    assert_eq!(keyboard.caret(), 2);
    keyboard.dictate("x");
    assert_eq!(keyboard.draft(), "ab xc");
}

#[test]
fn shoulders_pass_when_they_drive_columns() {
    let mut keyboard = with_draft("abc");
    for name in [ButtonName::LeftBumper, ButtonName::RightBumper] {
        assert_eq!(keyboard.button(name, ButtonState::Pressed, false), Response::Pass);
        assert_eq!(
            keyboard.button(name, ButtonState::Released, false),
            Response::Pass
        );
    }
    assert_eq!(keyboard.caret(), 3);
}

#[test]
fn left_stick_picks_whole_words_from_the_caret() {
    let mut keyboard = with_draft("hello big world");
    flick(&mut keyboard, -1000);
    assert_eq!(keyboard.picked_word(), Some(10..15));
    flick(&mut keyboard, -1000);
    assert_eq!(keyboard.picked_word(), Some(6..9));
    flick(&mut keyboard, 1000);
    assert_eq!(keyboard.picked_word(), Some(10..15));
}

#[test]
fn a_held_stick_picks_only_one_word() {
    let mut keyboard = with_draft("one two three");
    for _ in 0..5 {
        keyboard.left_stick(-900);
    }
    assert_eq!(keyboard.picked_word(), Some(8..13));
}

#[test]
fn dictation_overwrites_the_picked_word() {
    let mut keyboard = with_draft("hello big world");
    flick(&mut keyboard, -1000);
    flick(&mut keyboard, -1000);
    keyboard.dictate("small");
    assert_eq!(keyboard.draft(), "hello small world");
    assert_eq!(keyboard.picked_word(), None);
}

#[test]
fn typing_a_key_overwrites_the_picked_word() {
    let mut keyboard = with_draft("hello big world");
    flick(&mut keyboard, -1000);
    press(&mut keyboard, ButtonName::South);
    assert_eq!(keyboard.draft(), "hello big q");
}

#[test]
fn x_deletes_the_picked_word() {
    let mut keyboard = with_draft("hello big world");
    flick(&mut keyboard, -1000);
    flick(&mut keyboard, -1000);
    press(&mut keyboard, ButtonName::West);
    assert_eq!(keyboard.draft(), "hello  world");
    assert_eq!(keyboard.caret(), 6);
}

#[test]
fn shoulders_drop_to_a_character_caret_at_the_word_edge() {
    let mut keyboard = with_draft("hello big world");
    flick(&mut keyboard, -1000);
    flick(&mut keyboard, -1000);
    press(&mut keyboard, ButtonName::LeftBumper);
    assert_eq!((keyboard.picked_word(), keyboard.caret()), (None, 6));
    flick(&mut keyboard, 1000);
    press(&mut keyboard, ButtonName::RightBumper);
    assert_eq!((keyboard.picked_word(), keyboard.caret()), (None, 9));
}

#[test]
fn b_closes_and_discards_the_draft() {
    let mut keyboard = with_draft("secret plan");
    assert_eq!(tap(&mut keyboard, ButtonName::East), Response::Consumed);
    assert!(!keyboard.is_open());
    keyboard.open();
    assert_eq!(keyboard.draft(), "");
}

#[test]
fn menu_sends_the_draft_and_closes() {
    let mut keyboard = with_draft("hello there");
    assert_eq!(
        keyboard.button(ButtonName::Start, ButtonState::Pressed, true),
        Response::Consumed,
        "Menu sends on release, so a Guide press can still join the escape chord"
    );
    assert_eq!(
        keyboard.button(ButtonName::Start, ButtonState::Released, true),
        Response::Send("hello there".into())
    );
    assert!(!keyboard.is_open());
}

#[test]
fn guide_during_a_held_menu_cancels_the_send() {
    let mut keyboard = with_draft("hello");
    let _pressed = keyboard.button(ButtonName::Start, ButtonState::Pressed, true);
    let _guide = keyboard.button(ButtonName::Guide, ButtonState::Pressed, true);
    assert_eq!(
        keyboard.button(ButtonName::Start, ButtonState::Released, true),
        Response::Consumed
    );
    assert_eq!(keyboard.draft(), "hello");
}

#[test]
fn the_send_key_sends_the_draft() {
    let mut keyboard = with_draft("hi");
    go_to(&mut keyboard, 1, 11);
    assert_eq!(tap(&mut keyboard, ButtonName::South), Response::Send("hi".into()));
}

#[test]
fn sending_an_empty_draft_only_closes() {
    let mut keyboard = opened();
    assert_eq!(tap(&mut keyboard, ButtonName::Start), Response::Consumed);
    assert!(!keyboard.is_open());
}

#[test]
fn the_release_of_the_closing_press_stays_consumed() {
    let mut keyboard = opened();
    assert_eq!(
        keyboard.button(ButtonName::East, ButtonState::Pressed, true),
        Response::Consumed
    );
    assert_eq!(
        keyboard.button(ButtonName::East, ButtonState::Released, true),
        Response::Consumed
    );
    assert_eq!(
        keyboard.button(ButtonName::East, ButtonState::Pressed, true),
        Response::Pass
    );
}

#[test]
fn only_the_keyboard_phrase_opens_it() {
    for phrase in ["keyboard", " Keyboard. ", "KEYBOARD!"] {
        assert!(is_open_phrase(phrase), "{phrase}");
    }
    for phrase in ["keyboards", "open keyboard", "", "board"] {
        assert!(!is_open_phrase(phrase), "{phrase}");
    }
}
