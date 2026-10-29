//! Configuration boundary properties.

#[cfg(test)]
mod context {
    use proptest::prelude::*;
    use tehuti_protocol::{KeyName, parse_context};

    #[test]
    fn window_sizes_accept_fractions_and_named_presets() {
        let plain = parse_context("app_id = 'org.example.App'\nbuttons = {}").unwrap();
        assert_eq!(plain.window_column_width(), None);
        assert_eq!(plain.window_height(), None);
        for (value, expected) in [
            ("0.5", 0.5),
            ("1", 1.0),
            ("'one-third'", 0.333_333_333_333_333_3),
            ("'one-half'", 0.5),
            ("'two-thirds'", 0.666_666_666_666_666_6),
            ("'full'", 1.0),
        ] {
            let context = parse_context(&format!("app_id = 'org.example.App'\nbuttons = {{}}\n[window]\ncolumn_width = {value}\nheight = {value}")).unwrap();
            assert_eq!(context.window_column_width(), Some(expected), "{value}");
            assert_eq!(context.window_height(), Some(expected), "{value}");
        }
        for value in [
            "0",
            "-0.1",
            "1.1",
            "nan",
            "inf",
            "'half'",
            "true",
            "{ preset = 1 }",
        ] {
            for key in ["column_width", "height"] {
                assert!(
                    parse_context(&format!(
                        "app_id = 'org.example.App'\nbuttons = {{}}\n[window]\n{key} = {value}"
                    ))
                    .is_err(),
                    "{key}: {value}"
                );
            }
        }
    }

    #[test]
    fn window_fullscreen_is_optional_and_boolean() {
        let base = "app_id = 'org.example.Game'\nbuttons = {}\n";
        assert!(!parse_context(base).unwrap().open_fullscreen());
        for enabled in [false, true] {
            let context = parse_context(&format!("{base}[window]\nopen_fullscreen = {enabled}")).unwrap();
            assert_eq!(context.open_fullscreen(), enabled);
        }
        for field in ["open_fullscreen = 'true'", "unknown = true"] {
            assert!(parse_context(&format!("{base}[window]\n{field}")).is_err());
        }
    }

    #[test]
    fn favourites_preserve_order_and_default_to_empty() {
        let plain = parse_context("app_id = '*'\n[buttons]").unwrap();
        assert_eq!(plain.favourites(), Vec::<String>::new());
        let configured =
            parse_context("app_id = '*'\nfavourites = ['reader.desktop', 'browser.desktop']\n[buttons]")
                .unwrap();
        assert_eq!(configured.favourites(), ["reader.desktop", "browser.desktop"]);
        assert!(parse_context("app_id = '*'\nfavorite = ['reader.desktop']\n[buttons]").is_err());
    }

    #[test]
    fn passthrough_ptt_rejects_unsafe_or_invalid_chords() {
        for buttons in [
            "[]",
            "['start']",
            "['south']",
            "['east']",
            "['north']",
            "['west']",
            "['guide', 'select']",
            "['select', 'select']",
            "['unknown']",
        ] {
            assert!(
                parse_context(&format!(
                    "app_id = 'org.example.Emulator'\nbuttons = {{}}\n[pad]\npush_to_talk = {buttons}"
                ))
                .is_err(),
                "{buttons}"
            );
        }
        for buttons in ["['record']", "['select']", "['select', 'left_bumper']"] {
            assert!(
                parse_context(&format!(
                    "app_id = 'org.example.Emulator'\nbuttons = {{}}\n[pad]\npush_to_talk = {buttons}"
                ))
                .is_ok(),
                "{buttons}"
            );
        }
    }

    proptest::proptest! {
        #[test]
        fn pad_passthrough_is_explicit_and_boolean(enabled in proptest::bool::ANY) {
            let base = "app_id = 'org.example.Emulator'\nbuttons = {}\n";
            proptest::prop_assert!(!parse_context(base).unwrap().passthrough());
            let context = parse_context(&format!("{base}[pad]\npassthrough = {enabled}\n")).unwrap();
            proptest::prop_assert_eq!(context.passthrough(), enabled);
            proptest::prop_assert!(parse_context(&format!("{base}[pad]\npassthrough = '{enabled}'\n")).is_err(), "reject string values");
            proptest::prop_assert!(parse_context(&format!("{base}[pad]\nunknown = {enabled}\n")).is_err(), "reject unknown fields");
        }
    }

