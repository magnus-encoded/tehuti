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
    keyboard.left_stick(x, 0);
    keyboard.left_stick(0, 0);
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
            keyboard.left_stick(x, 0);
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
fn shift_toggles_capitals_until_pressed_again() {
    let mut keyboard = opened();
    press(&mut keyboard, ButtonName::South);
    go_to(&mut keyboard, 0, 0);
    press(&mut keyboard, ButtonName::South);
    go_to(&mut keyboard, 0, 2);
    press(&mut keyboard, ButtonName::South);
    press(&mut keyboard, ButtonName::South);
    assert!(keyboard.shifted());
    go_to(&mut keyboard, 0, 0);
    press(&mut keyboard, ButtonName::South);
    assert!(!keyboard.shifted());
    go_to(&mut keyboard, 0, 2);
    press(&mut keyboard, ButtonName::South);
    assert_eq!(keyboard.draft(), "qWWw");
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
        keyboard.left_stick(-900, 0);
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
fn menu_sends_the_draft_and_stays_open() {
    let mut keyboard = with_draft("hello there");
    assert_eq!(
        keyboard.button(ButtonName::Start, ButtonState::Pressed, true),
        Response::Consumed,
        "Menu sends on release, so a Guide press can still join the escape chord"
    );
    assert_eq!(
        keyboard.button(ButtonName::Start, ButtonState::Released, true),
        Response::Chord(
            "hello there".into(),
            Chord {
                key: ChordKey::Named(KeyName::Enter),
                modifiers: Modifiers::default()
            }
        )
    );
    assert!(keyboard.is_open());
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
    assert!(keyboard.is_open());
}

#[test]
fn sending_an_empty_draft_keeps_the_keyboard_open() {
    let mut keyboard = opened();
    let _pressed = keyboard.button(ButtonName::Start, ButtonState::Pressed, true);
    assert_eq!(
        keyboard.button(ButtonName::Start, ButtonState::Released, true),
        Response::Chord(
            String::new(),
            Chord {
                key: ChordKey::Named(KeyName::Enter),
                modifiers: Modifiers::default()
            }
        )
    );
    assert!(keyboard.is_open());
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

#[test]
fn left_stick_click_switches_between_letters_and_symbols() {
    let mut keyboard = opened();
    press(&mut keyboard, ButtonName::LeftStick);
    assert!(keyboard.symbols());
    press(&mut keyboard, ButtonName::South);
    press(&mut keyboard, ButtonName::LeftStick);
    assert!(!keyboard.symbols());
    press(&mut keyboard, ButtonName::South);
    assert_eq!(keyboard.draft(), "1q");
}

proptest! {
    #[test]
    fn symbols_preserve_the_letter_layer(capitals in any::<bool>(), trips in 1usize..8) {
        let mut keyboard = opened();
        if capitals {
            go_to(&mut keyboard, 0, 0);
            press(&mut keyboard, ButtonName::South);
        }
        go_to(&mut keyboard, 0, 1);
        for _ in 0..trips {
            press(&mut keyboard, ButtonName::LeftStick);
            assert!(keyboard.symbols());
            assert!(!keyboard.shifted());
            press(&mut keyboard, ButtonName::South);
            press(&mut keyboard, ButtonName::LeftStick);
            assert!(!keyboard.symbols());
            assert_eq!(keyboard.shifted(), capitals);
            press(&mut keyboard, ButtonName::South);
        }
        prop_assert_eq!(keyboard.draft(), (if capitals { "1Q" } else { "1q" }).repeat(trips));
    }
}

#[test]
fn b_resets_capitals_even_when_symbols_are_showing() {
    for symbols in [false, true] {
        let mut keyboard = opened();
        go_to(&mut keyboard, 0, 0);
        press(&mut keyboard, ButtonName::South);
        if symbols {
            press(&mut keyboard, ButtonName::LeftStick);
        }
        press(&mut keyboard, ButtonName::East);
        assert!(!keyboard.shifted());
        assert!(!keyboard.symbols());
        keyboard.open();
        assert!(!keyboard.shifted());
        press(&mut keyboard, ButtonName::South);
        assert_eq!(keyboard.draft(), "q");
        press(&mut keyboard, ButtonName::LeftStick);
        press(&mut keyboard, ButtonName::LeftStick);
        assert!(!keyboard.shifted());
    }
}

#[test]
fn field_kinds_choose_base_layouts_and_keep_symbols_available() {
    for (kind, special) in [
        (FieldKind::Normal, Key::Char(',')),
        (FieldKind::Url, Key::Text(".com")),
        (FieldKind::Email, Key::Char('@')),
        (FieldKind::Password, Key::Char(',')),
    ] {
        let mut keyboard = Keyboard::default();
        keyboard.set_field_kind(kind, true);
        keyboard.open();
        assert!(keyboard.rows()[2].iter().any(|slot| slot.key == special));
        assert_eq!(keyboard.password_field_active(), kind == FieldKind::Password);
        for row in keyboard.rows() {
            assert_eq!(row.iter().map(|slot| u16::from(slot.width)).sum::<u16>(), 28);
        }
        if kind == FieldKind::Url {
            assert!(keyboard.rows()[2].iter().any(|slot| slot.key == Key::Char('/')));
            go_to(&mut keyboard, 2, 9);
            press(&mut keyboard, ButtonName::South);
            assert_eq!(keyboard.draft(), ".com");
        }
        press(&mut keyboard, ButtonName::LeftStick);
        assert!(keyboard.symbols());
    }
}

#[test]
fn password_reveals_only_last_typed_character_for_one_and_a_half_seconds() {
    let mut keyboard = Keyboard::default();
    keyboard.set_field_kind(FieldKind::Password, true);
    keyboard.open();
    press(&mut keyboard, ButtonName::South);
    press(&mut keyboard, ButtonName::DpadRight);
    press(&mut keyboard, ButtonName::South);
    assert_eq!(keyboard.displayed_draft(), "*w");
    keyboard.tick(Duration::from_millis(1499));
    assert_eq!(keyboard.displayed_draft(), "*w");
    keyboard.tick(Duration::from_millis(1));
    assert_eq!(keyboard.displayed_draft(), "**");
    press(&mut keyboard, ButtonName::LeftBumper);
    press(&mut keyboard, ButtonName::South);
    assert_eq!(keyboard.displayed_draft(), "*w*");
    press(&mut keyboard, ButtonName::West);
    assert_eq!(keyboard.displayed_draft(), "**");
    keyboard.dictate("ignored");
    assert_eq!(keyboard.draft(), "qw");
    assert_eq!(tap(&mut keyboard, ButtonName::Start), Response::Consumed);
    assert!(keyboard.is_open());
    assert!(keyboard.password_field_active());
}

#[test]
fn changing_fields_discards_password_and_clears_reveal() {
    let mut keyboard = opened();
    keyboard.set_field_kind(FieldKind::Password, true);
    press(&mut keyboard, ButtonName::South);
    keyboard.set_field_kind(FieldKind::Normal, true);
    assert_eq!(keyboard.displayed_draft(), "");
    assert!(!keyboard.password_field_active());
}

fn terminal() -> Keyboard {
    let mut keyboard = Keyboard::default();
    keyboard.set_terminal(true);
    keyboard.open();
    keyboard
}

#[test]
fn terminals_add_a_strip_above_the_normal_letters() {
    let keyboard = terminal();
    let rows = keyboard.rows();
    assert_eq!(rows.get(1..4), Some(opened().rows()));
    let strip: Vec<Key> = rows[0].iter().map(|slot| slot.key).collect();
    for key in [
        KeyName::Escape,
        KeyName::Tab,
        KeyName::Left,
        KeyName::Up,
        KeyName::Down,
        KeyName::Right,
        KeyName::PageUp,
        KeyName::PageDown,
    ] {
        assert!(strip.contains(&Key::Tap(key)), "{key:?} on the strip");
    }
    assert!(strip.contains(&Key::Modifier(Modifier::Control)));
    assert!(strip.contains(&Key::Modifier(Modifier::Alt)));
    assert_eq!(keyboard.highlighted_key(), Some(Key::Char('q')));
}

#[test]
fn shell_symbols_live_on_the_terminal_base_layer() {
    let keyboard = terminal();
    let keys: Vec<Key> = keyboard
        .rows()
        .iter()
        .flat_map(|row| row.iter().map(|slot| slot.key))
        .collect();
    for symbol in ['|', '&', '>', '<', ';', '$', '~', '`', '-', '_', '/', '*'] {
        assert!(keys.contains(&Key::Char(symbol)), "{symbol} reachable");
    }
}

#[test]
fn tilde_is_on_the_terminal_base_row_without_a_duplicate_on_the_strip() {
    let keyboard = terminal();
    let rows = keyboard.rows();
    assert!(!rows[0].iter().any(|slot| slot.key == Key::Char('~')));
    assert!(rows[4].iter().any(|slot| slot.key == Key::Char('~')));
}

#[test]
fn every_terminal_row_spans_the_same_width() {
    let mut keyboard = terminal();
    for _ in 0..2 {
        for row in keyboard.rows() {
            assert_eq!(row.iter().map(|slot| slot.width).sum::<u8>(), ROW_UNITS);
        }
        press(&mut keyboard, ButtonName::LeftStick);
    }
}

#[test]
fn a_terminal_field_picks_the_terminal_layer() {
    let mut keyboard = opened();
    keyboard.set_field_kind(FieldKind::Terminal, true);
    assert_eq!(keyboard.rows(), terminal().rows());
    keyboard.close();
    keyboard.open();
    assert_eq!(keyboard.highlighted_key(), Some(Key::Char('q')));
}

#[test]
fn b_in_a_terminal_closes_and_discards_the_draft() {
    let mut keyboard = terminal();
    keyboard.dictate("make");
    assert_eq!(tap(&mut keyboard, ButtonName::East), Response::Consumed);
    assert!(!keyboard.is_open());
    assert_eq!(keyboard.draft(), "");
}

#[test]
fn start_in_a_terminal_runs_the_draft_and_stays_open() {
    let mut keyboard = terminal();
    keyboard.dictate("ls");
    let _press = keyboard.button(ButtonName::Start, ButtonState::Pressed, true);
    assert_eq!(
        keyboard.button(ButtonName::Start, ButtonState::Released, true),
        Response::Chord(
            "ls".into(),
            Chord {
                key: ChordKey::Named(KeyName::Enter),
                modifiers: Modifiers::default(),
            }
        )
    );
    assert!(keyboard.is_open());
    assert_eq!(keyboard.draft(), "");
}

fn strip_key(keyboard: &mut Keyboard, key: Key) -> Response {
    let index = keyboard.rows()[0]
        .iter()
        .position(|slot| slot.key == key)
        .unwrap();
    go_to(keyboard, 0, index);
    tap(keyboard, ButtonName::South)
}

#[test]
fn immediate_terminal_keys_leave_the_draft_unchanged() {
    let immediate_keys = [
        KeyName::Escape,
        KeyName::Tab,
        KeyName::Left,
        KeyName::Up,
        KeyName::Down,
        KeyName::Right,
        KeyName::PageUp,
        KeyName::PageDown,
    ];
    for key in immediate_keys {
        let mut keyboard = terminal();
        keyboard.dictate("cd src");
        assert_eq!(
            strip_key(&mut keyboard, Key::Tap(key)),
            Response::Chord(
                String::new(),
                Chord {
                    key: ChordKey::Named(key),
                    modifiers: Modifiers::default()
                }
            )
        );
        assert!(keyboard.is_open());
        assert_eq!(keyboard.draft(), "cd src");
        assert_eq!(keyboard.caret(), 6);
    }
    for character in ['c', 'd', 'z'] {
        let mut keyboard = terminal();
        keyboard.dictate("cd src");
        let _latch = strip_key(&mut keyboard, Key::Modifier(Modifier::Control));
        let (row, index) = keyboard
            .rows()
            .iter()
            .enumerate()
            .find_map(|(row, keys)| {
                keys.iter()
                    .position(|slot| slot.key == Key::Char(character))
                    .map(|index| (row, index))
            })
            .unwrap();
        go_to(&mut keyboard, row, index);
        assert_eq!(
            tap(&mut keyboard, ButtonName::South),
            Response::Chord(
                String::new(),
                Chord {
                    key: ChordKey::Char(character),
                    modifiers: Modifiers {
                        control: true,
                        alt: false
                    }
                }
            )
        );
        assert_eq!(keyboard.draft(), "cd src");
        assert_eq!(keyboard.caret(), 6);
        assert_eq!(keyboard.latched(), Modifiers::default());
    }
}

#[test]
fn ctrl_latches_for_one_letter_then_releases() {
    let mut keyboard = terminal();
    assert_eq!(
        strip_key(&mut keyboard, Key::Modifier(Modifier::Control)),
        Response::Consumed
    );
    assert!(keyboard.latched().control);
    go_to(&mut keyboard, 3, 3);
    assert_eq!(
        tap(&mut keyboard, ButtonName::South),
        Response::Chord(String::new(), Chord::INTERRUPT)
    );
    assert_eq!(keyboard.latched(), Modifiers::default());
    press(&mut keyboard, ButtonName::South);
    assert_eq!(keyboard.draft(), "c");
}

#[test]
fn alt_latches_onto_the_next_strip_key() {
    let mut keyboard = terminal();
    let _latch = strip_key(&mut keyboard, Key::Modifier(Modifier::Alt));
    assert_eq!(
        strip_key(&mut keyboard, Key::Tap(KeyName::Left)),
        Response::Chord(
            String::new(),
            Chord {
                key: ChordKey::Named(KeyName::Left),
                modifiers: Modifiers {
                    control: false,
                    alt: true
                }
            }
        )
    );
}

proptest! {
    #[test]
    fn dictation_keeps_punctuation_in_every_application(text in "[a-z]{1,6}[.,]( [a-z]{1,6}[.,]){0,3}", terminal_app in any::<bool>()) {
        let mut keyboard = opened();
        keyboard.set_terminal(terminal_app);
        keyboard.dictate(&text);
        prop_assert_eq!(keyboard.draft(), text.as_str());
    }
}

#[test]
fn line_moves_stop_at_line_ends_and_draft_edges() {
    let mut keyboard = with_draft("longer\né\nlast");
    keyboard.left_stick(0, 1000);
    assert_eq!(keyboard.caret(), 14);
    keyboard.left_stick(0, 0);
    keyboard.left_stick(0, -1000);
    assert_eq!(keyboard.caret(), 9);
    for _ in 0..5 {
        keyboard.left_stick(0, -1000);
    }
    assert_eq!(keyboard.caret(), 9, "a held stick moves only one line");
    keyboard.left_stick(0, 0);
    keyboard.left_stick(0, -1000);
    assert_eq!(keyboard.caret(), 1);
    keyboard.left_stick(0, 0);
    keyboard.left_stick(0, -1000);
    assert_eq!(keyboard.caret(), 1);
}

#[test]
fn line_moves_cross_empty_lines_and_clear_a_picked_word() {
    let mut keyboard = with_draft("one\n\nthree");
    flick(&mut keyboard, -1000);
    assert_eq!(keyboard.picked_word(), Some(5..10));
    keyboard.left_stick(0, -1000);
    assert_eq!(keyboard.caret(), 4);
    assert_eq!(keyboard.picked_word(), None);
    keyboard.left_stick(0, 0);
    keyboard.left_stick(0, -1000);
    assert_eq!(keyboard.caret(), 0);
}

#[test]
fn vertical_stick_leaves_a_single_line_draft_unchanged() {
    let mut keyboard = with_draft("one two");
    for y in [-1000, 1000] {
        keyboard.left_stick(0, y);
        keyboard.left_stick(0, 0);
        assert_eq!(keyboard.caret(), 7);
        assert_eq!(keyboard.picked_word(), None);
        assert_eq!(keyboard.draft(), "one two");
    }
}

proptest! {
    #[test]
    fn dpad_moves_only_the_highlight(moves in prop::collection::vec(dpad(), 1..40), picked in any::<bool>()) {
        let mut keyboard = with_draft("one two three");
        if picked {
            flick(&mut keyboard, -1000);
        }
        let caret = keyboard.caret();
        let word = keyboard.picked_word();
        for name in moves {
            press(&mut keyboard, name);
            keyboard.tick(REPEAT_DELAY);
            prop_assert_eq!(keyboard.caret(), caret);
            prop_assert_eq!(keyboard.picked_word(), word.clone());
            prop_assert_eq!(keyboard.draft(), "one two three");
        }
    }
}

#[test]
fn synced_replacement_emits_an_edit_even_when_text_is_unchanged() {
    let mut keyboard = opened();
    keyboard.set_surrounding(Some(("qéq".into(), 1, 0)));
    press(&mut keyboard, ButtonName::South);
    assert_eq!(keyboard.draft(), "qéq");
    assert_eq!(
        keyboard.take_edits(),
        vec![Edit {
            before: 0,
            after: 0,
            text: "q".into(),
            caret: 1,
        }]
    );
    assert_eq!(keyboard.take_edits(), vec![]);
}
