//! Voice segments follow public capture and routing interfaces.

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use proptest::prelude::*;
    use tehuti_core::{
        State,
        bar::{Segment, Tone},
        plugin::{self, Input, RequestKey},
        speech::{SpeechEffect, SpeechFailure, SpeechInput, SpeechState},
        step, step_with_plugin,
    };
    use tehuti_protocol::{Action, Event, Permission, Text, parse_context};

    fn focus(state: State, app_id: Option<String>) -> State {
        step(
            state,
            &Event::Focus {
                app_id,
                title: String::new(),
            },
        )
        .0
    }

    fn chip_with_agents(app_id: &str, title: &str, agents: Vec<String>) -> String {
        let state = step(
            State::default().with_bar_agents(agents),
            &Event::Focus {
                app_id: Some(app_id.into()),
                title: title.into(),
            },
        )
        .0;
        state.bar().segments()[0].text.clone()
    }

    fn chip_after_focus(app_id: &str, title: &str) -> String {
        chip_with_agents(app_id, title, vec!["Claude".into(), "Codex".into()])
    }

    #[test]
    fn app_level_shows_the_full_title_beside_the_context() {
        let state = focus_titled(
            labelled(&["app_id = 'editor'\nlabel = 'Editor'\nbuttons = {}"]),
            "editor",
            "notes.txt - Editor",
        );
        let texts: Vec<_> = state
            .bar()
            .segments()
            .into_iter()
            .map(|segment| segment.text)
            .collect();
        assert_eq!(texts, ["Editor", "notes.txt - Editor"]);
        let state = focus_titled(state, "editor", "draft.txt - Editor");
        assert_eq!(state.bar().segments()[1].text, "draft.txt - Editor");
        let state = focus_titled(state, "editor", "");
        let fallback_texts: Vec<_> = state
            .bar()
            .segments()
            .into_iter()
            .map(|segment| segment.text)
            .collect();
        assert_eq!(fallback_texts, ["Editor"]);
    }

    #[test]
    fn only_configured_agents_override_the_app_id() {
        assert_eq!(chip_with_agents("kitty", "Claude: project", vec![]), "kitty");
        assert_eq!(
            chip_with_agents("kitty", "Helper", vec!["Helper".into()]),
            "Helper"
        );
        assert_eq!(
            chip_with_agents("kitty", "Helper: project", vec!["Helper".into()]),
            "project"
        );
        assert_eq!(
            chip_with_agents("kitty", ": project", vec![String::new()]),
            "kitty"
        );
    }

    #[test]
    fn malformed_agent_titles_keep_the_app_id() {
        for title in ["Claude: ", "Claude:   ", "Claude:project", "Claudeish: project"] {
            assert_eq!(chip_after_focus("kitty", title), "kitty");
        }
    }

    #[test]
    fn agent_windows_show_the_agent_name() {
        assert_eq!(chip_after_focus("kitty", "Claude"), "Claude");
        assert_eq!(chip_after_focus("kitty", "Codex"), "Codex");
    }

    #[test]
    fn work_on_windows_show_the_project_name() {
        assert_eq!(chip_after_focus("kitty", "Claude: atlas"), "atlas");
        assert_eq!(
            chip_after_focus("kitty", "Codex: comment-review"),
            "comment-review"
        );
    }

    fn labelled(contexts: &[&str]) -> State {
        State::with_contexts(
            contexts
                .iter()
                .map(|source| parse_context(source).unwrap())
                .collect(),
        )
        .with_bar_agents(vec!["Claude".into()])
    }

    fn focus_titled(state: State, app_id: &str, title: &str) -> State {
        step(
            state,
            &Event::Focus {
                app_id: Some(app_id.into()),
                title: title.into(),
            },
        )
        .0
    }

    #[test]
    fn a_context_label_names_the_focused_window() {
        let state = labelled(&["app_id = 'mgba-qt'\nlabel = 'mGBA'\nbuttons = {}"]);
        let mgba = focus_titled(
            state.clone(),
            "mgba-qt",
            "mGBA - Pokemon - Red Version (USA, Europe)",
        );
        assert_eq!(mgba.bar().segments()[0].text, "mGBA");
        let other = focus_titled(state.clone(), "kitty", "~/Projects");
        assert_eq!(other.bar().segments()[0].text, "kitty");
        let agent = focus_titled(state, "kitty", "Claude: atlas");
        assert_eq!(agent.bar().segments()[0].text, "atlas");
    }

    #[test]
    fn agent_titles_win_over_a_label() {
        let state = labelled(&["app_id = 'kitty'\nlabel = 'Terminal'\nbuttons = {}"]);
        assert_eq!(
            focus_titled(state.clone(), "kitty", "Claude: atlas")
                .bar()
                .segments()[0]
                .text,
            "atlas"
        );
        assert_eq!(
            focus_titled(state, "kitty", "~/Projects").bar().segments()[0].text,
            "Terminal"
        );
    }

    #[test]
    fn reloading_contexts_relabels_the_focused_window() {
        let mut state = focus_titled(labelled(&[]), "mgba-qt", "mGBA - 0.10.1");
        assert_eq!(state.bar().segments()[0].text, "mgba-qt");
        state.replace_contexts(vec![
            parse_context("app_id = 'mgba-qt'\nlabel = 'mGBA'\nbuttons = {}").unwrap(),
        ]);
        assert_eq!(state.bar().segments()[0].text, "mGBA");
        state.replace_contexts(Vec::new());
        assert_eq!(state.bar().segments()[0].text, "mgba-qt");
    }

    #[test]
    fn a_desktop_entry_name_labels_an_app_without_a_context_label() {
        let mut state = labelled(&["app_id = 'org.example.Viewer'\nlabel = 'Pictures'\nbuttons = {}"]);
        state.set_app_names(vec![
            ("org.example.Editor".into(), "Editor".into()),
            ("org.example.Viewer".into(), "Image Viewer".into()),
        ]);
        assert_eq!(state.label(Some("org.example.Editor")), Some("Editor"));
        assert_eq!(state.label(Some("org.example.Viewer")), Some("Pictures"));
        assert_eq!(state.label(Some("org.example.Other")), None);
        let editor = focus_titled(state.clone(), "org.example.Editor", "notes.txt");
        assert_eq!(editor.bar().segments()[0].text, "Editor");
        let viewer = focus_titled(state.clone(), "org.example.Viewer", "photo.png");
        assert_eq!(viewer.bar().segments()[0].text, "Pictures");
        let other = focus_titled(state, "org.example.Other", "");
        assert_eq!(other.bar().segments()[0].text, "org.example.Other");
    }

    #[test]
    fn new_desktop_names_relabel_the_focused_window() {
        let mut state = focus_titled(labelled(&[]), "org.example.Editor", "notes.txt");
        state.set_app_names(vec![("org.example.Editor".into(), "Editor".into())]);
        assert_eq!(state.bar().segments()[0].text, "Editor");
        state.set_app_names(Vec::new());
        assert_eq!(state.bar().segments()[0].text, "org.example.Editor");
    }

    #[test]
    fn the_first_context_for_an_app_decides_its_label() {
        let state = labelled(&[
            "app_id = 'mgba-qt'\nbuttons = {}",
            "app_id = 'mgba-qt'\nlabel = 'mGBA'\nbuttons = {}",
            "app_id = '*'\nlabel = 'Everything'\nbuttons = {}",
        ]);
        assert_eq!(state.label(Some("mgba-qt")), None);
        assert_eq!(state.label(Some("*")), None);
        assert_eq!(
            focus_titled(state, "mgba-qt", "mGBA").bar().segments()[0].text,
            "mgba-qt"
        );
    }

    #[test]
    fn columns_chips_use_short_names() {
        use tehuti_core::bar::window_label;
        assert_eq!(
            window_label(Some("mGBA"), Some("mGBA - Pokemon"), Some("mgba-qt"), &[]),
            "mGBA"
        );
        assert_eq!(
            window_label(None, Some("mGBA - Pokemon"), Some("mgba-qt"), &[]),
            "mgba-qt"
        );
        assert_eq!(window_label(None, Some(""), Some("mgba-qt"), &[]), "mgba-qt");
        assert_eq!(window_label(None, None, None, &[]), "Untitled");
        let bar = State::new().with_bar_agents(vec!["Helper".into()]).bar().clone();
        assert_eq!(
            bar.window_label(Some("Terminal"), Some("Helper: atlas"), Some("terminal")),
            "atlas"
        );
        assert_eq!(bar.window_label(None, Some("Helper"), Some("terminal")), "Helper");
    }

    proptest! {
        #[test]
        fn configured_agent_titles_show_the_project(agent in "[A-Z][a-z]{1,12}", project in "[a-z]{1,12}") {
            let title = format!("{agent}: {project}");
            prop_assert_eq!(chip_with_agents("kitty", &title, vec![agent]), project);
        }

        #[test]
        fn other_titles_keep_the_app_id(app_id in "[a-z]{1,8}", title in "[a-z ~/]{0,12}") {
            prop_assert_eq!(chip_after_focus(&app_id, &title), app_id);
        }
    }

    fn feedback(state: &State) -> Option<Segment> {
        let segments = state.bar().segments();
        segments
            .into_iter()
            .find(|segment| matches!(segment.tone, Tone::Live | Tone::Result | Tone::Error))
    }

    fn segment(text: &str, tone: Tone) -> Segment {
        Segment {
            text: text.into(),
            tone,
        }
    }

    fn capture(state: &mut State, speech: &mut SpeechState, input: SpeechInput) -> Vec<SpeechEffect> {
        let (next, effects) = state.capture(core::mem::take(speech), input);
        *speech = next;
        effects
    }

    fn listening(state: &mut State) -> SpeechState {
        let mut speech = SpeechState::default();
        drop(capture(state, &mut speech, SpeechInput::Available));
        drop(capture(state, &mut speech, SpeechInput::Press));
        speech
    }

    fn pending(state: State, request_id: u64) -> (State, RequestKey) {
        let event = Event::Speech {
            text: Text::new("hello".into()).unwrap(),
        };
        let (state, _, routed) = step_with_plugin(state, &event, true);
        assert!(routed, "speech reaches the plugin");
        let key = RequestKey {
            plugin: 0,
            request_id,
        };
        let (state, _, _) = plugin::step(
            state,
            Input::Request {
                key,
                event,
                permissions: vec![Permission::Text],
            },
        );
        (state, key)
    }

    proptest! {
        #[test]
        fn capture_shows_live_text_without_delivering_it_and_rejects_stale_previews(
            app_id in "[a-zA-Z]{1,30}",
            text in any::<Text>(),
            stale_session in 1u64..u64::MAX,
        ) {
            let mut state = focus(State::new(), Some(app_id.clone()));
            prop_assert_eq!(state.bar().segments().first().cloned(), Some(Segment { text: app_id, tone: Tone::Context }));
            let mut speech = listening(&mut state);
            prop_assert_eq!(feedback(&state), Some(segment("Listening…", Tone::Live)));
            let preview = capture(&mut state, &mut speech, SpeechInput::Interim { session: 0, text: text.clone() });
            prop_assert_eq!(preview, vec![SpeechEffect::Interim { session: 0, text: text.clone() }]);
            prop_assert_eq!(feedback(&state), Some(segment(text.as_str(), Tone::Live)));
            let before = state.bar().clone();
            let rejected = capture(&mut state, &mut speech, SpeechInput::Interim { session: stale_session, text });
            prop_assert!(rejected.is_empty());
            prop_assert_eq!(state.bar(), &before);
            state = step(state, &Event::Tick { elapsed: Duration::MAX }).0;
            prop_assert_eq!(state.bar(), &before);
            drop(capture(&mut state, &mut speech, SpeechInput::Release));
            prop_assert_eq!(feedback(&state), Some(segment("Transcribing…", Tone::Live)));
            let after_release = state.bar().clone();
            let late = capture(&mut state, &mut speech, SpeechInput::Interim { session: 0, text: Text::new("late".into()).unwrap() });
            prop_assert!(late.is_empty());
            prop_assert_eq!(state.bar(), &after_release);
        }

        #[test]
        fn generated_contexts_show_mapping_results_then_clear_at_three_seconds(
            app_id in "[a-zA-Z]{1,30}",
            phrase in "[a-z]{1,30}",
            mapping in 0..3u8,
            millis in 0..3000u64,
        ) {
            let (mapping_source, label, tone) = match mapping {
                0 => ("text = 'hello'", "typed", Tone::Result),
                1 => ("launch = 'org.example.App.desktop'", "org.example.App.desktop", Tone::Result),
                _ => ("notify = 'Done'", "Done", Tone::Result),
            };
            let context = parse_context(&format!("app_id = '{app_id}'\n[buttons]\n[speech]\n'{phrase}' = {{ {mapping_source} }}")).unwrap();
            let state = focus(State::with_contexts(vec![context]), Some(app_id));
            let (mut state, actions) = step(state, &Event::Speech { text: Text::new(phrase).unwrap() });
            prop_assert_eq!(actions.len(), 1);
            prop_assert_eq!(feedback(&state), Some(segment(label, tone)));
            state = step(state, &Event::Tick { elapsed: Duration::from_millis(millis) }).0;
            prop_assert_eq!(feedback(&state), Some(segment(label, tone)));
            state = step(state, &Event::Tick { elapsed: Duration::from_millis(3000u64.saturating_sub(millis)) }).0;
            prop_assert_eq!(feedback(&state), None);
        }
    }

    #[test]
    fn admitted_final_resolves_once_and_a_new_press_replaces_the_result() {
        let context = parse_context("app_id = 'Claude'\nunmatched_speech = 'type'\n[buttons]").unwrap();
        let mut state = focus(State::with_contexts(vec![context]), Some("Claude".into()));
        let mut speech = listening(&mut state);
        drop(capture(&mut state, &mut speech, SpeechInput::Release));
        let final_input = SpeechInput::Transcript {
            session: 0,
            text: Text::new("Hello".into()).unwrap(),
        };
        let effects = capture(&mut state, &mut speech, final_input.clone());
        assert_eq!(
            effects,
            vec![SpeechEffect::Speech {
                session: 0,
                text: Text::new("Hello".into()).unwrap()
            }]
        );
        let (next, actions) = step(
            state,
            &Event::Speech {
                text: Text::new("Hello".into()).unwrap(),
            },
        );
        state = next;
        assert_eq!(
            actions,
            vec![Action::Text {
                text: Text::new("Hello ".into()).unwrap()
            }]
        );
        assert_eq!(feedback(&state), Some(segment("typed", Tone::Result)));
        assert!(
            capture(&mut state, &mut speech, final_input).is_empty(),
            "finals are one-use"
        );
        drop(capture(&mut state, &mut speech, SpeechInput::Press));
        assert_eq!(feedback(&state), Some(segment("Listening…", Tone::Live)));
    }

    #[test]
    fn unmatched_and_empty_speech_have_expiring_errors() {
        for (text, label) in [("hello", "No speech mapping"), (" ", "No speech")] {
            let (state, _) = step(
                State::new(),
                &Event::Speech {
                    text: Text::new(text.into()).unwrap(),
                },
            );
            assert_eq!(feedback(&state), Some(segment(label, Tone::Error)));
            let (state, _) = step(
                state,
                &Event::Tick {
                    elapsed: Duration::MAX,
                },
            );
            assert_eq!(feedback(&state), None);
        }
    }

    #[test]
    fn failures_and_unavailable_capture_show_errors() {
        for reason in [SpeechFailure::Silent, SpeechFailure::Transcription] {
            let mut state = State::new();
            let mut speech = listening(&mut state);
            drop(capture(&mut state, &mut speech, SpeechInput::Release));
            drop(capture(
                &mut state,
                &mut speech,
                SpeechInput::Failed { session: 0, reason },
            ));
            let label = if reason == SpeechFailure::Silent {
                "Mic silent"
            } else {
                "Speech failed"
            };
            assert_eq!(feedback(&state), Some(segment(label, Tone::Error)));
            assert_eq!(
                feedback(
                    &step(
                        state,
                        &Event::Tick {
                            elapsed: Duration::MAX
                        }
                    )
                    .0
                ),
                None
            );
        }
        let mut state = State::new();
        let mut speech = listening(&mut state);
        drop(capture(&mut state, &mut speech, SpeechInput::Unavailable));
        assert_eq!(feedback(&state), Some(segment("Speech unavailable", Tone::Error)));
    }

    #[test]
    fn cancellation_and_focus_changes_clear_feedback_and_stale_finals() {
        for disruption in [SpeechInput::Cancel, SpeechInput::Disconnected] {
            let mut state = State::new();
            let mut speech = listening(&mut state);
            drop(capture(&mut state, &mut speech, disruption));
            assert_eq!(feedback(&state), None);
            let effects = capture(
                &mut state,
                &mut speech,
                SpeechInput::Transcript {
                    session: 0,
                    text: Text::new("old".into()).unwrap(),
                },
            );
            assert!(effects.is_empty(), "cancelled final cannot reach routing");
            assert_eq!(feedback(&state), None);
        }
        let (state, _) = pending(State::new(), 0);
        let state = focus(state, None);
        assert_eq!(
            state.bar().segments(),
            vec![Segment {
                text: "Desktop".into(),
                tone: Tone::Context
            }]
        );
    }

    #[test]
    fn plugin_reply_and_fallback_resolve_only_the_current_utterance() {
        let (state, key) = pending(State::new(), 0);
        assert_eq!(feedback(&state), Some(segment("Resolving…", Tone::Live)));
        let reply = Input::Reply {
            key,
            result: Ok(vec![Action::Text {
                text: Text::new("hello".into()).unwrap(),
            }]),
        };
        let (resolved, _, _) = plugin::step(state.clone(), reply.clone());
        assert_eq!(feedback(&resolved), Some(segment("typed", Tone::Result)));
        for outcome in [
            Input::Tick {
                elapsed: Duration::from_secs(2),
            },
            Input::Exited { plugin: 0 },
            Input::Reply {
                key,
                result: Ok(Vec::new()),
            },
        ] {
            let (fallback, _, _) = plugin::step(state.clone(), outcome);
            assert_eq!(
                feedback(&fallback),
                Some(segment("No speech mapping", Tone::Error))
            );
        }
        let (mut newer, _) = pending(state, 1);
        let mut speech = listening(&mut newer);
        drop(capture(
            &mut newer,
            &mut speech,
            SpeechInput::Interim {
                session: 0,
                text: Text::new("new".into()).unwrap(),
            },
        ));
        let (newer, _, _) = plugin::step(newer, reply);
        assert_eq!(feedback(&newer), Some(segment("new", Tone::Live)));
    }
}