    #[test]
    fn firefox_context_maps_buttons_and_speech_to_modifier_actions() {
        use tehuti_protocol::{Action, ButtonName, Modifier};
        let context = parse_context(include_str!("../../../dist/contexts/firefox/context.toml")).unwrap();
        assert_eq!(context.app_id(), "firefox");
        for (button, key, mods) in [
            (ButtonName::East, KeyName::Left, vec![Modifier::Alt]),
            (ButtonName::North, KeyName::L, vec![Modifier::Ctrl]),
            (
                ButtonName::LeftBumper,
                KeyName::Tab,
                vec![Modifier::Ctrl, Modifier::Shift],
            ),
            (ButtonName::RightBumper, KeyName::Tab, vec![Modifier::Ctrl]),
        ] {
            assert_eq!(context.action_for(button), Some(Action::Key { key, mods }));
            assert_eq!(
                context.key_for(button),
                None,
                "a chord cannot become an unmodified key"
            );
        }
        for (phrase, key, mods) in [
            ("new tab", KeyName::T, vec![Modifier::Ctrl]),
            ("reload", KeyName::F5, vec![]),
            ("close tab", KeyName::W, vec![Modifier::Ctrl]),
        ] {
            assert_eq!(context.speech_action(phrase), Some(Action::Key { key, mods }));
        }
    }

