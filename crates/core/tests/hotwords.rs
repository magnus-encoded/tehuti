#[cfg(test)]
mod hotwords {
    use tehuti_core::{State, step};
    use tehuti_protocol::{Action, Event, Text, parse_context};

    fn launch(entry: &str) -> Action {
        Action::Launch {
            desktop_entry: Text::new(entry.into()).unwrap(),
        }
    }

    fn focused(contexts: &[&str]) -> State {
        let mut state = State::with_contexts(
            contexts
                .iter()
                .map(|source| parse_context(source).unwrap())
                .collect(),
        );
        state.set_app_hotwords(vec![
            ("Atlas".into(), launch("atlas.desktop")),
            ("Web Browser".into(), launch("firefox.desktop")),
        ]);
        step(
            state,
            &Event::Focus {
                app_id: Some("editor".into()),
                title: String::new(),
            },
        )
        .0
    }

    #[test]
    fn context_speech_mapping_beats_an_installed_app_name() {
        let mut state =
            focused(&["app_id = 'editor'\n[buttons]\n[speech]\n'open settings' = { key = 'tab' }"]);
        state.set_app_hotwords(vec![("Settings".into(), launch("settings.desktop"))]);
        let (_, actions) = step(
            state,
            &Event::Speech {
                text: Text::new("Open Settings!".into()).unwrap(),
            },
        );
        assert_eq!(
            actions,
            vec![Action::Key {
                key: tehuti_protocol::KeyName::Tab
            }]
        );
    }

    #[test]
    fn local_hotwords_override_global_and_installed_apps_and_global_fills_gaps() {
        let state = focused(&[
            "app_id = '*'\n[buttons]\n[hotwords]\natlas = { app = 'global.desktop' }\nnotes = { app = 'notes.desktop' }",
            "app_id = 'editor'\n[buttons]\n[hotwords]\natlas = { app = 'local.desktop' }",
        ]);
        assert_eq!(state.hotwords().get("atlas"), Some(&launch("local.desktop")));
        assert_eq!(state.hotwords().get("notes"), Some(&launch("notes.desktop")));
        for (heard, expected) in [
            ("Open ATLAS!", "local.desktop"),
            ("notes", "notes.desktop"),
            ("open web browser.", "firefox.desktop"),
        ] {
            let (_, actions) = step(
                state.clone(),
                &Event::Speech {
                    text: Text::new(heard.into()).unwrap(),
                },
            );
            assert_eq!(actions, vec![launch(expected)], "{heard}");
        }
        let (unfocused, _) = step(
            state,
            &Event::Focus {
                app_id: None,
                title: String::new(),
            },
        );
        assert_eq!(unfocused.hotwords().get("atlas"), Some(&launch("global.desktop")));
    }

    #[test]
    fn app_set_can_be_disabled_globally_and_reenabled_locally() {
        let global = "app_id = '*'\n[buttons]\n[hotwords]\napps = false\natlas = { app = 'custom.desktop' }";
        let state = focused(&[global]);
        assert_eq!(state.hotwords().len(), 1);
        assert_eq!(state.hotwords().get("atlas"), Some(&launch("custom.desktop")));
        let enabled = focused(&[global, "app_id = 'editor'\n[buttons]\n[hotwords]\napps = true"]);
        assert!(enabled.hotwords().contains_key("web browser"));
        let disabled = focused(&["app_id = 'editor'\n[buttons]\n[hotwords]\napps = false"]);
        assert!(disabled.hotwords().is_empty());
    }
}
