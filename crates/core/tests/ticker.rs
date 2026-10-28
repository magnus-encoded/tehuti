//! The mapping ticker lists what the focused context maps and scrolls only while shown.

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use proptest::prelude::*;
    use tehuti_core::{
        Level, State, step,
        ticker::{self, PadFamily},
    };
    use tehuti_protocol::{Context, Event, parse_context};

    const GLOBAL: &str = r#"
app_id = "*"
push_to_talk = ["record", "start"]

[ticker]
enabled = true

[buttons]
east = { key = "escape" }
left_bumper = { layout = "focus-column-left" }

[speech]
"Lavere lyd." = { launch = "tehuti-volume-down.desktop" }
"Skru ned lyden." = { launch = "tehuti-volume-down.desktop" }
"Lukk." = { close = "focused" }
"Pause." = { key = "p", mods = ["ctrl"] }
"#;

    const GAME: &str = r#"
app_id = "org.example.Game"

[buttons]
south = { key = "x", hold = true }
dpad_up = { key = "up", hold = true }
right_stick = { ticker = "toggle" }

[speech]
"Lagre spillet." = { key = "f1", mods = ["shift"], icon = "💾" }
"Lagre." = { key = "f1", mods = ["shift"], icon = "💾" }
"Pause." = { key = "p", mods = ["ctrl"], icon = "⏸" }
"Last spillet." = { key = "f1" }
"#;

    fn contexts(sources: &[&str]) -> Vec<Context> {
        sources
            .iter()
            .map(|source| parse_context(source).unwrap())
            .collect()
    }

    fn texts(entries: &[ticker::Entry]) -> Vec<String> {
        entries.iter().map(|entry| entry.text(|_| true)).collect()
    }

    fn focus(state: State, app_id: &str) -> State {
        step(
            state,
            &Event::Focus {
                app_id: Some(app_id.into()),
                title: String::new(),
            },
        )
        .0
    }

    fn tick(state: State, millis: u64) -> State {
        step(
            state,
            &Event::Tick {
                elapsed: Duration::from_millis(millis),
            },
        )
        .0
    }

    #[test]
    fn nintendo_ticker_places_default_enter_on_a_and_keeps_configured_positions() {
        let contexts = contexts(&["app_id = 'editor'\n[buttons]\nsouth = { key = 'x' }"]);
        assert_eq!(
            texts(&ticker::entries(
                &contexts,
                Some("editor"),
                PadFamily::Nintendo,
                Level::App
            )),
            ["B -> [X]", "A -> [⏎]", "Capture -> 🎤"]
        );
    }

    #[test]
    fn buttons_use_the_pads_names_and_fall_back_to_the_global_context() {
        let contexts = contexts(&[GLOBAL, GAME]);
        assert_eq!(
            texts(&ticker::entries(
                &contexts,
                Some("org.example.Game"),
                PadFamily::Symbols,
                Level::App
            )),
            [
                "Cross -> [X] hold",
                "Circle -> [Esc]",
                "Up -> [\u{2191}] hold",
                "L1 -> Column left",
                "R3 -> \u{1f4dc}",
                "Options -> \u{1f3a4}",
                "Record -> \u{1f3a4}",
                "\"Lagre\" / \"Lagre spillet\" -> \u{1f4be}",
                "\"Lavere lyd\" / \"Skru ned lyden\" -> \u{1f509}",
                "\"Lukk\" -> \u{274c}",
                "\"Pause\" -> \u{23f8}",
                "\"Last spillet\" -> [F1]",
            ]
        );
        let lettered = ticker::entries(
            &contexts,
            Some("org.example.Game"),
            PadFamily::Letters,
            Level::App,
        );
        assert_eq!(lettered[0].text(|_| true), "A -> [X] hold");
        assert_eq!(lettered[1].text(|_| true), "B -> [Esc]");
    }

    #[test]
    fn buttons_are_symbols_in_the_pads_colours_and_keys_are_keycaps() {
        use tehuti_protocol::ButtonName;
        use ticker::{Direction, Piece, Shape};
        let symbolic = PadFamily::Symbols;
        let Piece::Glyph { shape, colour } = symbolic.piece(ButtonName::South) else {
            panic!("Cross is a symbol");
        };
        assert_eq!(shape, Shape::Cross, "South on a symbolic pad is the cross");
        assert!(colour.2 > colour.0, "the cross is blue: {colour:?}");
        let Piece::Glyph {
            shape: letter_shape,
            colour: green,
        } = PadFamily::Letters.piece(ButtonName::South)
        else {
            panic!("A is a symbol");
        };
        assert_eq!(
            letter_shape,
            Shape::Disc('A'),
            "South on an lettered pad is a lettered disc"
        );
        assert!(green.1 > green.0 && green.1 > green.2, "A is green: {green:?}");
        assert!(
            matches!(
                symbolic.piece(ButtonName::DpadLeft),
                Piece::Glyph {
                    shape: Shape::Dpad(Direction::Left),
                    ..
                }
            ),
            "the D-pad is drawn"
        );
        assert_eq!(
            symbolic.piece(ButtonName::LeftBumper),
            Piece::Chip("L1".into()),
            "bumpers are chips"
        );
        assert_eq!(
            ticker::action_pieces(&tehuti_protocol::Action::Key {
                key: tehuti_protocol::KeyName::O,
                mods: vec![tehuti_protocol::Modifier::Ctrl],
            }),
            [
                Piece::Key("Ctrl".into()),
                Piece::Text("+".into()),
                Piece::Key("O".into())
            ],
            "a chord is keycaps joined by +"
        );
        assert_eq!(
            ticker::key_cap(tehuti_protocol::KeyName::Enter),
            "\u{23ce}",
            "Enter is a return symbol"
        );
        assert_eq!(
            ticker::key_cap(tehuti_protocol::KeyName::Backspace),
            "\u{232b}",
            "Backspace is its symbol"
        );
    }

    #[test]
    fn south_shows_the_global_enter_when_nothing_maps_it() {
        let contexts = contexts(&[GLOBAL]);
        let entries = ticker::entries(
            &contexts,
            Some("org.example.Terminal"),
            PadFamily::Letters,
            Level::App,
        );
        assert_eq!(entries[0].text(|_| true), "A -> [\u{23ce}]");
    }

    #[test]
    fn columns_level_shows_bumpers_moving_focus() {
        let contexts = contexts(&[GLOBAL, GAME]);
        let entries = texts(&ticker::entries(
            &contexts,
            Some("org.example.Game"),
            PadFamily::Letters,
            Level::Columns,
        ));
        assert!(entries.contains(&"RB -> Column right".to_owned()), "{entries:?}");
    }

    #[test]
    fn an_icon_the_renderer_cannot_draw_falls_back_to_its_word() {
        let contexts = contexts(&[GLOBAL, GAME]);
        let entries = ticker::entries(
            &contexts,
            Some("org.example.Game"),
            PadFamily::Letters,
            Level::App,
        );
        let words: Vec<String> = entries.iter().map(|entry| entry.text(|_| false)).collect();
        assert!(
            words.contains(&"\"Lagre\" / \"Lagre spillet\" -> Save".to_owned()),
            "{words:?}"
        );
        assert!(
            words.contains(&"\"Lavere lyd\" / \"Skru ned lyden\" -> Vol-".to_owned()),
            "{words:?}"
        );
        assert!(
            words.contains(&"\"Last spillet\" -> [F1]".to_owned()),
            "{words:?}"
        );
    }

    #[test]
    fn an_unknown_icon_falls_back_to_the_action() {
        let contexts = contexts(&[
            "app_id = 'a'\n[ticker]\nenabled = true\n[buttons]\n[speech]\n'Hopp' = { key = 'space', icon = '🦘' }",
        ]);
        let entries = ticker::entries(&contexts, Some("a"), PadFamily::Letters, Level::App);
        assert_eq!(entries.last().unwrap().text(|_| false), "\"Hopp\" -> [Space]");
        assert_eq!(entries.last().unwrap().text(|_| true), "\"Hopp\" -> \u{1f998}");
    }

    #[test]
    fn icons_mode_lists_only_phrases_with_icons() {
        let game = format!("{GAME}\n[ticker]\nspeech = 'icons'\n");
        let contexts = contexts(&[GLOBAL, &game]);
        let entries = texts(&ticker::entries(
            &contexts,
            Some("org.example.Game"),
            PadFamily::Letters,
            Level::App,
        ));
        assert!(
            !entries.iter().any(|entry| entry.contains("Last spillet")),
            "{entries:?}"
        );
        assert!(entries.iter().any(|entry| entry.contains("Lagre")), "{entries:?}");
    }

    #[test]
    fn passthrough_says_the_pad_belongs_to_the_application() {
        let contexts = contexts(&[
            GLOBAL,
            "app_id = 'game'\n[buttons]\n[pad]\npassthrough = true\npush_to_talk = ['select', 'left_bumper']",
        ]);
        let entries = texts(&ticker::entries(
            &contexts,
            Some("game"),
            PadFamily::Letters,
            Level::App,
        ));
        assert_eq!(entries[..2], ["Pad -> app", "Hold View + LB -> \u{1f3a4}"]);
    }

    #[test]
    fn columns_level_lists_buttons_for_a_passthrough_context() {
        let contexts = contexts(&[GLOBAL, "app_id = 'game'\n[buttons]\n[pad]\npassthrough = true"]);
        let entries = texts(&ticker::entries(
            &contexts,
            Some("game"),
            PadFamily::Letters,
            Level::Columns,
        ));
        assert!(!entries.iter().any(|entry| entry == "Pad -> app"), "{entries:?}");
    }

    #[test]
    fn the_ticker_is_off_unless_a_context_turns_it_on() {
        let off = State::with_contexts(contexts(&["app_id = '*'\n[buttons]\neast = { key = 'escape' }"]));
        assert!(off.bar().ticker().is_none(), "off by default");
        let global = State::with_contexts(contexts(&[
            GLOBAL,
            "app_id = 'quiet'\n[buttons]\n[ticker]\nenabled = false",
        ]));
        assert!(
            global.bar().ticker().is_some(),
            "the desktop uses the global choice"
        );
        assert!(
            focus(global, "quiet").bar().ticker().is_none(),
            "a context overrides the global choice"
        );
    }

    #[test]
    fn focus_and_reload_replace_the_entries() {
        let state = focus(
            State::with_contexts(contexts(&[GLOBAL, GAME])),
            "org.example.Game",
        );
        let first = state.bar().ticker().unwrap().entries;
        assert_eq!(first[0].text(|_| true), "A -> [X] hold");
        let mut state = focus(state, "org.example.Terminal");
        assert_eq!(
            state.bar().ticker().unwrap().entries[0].text(|_| true),
            "A -> [\u{23ce}]"
        );
        state.replace_contexts(contexts(&[
            &GLOBAL.replace("east = { key = \"escape\" }", "south = { key = \"space\" }")
        ]));
        assert_eq!(
            state.bar().ticker().unwrap().entries[0].text(|_| true),
            "A -> [Space]"
        );
        state.set_pad_family(PadFamily::Symbols);
        assert_eq!(
            state.bar().ticker().unwrap().entries[0].text(|_| true),
            "Cross -> [Space]"
        );
    }

    const NAMED_GAME: &str = r#"