    #[test]
    fn key_modifiers_are_optional_and_unknown_modifiers_are_rejected() {
        use tehuti_protocol::{Action, Modifier};
        let chord: Action = serde_json::from_str(r#"{"type":"key","key":"Left","mods":["alt"]}"#).unwrap();
        assert_eq!(
            chord,
            Action::Key {
                key: KeyName::Left,
                mods: vec![Modifier::Alt]
            }
        );
        let plain: Action = serde_json::from_str(r#"{"type":"key","key":"enter"}"#).unwrap();
        assert_eq!(
            plain,
            Action::Key {
                key: KeyName::Enter,
                mods: vec![]
            }
        );
        assert_eq!(
            serde_json::to_value(plain).unwrap(),
            serde_json::json!({"type":"key", "key":"enter"})
        );
        assert!(serde_json::from_str::<Action>(r#"{"type":"key","key":"tab","mods":["hyper"]}"#).is_err());
        assert!(parse_context("app_id = 'test'\n[buttons.east]\nkey = 'Left'\nmods = ['hyper']").is_err());
    }

    proptest! {
        #[test]
        fn arbitrary_configuration_returns_a_result(source in any::<String>()) {
            let _parsed = parse_context(&source);
        }

        #[test]
        fn every_named_key_parses(key in prop::sample::select(vec![
            ("enter", KeyName::Enter), ("escape", KeyName::Escape),
            ("space", KeyName::Space), ("tab", KeyName::Tab),
            ("backspace", KeyName::Backspace), ("up", KeyName::Up),
            ("down", KeyName::Down), ("left", KeyName::Left), ("right", KeyName::Right),
        ])) {
            let context = parse_context(&format!("app_id = 'test'\n[buttons.east]\nkey = '{}'", key.0)).unwrap();
            prop_assert_eq!(context.key_for(tehuti_protocol::ButtonName::East).unwrap(), key.1);
        }

        /// Whisper capitalises and punctuates between words as well as at the end.
        #[test]
        fn whisper_punctuation_and_case_do_not_block_a_phrase(
            words in prop::collection::vec("[a-z]{1,8}", 1..5),
            separators in prop::collection::vec(prop::sample::select(vec![" ", ". ", ", ", "! ", "? ", " - ", "  "]), 4),
            ending in prop::sample::select(vec!["", ".", "!", "?", "...", " ."]),
        ) {
            let mapped = words.join(" ");
            let mut spoken = String::new();
            let leading = core::iter::once("").chain(separators.iter().copied());
            for (separator, word) in leading.zip(&words) {
                spoken.push_str(separator);
                let mut letters = word.chars();
                if let Some(first) = letters.next() {
                    spoken.extend(first.to_uppercase());
                    spoken.extend(letters);
                }
            }
            spoken.push_str(ending);
            let context = parse_context(&format!("app_id = 'test'\n[buttons]\n[speech]\n\"{mapped}\" = {{ key = 'enter' }}")).unwrap();
            prop_assert!(context.speech_action(&format!(" {spoken}")).is_some(), "{spoken:?} should match {mapped:?}");
        }
    }

    #[test]
    fn transcript_with_inner_punctuation_matches() {
        let context =
            parse_context("app_id = 'foot'\n[buttons]\n[speech]\n\"Front center.\" = { key = 'enter' }")
                .unwrap();
        assert!(context.speech_action("Front. Center.").is_some());
        assert!(context.speech_action("Front centre.").is_none());
        assert!(context.speech_action("Don't.").is_none());
    }

    #[test]
    fn speech_phrases_that_normalize_alike_are_rejected() {
        let error = parse_context(
            "app_id = 'foot'\n[buttons]\n[speech]\n\"Hello\" = { key = 'enter' }\n\"hello.\" = { key = 'tab' }",
        )
        .unwrap_err();
        assert!(error.contains("normalizes to \"hello\""), "{error}");
    }

    #[test]
    fn speech_phrases_that_normalize_to_nothing_are_rejected() {
        for phrase in ["", "...", " ' ", "\u{2014}\u{ff01}"] {
            let source = format!("app_id = 'foot'\n[buttons]\n[speech]\n{phrase:?} = {{ key = 'enter' }}");
            let error = parse_context(&source).unwrap_err();
            assert!(error.contains("no words"), "{phrase:?}: {error}");
        }
    }

    #[test]
    fn empty_transcript_never_matches() {
        let context =
            parse_context("app_id = 'foot'\n[buttons]\n[speech]\n\"go\" = { key = 'enter' }").unwrap();
        for spoken in ["", "  ", "...", "\u{3002}"] {
            assert!(context.speech_action(spoken).is_none(), "{spoken:?}");
        }
    }

    #[test]
    fn unicode_apostrophes_and_punctuation_fold() {
        let context = parse_context(
            "app_id = 'foot'\n[buttons]\n[speech]\n\"don't stop\" = { key = 'enter' }\n\"caf\u{e9} ol\u{e9}\" = { key = 'tab' }",
        )
        .unwrap();
        assert!(context.speech_action("Don\u{2019}t stop.").is_some());
        assert!(
            context
                .speech_action("don\u{2bc}t \u{201c}stop\u{201d}")
                .is_some()
        );
        assert!(
            context
                .speech_action("\u{bf}Caf\u{e9}\u{2014}ol\u{e9}!")
                .is_some()
        );
        assert!(context.speech_action("dont stop").is_none());
    }

    #[test]
    fn decomposed_accents_match_composed_ones() {
        let context =
            parse_context("app_id = 'foot'\n[buttons]\n[speech]\n\"caf\u{e9}\" = { key = 'enter' }").unwrap();
        assert!(context.speech_action("Cafe\u{301}.").is_some());
        assert!(context.speech_action("cafe").is_none());
        assert_eq!(tehuti_protocol::normalize_phrase("cafe\u{301}"), "caf\u{e9}");
    }

    /// Lowercasing "J\u{30c}" yields "j\u{30c}", which only composes to "\u{1f0}" if NFC runs last.
    #[test]
    fn nfc_applies_after_lowercasing() {
        let context =
            parse_context("app_id = 'foot'\n[buttons]\n[speech]\n\"\u{1f0}\" = { key = 'enter' }").unwrap();
        assert!(context.speech_action("J\u{30c}").is_some());
        let error = parse_context(
            "app_id = 'foot'\n[buttons]\n[speech]\n\"\u{1f0}\" = { key = 'enter' }\n\"J\u{30c}\" = { key = 'tab' }",
        )
        .unwrap_err();
        assert!(error.contains("normalizes to"), "{error}");
    }

    #[test]
    fn text_rejects_nul_at_construction_and_deserialization() {
        assert!(tehuti_protocol::Text::new("a\0b".into()).is_err());
        let source = r#"app_id = 'test'
[buttons.east]
text = "a\u0000b"
"#;
        assert!(parse_context(source).is_err());
        assert!(parse_context(&source.replace("\\u0000", "\\u0001")).is_ok());
        assert_eq!(tehuti_protocol::Text::new("🦀".into()).unwrap().as_str(), "🦀");
    }

    #[test]
    fn context_can_request_a_secret_by_reference() {
        let context = parse_context(
            "app_id = 'browser'\n[buttons.east]\nsecret = { entry = 'password-store/browser' }\n",
        )
        .unwrap();

        assert!(context.key_for(tehuti_protocol::ButtonName::East).is_none());
        assert_eq!(
            context.action_for(tehuti_protocol::ButtonName::East),
            Some(tehuti_protocol::Action::SecretRequest {
                reference: tehuti_protocol::SecretReference {
                    entry: "password-store/browser".into()
                }
            }),
        );
    }

    #[test]
    fn malformed_contexts_are_rejected() {
        for source in [
            "app_id = ''\n[buttons]",
            "app_id = '  '\n[buttons]",
            "app_id = 'test'\n[buttons.unknown]\nkey = 'enter'",
            "app_id = 'test'\n[buttons.east]\nkey = 'unknown'",
            "app_id = 'test'\nextra = true\n[buttons]",
            "app_id = 'test'\n[buttons.east]\nkey = 'enter'\ntext = 'secret'",
            "app_id = 'test'\n[buttons.east]\ntext = '\0'",
            "not toml",
        ] {
            assert!(
                parse_context(source).is_err(),
                "invalid input must fail: {source}"
            );
        }
    }

    #[test]
    fn keyboard_options_reject_unknown_modes_buttons_and_fields() {
        for setting in [
            "on_text_field = 'always'",
            "button = 'unknown'",
            "button = 'guide'",
            "button = 'record'",
            "popup = true",
        ] {
            assert!(
                tehuti_protocol::parse_context(&format!(
                    "app_id = 'editor'\n[buttons]\n[keyboard]\n{setting}"
                ))
                .is_err()
            );
        }
    }

    #[test]
    fn hold_is_an_opt_in_boolean_on_plain_button_keys() {
        use tehuti_protocol::{Action, ButtonName};
        let context = parse_context(
            "app_id = 'org.example.Game'\n[buttons]\nsouth = { key = 'x', hold = true }\neast = { key = 'z', hold = false }\nnorth = { key = 'enter' }",
        )
        .unwrap();
        assert_eq!(context.held_key_for(ButtonName::South), Some(KeyName::X));
        for button in [ButtonName::East, ButtonName::North, ButtonName::West] {
            assert_eq!(context.held_key_for(button), None, "{button:?}");
        }
        assert_eq!(
            context.action_for(ButtonName::East),
            Some(Action::Key {
                key: KeyName::Z,
                mods: Vec::new()
            })
        );
    }

    #[test]
    fn hold_rejects_mods_phrases_and_non_boolean_values() {
        for source in [
            "[buttons]\nsouth = { key = 'x', hold = true, mods = ['ctrl'] }",
            "[buttons]\nsouth = { key = 'x', hold = 'yes' }",
            "[buttons]\nsouth = { text = 'x', hold = true }",
            "[buttons]\n[speech]\n'jump' = { key = 'x', hold = true }",
        ] {
            assert!(
                parse_context(&format!("app_id = 'org.example.Game'\n{source}")).is_err(),
                "{source}"
            );
        }
    }

    #[test]
    fn a_speech_icon_is_optional_and_keeps_the_action() {
        use tehuti_protocol::{Action, Modifier};
        let context = parse_context(
            "app_id = 'org.example.Game'\n[buttons]\n[speech]\n\"Lagre spillet.\" = { key = 'f1', mods = ['shift'], icon = '\u{1f4be}' }\n\"Pause.\" = { key = 'p', mods = ['ctrl'] }",
        )
        .unwrap();
        assert_eq!(
            context.speech_action("lagre spillet"),
            Some(Action::Key {
                key: KeyName::F1,
                mods: vec![Modifier::Shift]
            })
        );
        let mut mappings: Vec<_> = context
            .speech_mappings()
            .map(|mapping| (mapping.phrase, mapping.icon))
            .collect();
        mappings.sort_unstable();
        assert_eq!(
            mappings,
            [("Lagre spillet.", Some("\u{1f4be}")), ("Pause.", None)],
            "phrases keep the file's spelling"
        );
    }

    #[test]
    fn a_speech_entry_rejects_unknown_fields_and_bad_icons_beside_an_icon() {
        for entry in [
            "{ key = 'enter', icon = 'x', extra = 1 }",
            "{ icon = 'x' }",
            "{ key = 'enter', icon = 1 }",
            "{ key = 'enter', icon = '\0' }",
            "{ key = 'enter', hold = true, icon = 'x' }",
        ] {
            assert!(
                parse_context(&format!("app_id = 'test'\n[buttons]\n[speech]\n'go' = {entry}")).is_err(),
                "{entry}"
            );
        }
    }

    #[test]
    fn ticker_options_are_optional_and_strict() {
        use tehuti_protocol::{TickerConfig, TickerSpeech};
        let base = "app_id = 'test'\n[buttons]\n";
        assert_eq!(parse_context(base).unwrap().ticker(), &TickerConfig::default());
        let context = parse_context(&format!("{base}[ticker]\nenabled = true\nspeech = 'icons'")).unwrap();
        assert_eq!(
            context.ticker(),
            &TickerConfig {
                enabled: Some(true),
                speech: Some(TickerSpeech::Icons),
                ..TickerConfig::default()
            }
        );
        for setting in ["enabled = 'yes'", "speech = 'some'", "speed = 2"] {
            assert!(
                parse_context(&format!("{base}[ticker]\n{setting}")).is_err(),
                "{setting}"
            );
        }
    }

    #[test]
    fn ticker_names_parse_and_look_up_keys_and_chords() {
        use tehuti_protocol::{KeyName, Modifier};
        let context = parse_context(
            "app_id = 'test'\n[buttons]\n[ticker.names]\nenter = 'Jump'\n'shift+ctrl+s' = 'Save'\nEscape = 'Pause'",
        )
        .unwrap();
        assert_eq!(context.ticker_name(KeyName::Enter, &[]), Some("Jump"));
        assert_eq!(context.ticker_name(KeyName::Escape, &[]), Some("Pause"));
        assert_eq!(
            context.ticker_name(KeyName::S, &[Modifier::Ctrl, Modifier::Shift]),
            Some("Save")
        );
        assert_eq!(context.ticker_name(KeyName::S, &[Modifier::Ctrl]), None);
        assert_eq!(context.ticker_name(KeyName::Up, &[]), None);
        let none = parse_context("app_id = 'test'\n[buttons]\n").unwrap();
        assert_eq!(none.ticker_name(KeyName::Enter, &[]), None);
    }

    #[test]
    fn ticker_names_reject_bad_input() {
        let long = "x".repeat(25);
        let maximum_length = "x".repeat(24);
        let base = "app_id = 'test'\n[buttons]\n[ticker.names]\n";
        assert!(parse_context(&format!("{base}enter = '{maximum_length}'")).is_ok());
        for bad in [
            "enter = ''".to_owned(),
            "enter = '   '".to_owned(),
            "enter = ' Jump'".to_owned(),
            format!("enter = '{long}'"),
            "enter = \"a\\nb\"".to_owned(),
            "banana = 'Jump'".to_owned(),
            "'hyper+s' = 'Save'".to_owned(),
            "'ctrl+' = 'Save'".to_owned(),
            "enter = 3".to_owned(),
            "enter = 'A'\n'Enter' = 'B'".to_owned(),
        ] {
            assert!(parse_context(&format!("{base}{bad}")).is_err(), "{bad}");
        }
        let control_error = parse_context(&format!("{base}enter = \"a\\nb\"")).unwrap_err();
        assert!(control_error.contains("control characters"), "{control_error}");
        let error = parse_context(&format!("{base}banana = 'Jump'")).unwrap_err();
        assert!(error.contains("banana"), "{error}");
    }

    #[test]
    fn a_button_or_phrase_can_toggle_the_ticker() {
        use tehuti_protocol::{Action, ButtonName};
        let context = parse_context(
            "app_id = 'test'\n[buttons]\nright_stick = { ticker = 'toggle' }\n[speech]\n'ticker' = { ticker = 'toggle' }",
        )
        .unwrap();
        assert_eq!(
            context.action_for(ButtonName::RightStick),
            Some(Action::ToggleTicker)
        );
        assert_eq!(context.speech_action("Ticker."), Some(Action::ToggleTicker));
        assert!(parse_context("app_id = 'test'\n[buttons]\neast = { ticker = 'show' }").is_err());
    }

    #[test]
    fn a_context_may_name_its_windows() {
        let context = parse_context("app_id = 'mgba-qt'\nlabel = ' mGBA '\nbuttons = {}").unwrap();
        assert_eq!(context.label(), Some("mGBA"));
        let unlabelled = parse_context("app_id = 'mgba-qt'\nbuttons = {}").unwrap();
        assert_eq!(unlabelled.label(), None);
        assert!(parse_context("app_id = 'mgba-qt'\nlabel = '  '\nbuttons = {}").is_err());
        assert!(parse_context("app_id = 'mgba-qt'\nlabel = \"a\\u0000b\"\nbuttons = {}").is_err());
        assert!(parse_context("app_id = 'mgba-qt'\nlabel = 3\nbuttons = {}").is_err());
    }
}