app_id = "game"

[buttons]
south = { key = "enter", hold = true }
west = { key = "backspace" }
north = { key = "s", mods = ["ctrl"] }
east = { key = "escape" }
dpad_up = { key = "up" }

[ticker.names]
enter = "Jump"
backspace = "Action"
"ctrl+s" = "Save"
"#;

    #[test]
    fn named_keys_show_as_text_and_others_keep_their_keycap() {
        let contexts = contexts(&[GLOBAL, NAMED_GAME]);
        let entries = ticker::entries(&contexts, Some("game"), PadFamily::Letters, Level::App);
        let texts = texts(&entries);
        assert_eq!(texts[0], "A -> Jump hold");
        assert!(texts.contains(&"X -> Action".to_owned()), "{texts:?}");
        assert!(texts.contains(&"Y -> Save".to_owned()), "{texts:?}");
        assert!(texts.contains(&"B -> [Esc]".to_owned()), "{texts:?}");
        assert!(texts.contains(&"Up -> [\u{2191}]".to_owned()), "{texts:?}");
        let jump = &entries[0].result;
        assert_eq!(jump[0], ticker::Piece::Text("Jump".into()));
        assert_eq!(jump[1], ticker::Piece::Dim("hold".into()));
        assert!(
            ticker::entries(
                &contexts,
                Some("org.example.Terminal"),
                PadFamily::Letters,
                Level::App
            )
            .iter()
            .all(|entry| !entry.text(|_| true).contains("Jump"))
        );
    }

    #[test]
    fn names_do_not_touch_speech_and_reload_updates_them() {
        let with_speech = NAMED_GAME.replace(
            "[ticker.names]",
            "[speech]\n\"Hopp.\" = { key = \"enter\" }\n[ticker.names]",
        );
        let mut state = focus(State::with_contexts(contexts(&[GLOBAL, &with_speech])), "game");
        let texts_now = texts(&state.bar().ticker().unwrap().entries);
        assert!(
            texts_now.contains(&"\"Hopp\" -> [\u{23ce}]".to_owned()),
            "{texts_now:?}"
        );
        state.replace_contexts(contexts(&[GLOBAL, &with_speech.replace("\"Jump\"", "\"Leap\"")]));
        assert_eq!(
            state.bar().ticker().unwrap().entries[0].text(|_| true),
            "A -> Leap hold"
        );
    }

    #[test]
    fn scroll_steps_advance_four_times_a_second_and_restart_on_focus() {
        let state = State::with_contexts(contexts(&[GLOBAL, GAME]));
        let state = tick(state, 249);
        assert_eq!(state.bar().ticker().unwrap().step, 0, "under one step");
        let state = tick(state, 1);
        assert_eq!(state.bar().ticker().unwrap().step, 1);
        let state = tick(state, 750);
        assert_eq!(state.bar().ticker().unwrap().step, 4);
        let state = focus(state, "org.example.Game");
        assert_eq!(
            state.bar().ticker().unwrap().step,
            0,
            "new entries start from the beginning"
        );
    }

    #[test]
    fn a_toggle_hides_the_ticker_across_focus_and_stops_its_clock() {
        let mut state = focus(
            State::with_contexts(contexts(&[GLOBAL, GAME])),
            "org.example.Game",
        );
        state.toggle_ticker();
        let state = tick(state, 1000);
        assert!(state.bar().ticker().is_none());
        let mut state = focus(state, "org.example.Terminal");
        assert!(state.bar().ticker().is_none(), "hiding outlasts focus changes");
        state.toggle_ticker();
        assert_eq!(
            state.bar().ticker().unwrap().step,
            0,
            "hidden time does not scroll"
        );
    }

    #[test]
    fn a_disabled_ticker_leaves_the_bar_unchanged_by_time() {
        let state = State::with_contexts(contexts(&["app_id = '*'\n[buttons]"]));
        let before = state.bar().clone();
        assert_eq!(tick(state, 10_000).bar(), &before, "nothing to redraw");
    }

    #[test]
    fn text_that_fits_stays_still() {
        for step in [0, 1, 99] {
            assert_eq!(ticker::scroll_offset(step, 100, 100, 6, 48), 0);
        }
    }

    #[test]
    fn longer_text_moves_one_step_width_per_step() {
        assert_eq!(ticker::scroll_offset(0, 500, 100, 6, 48), 0);
        assert_eq!(ticker::scroll_offset(1, 500, 100, 6, 48), 6);
        assert_eq!(ticker::scroll_offset(10, 500, 100, 6, 48), 60);
    }

    proptest! {
        #[test]
        fn the_offset_stays_inside_one_repeat(
            step in any::<u64>(),
            content in 1..10_000usize,
            viewport in 1..2_000usize,
            pixels in 1..16usize,
            gap in 0..200usize,
        ) {
            let offset = ticker::scroll_offset(step, content, viewport, pixels, gap);
            prop_assert!(offset < content.saturating_add(gap), "{offset} wraps after {content} + {gap}");
        }

        #[test]
        fn the_offset_repeats_after_one_period_of_steps(
            step in 0..1_000_000u64,
            content in 200..5_000usize,
            pixels in 1..16usize,
            gap in 0..200usize,
        ) {
            let period = u64::try_from(content.saturating_add(gap)).unwrap();
            prop_assert_eq!(
                ticker::scroll_offset(step, content, 100, pixels, gap),
                ticker::scroll_offset(step.saturating_add(period), content, 100, pixels, gap)
            );
        }
    }
}
