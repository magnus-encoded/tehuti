//! Laws and end-to-end dispatch tests for the global "what can I say / press" overlay.

#[cfg(test)]
mod help_overlay {
    use core::time::Duration;
    use proptest::prelude::*;
    use tehuti_core::{
        Level, State as CoreState,
        dispatch::{Effect, Input, State as DispatchState, step as dispatch_step},
        help_overlay::{GroupKind, HelpOverlay, Target},
        menu::Command,
        pad::PadFamily,
        ticker,
        tool_words::{ToolWord, WHAT_CAN_I_SAY, tool_word},
    };
    use tehuti_protocol::{
        Action, ButtonName, ButtonState, Context, Event, KeyName, Modifier, Text, parse_context,
    };

    const GLOBAL: &str = r#"
app_id = "*"
push_to_talk = ["record"]

[buttons]
east = { key = "escape" }
left_bumper = { layout = "focus-column-left" }

[speech]
"Close window." = { close = "focused" }
"Escape." = { key = "escape" }
"#;

    const EDITOR: &str = r#"
app_id = "org.example.Editor"
label = "Editor"

[buttons]
south = { key = "s", mods = ["ctrl"] }
west = { key = "f1" }

[sticks.left]
up = { key = "up" }

[speech]
"Save file." = { key = "s", mods = ["ctrl"], icon = "💾" }
"Save." = { key = "s", mods = ["ctrl"], icon = "💾" }
"Escape." = { key = "f2" }
"#;

    const PASSTHROUGH_GAME: &str = r#"
app_id = "org.example.Game"
label = "Game"

[buttons]
[pad]
passthrough = true
push_to_talk = ["select", "left_bumper"]

[ticker.buttons]
south = "Primary"

[speech]
"Pause." = { key = "p", mods = ["ctrl"] }
"#;

    fn contexts(sources: &[&str]) -> Vec<Context> {
        sources
            .iter()
            .map(|source| parse_context(source).unwrap())
            .collect()
    }

    fn speech_event(text: &str) -> Input {
        Input::Event {
            event: Event::Speech {
                text: Text::new(text.into()).unwrap(),
            },
            elapsed: Duration::ZERO,
        }
    }

    fn button_input(name: ButtonName, state: ButtonState) -> Input {
        Input::Event {
            event: Event::Button { name, state },
            elapsed: Duration::ZERO,
        }
    }

    #[derive(Default)]
    struct World {
        plugin_active: bool,
        trace: Vec<Effect>,
    }

    impl World {
        fn answer(&self, effect: &Effect) -> Option<Input> {
            match effect {
                Effect::QueryPlugin(_) => Some(Input::PluginActive(self.plugin_active)),
                Effect::DispatchPlugin { .. } => Some(Input::PluginRequest(None)),
                Effect::Supervise => Some(Input::Supervised(Vec::new())),
                Effect::ReadClock => Some(Input::ClockRead(Duration::ZERO)),
                Effect::ConfirmSecret(_) => Some(Input::SecretConfirmation(false)),
                Effect::SpeechCommand(_) => Some(Input::SpeechSent(true)),
                Effect::PollSpeech(_) => Some(Input::SpeechPolled(Vec::new())),
                Effect::Handle(_)
                | Effect::ChooseLauncher(_)
                | Effect::LauncherUsed(_)
                | Effect::OpenLauncher(_)
                | Effect::AdjusterValue { .. }
                | Effect::ReadVolume
                | Effect::ReadWindowSize(_)
                | Effect::SetWindowSize { .. }
                | Effect::SetVolume(_)
                | Effect::PasswordSpeechIgnored
                | Effect::RunMenuCommand(_)
                | Effect::SettingsMode(_)
                | Effect::PersistMode(_)
                | Effect::SettingsScale(_)
                | Effect::SettingsBarFont(_)
                | Effect::WallpaperPreview(_)
                | Effect::WallpaperSave(_)
                | Effect::UnlockWallpapers
                | Effect::SettingsController(_)
                | Effect::PersistController(_)
                | Effect::PersistControllerTalk(_, _)
                | Effect::PersistControllerDeadzone(_, _)
                | Effect::RecentreController(_)
                | Effect::SettingsMicrophone(_)
                | Effect::SettingsControllerEnabled { .. }
                | Effect::SettingsMicrophoneEnabled { .. }
                | Effect::ListMicrophones
                | Effect::ListRemoteHosts
                | Effect::ConnectRemoteMicrophone { .. }
                | Effect::MovePointer { .. }
                | Effect::Scroll { .. }
                | Effect::Click { .. }
                | Effect::MouseMode(_)
                | Effect::InvalidReply
                | Effect::PollSources
                | Effect::Chord(_)
                | Effect::ConfirmPressed(_)
                | Effect::ResetSecretFocus
                | Effect::FocusGapExpired
                | Effect::Apply(_)
                | Effect::PluginAction(_)
                | Effect::PluginRejected(_)
                | Effect::SpeechFailed { .. }
                | Effect::TranscriptReceived { .. }
                | Effect::SpeechRouted { .. } => None,
            }
        }

        fn run(&mut self, mut state: DispatchState, mut input: Input) -> DispatchState {
            for _ in 0..64 {
                let (next, effects) = dispatch_step(state, input);
                state = next;
                let mut reply = None;
                let mut follow_ups = Vec::new();
                for effect in effects {
                    if let Some(answer) = self.answer(&effect) {
                        reply = Some(answer);
                    }
                    if let Effect::Handle(event) = &effect {
                        follow_ups.push(event.clone());
                    }
                    self.trace.push(effect);
                }
                for event in follow_ups {
                    state = self.run(
                        state,
                        Input::Event {
                            event,
                            elapsed: Duration::ZERO,
                        },
                    );
                }
                if state.busy() == 0 {
                    return state;
                }
                input = reply.unwrap_or(Input::Resume);
            }
            state
        }

        fn tap(&mut self, state: DispatchState, name: ButtonName) -> DispatchState {
            let state = self.run(state, button_input(name, ButtonState::Pressed));
            self.run(state, button_input(name, ButtonState::Released))
        }
    }

    fn focused_state(sources: &[&str], app_id: Option<&str>) -> DispatchState {
        let mut world = World::default();
        let state = DispatchState::new(CoreState::with_contexts(contexts(sources)));
        match app_id {
            Some(app) => world.run(
                state,
                Input::Event {
                    event: Event::Focus {
                        app_id: Some(app.into()),
                        title: String::new(),
                    },
                    elapsed: Duration::ZERO,
                },
            ),
            None => state,
        }
    }

    #[test]
    fn groups_focused_context_and_global_layer_without_shadowed_duplicates() {
        let state = focused_state(&[GLOBAL, EDITOR], Some("org.example.Editor"));
        let overlay = state.core().help_overlay(&[]);
        assert_eq!(overlay.context_name(), Some("Editor"));
        let kinds: Vec<GroupKind> = overlay.groups().iter().map(|group| group.kind).collect();
        assert_eq!(
            kinds,
            vec![
                GroupKind::FocusedVoice,
                GroupKind::FocusedPad,
                GroupKind::GlobalVoice,
                GroupKind::GlobalPad,
            ],
            "focused context groups precede global groups"
        );
        let focused_voice = &overlay.groups()[0];
        assert!(
            focused_voice
                .items
                .iter()
                .any(|item| item.voice_name.as_deref() == Some("Save")),
            "focused voice lists shortest synonym as voice name: {focused_voice:?}"
        );
        let global_voice = &overlay.groups()[2];
        assert!(
            !global_voice
                .items
                .iter()
                .any(|item| item.voice_name.as_deref() == Some("Escape")),
            "shadowed global phrase is omitted from GlobalVoice"
        );
        assert!(
            global_voice
                .items
                .iter()
                .any(|item| item.voice_name.as_deref() == Some("Close window")),
            "unshadowed global phrase appears in GlobalVoice"
        );
    }

    #[test]
    fn focused_pad_annotates_shared_voice_names_and_includes_sticks() {
        let state = focused_state(&[GLOBAL, EDITOR], Some("org.example.Editor"));
        let overlay = state.core().help_overlay(&[]);
        let focused_pad = &overlay.groups()[1];
        let save_pad = focused_pad
            .items
            .iter()
            .find(|item| {
                item.target
                    == Target::Action(Action::Key {
                        key: KeyName::S,
                        mods: vec![Modifier::Ctrl],
                    })
            })
            .expect("South Ctrl+S entry in FocusedPad");
        assert_eq!(
            save_pad.voice_name.as_deref(),
            Some("Save"),
            "pad control shares the voice name of its action"
        );
        assert!(
            focused_pad.items.iter().any(|item| {
                item.target
                    == Target::Action(Action::Key {
                        key: KeyName::Up,
                        mods: Vec::new(),
                    })
            }),
            "stick mapping appears in FocusedPad"
        );
    }

    #[test]
    fn without_a_focused_context_only_global_groups_appear() {
        let state = focused_state(&[GLOBAL], None);
        let overlay = state.core().help_overlay(&[]);
        assert_eq!(overlay.context_name(), None);
        let kinds: Vec<GroupKind> = overlay.groups().iter().map(|group| group.kind).collect();
        assert_eq!(kinds, vec![GroupKind::GlobalVoice, GroupKind::GlobalPad]);
    }

    #[test]
    fn passthrough_hides_global_buttons_and_keeps_labels_and_talk_chord() {
        let state = focused_state(&[GLOBAL, PASSTHROUGH_GAME], Some("org.example.Game"));
        let overlay = state.core().help_overlay(&[]);
        let focused_pad = overlay
            .groups()
            .iter()
            .find(|group| group.kind == GroupKind::FocusedPad)
            .expect("passthrough game has FocusedPad");
        assert!(
            focused_pad.items.iter().any(|item| item.target == Target::None),
            "passthrough displays app ownership and ticker labels as reference-only"
        );
        let global_pad = overlay
            .groups()
            .iter()
            .find(|group| group.kind == GroupKind::GlobalPad)
            .expect("GlobalPad is present");
        assert!(
            !global_pad.items.iter().any(|item| {
                item.target
                    == Target::Action(Action::Key {
                        key: KeyName::Escape,
                        mods: Vec::new(),
                    })
            }),
            "passthrough hides inherited global button mappings"
        );
    }

    #[test]
    fn voice_phrase_opens_overlay_across_normal_invitation_and_passthrough_before_plugins() {
        for (sources, app_id, passthrough) in [
            (&[GLOBAL, EDITOR][..], Some("org.example.Editor"), false),
            (&[GLOBAL][..], None, false),
            (&[GLOBAL, PASSTHROUGH_GAME][..], Some("org.example.Game"), true),
        ] {
            let mut world = World {
                plugin_active: true,
                ..World::default()
            };
            let mut state = focused_state(sources, app_id);
            if app_id.is_none() {
                state = world.run(
                    state,
                    Input::Launcher(Box::new(tehuti_core::launcher::Launcher::invitation(None))),
                );
                assert!(state.launcher().is_some(), "invitation starts open");
            }
            world.trace.clear();
            state = world.run(state, speech_event("What can I say?"));
            assert!(
                state.help_overlay().is_some(),
                "phrase opens help overlay for {app_id:?}"
            );
            assert!(
                state.launcher().is_none(),
                "opening help overlay closes the launcher invitation"
            );
            assert!(
                state.overlay_owns_pad(),
                "help overlay owns the pad (passthrough = {passthrough})"
            );
            assert!(
                !world.trace.iter().any(|effect| matches!(
                    effect,
                    Effect::QueryPlugin(Event::Speech { .. })
                        | Effect::DispatchPlugin {
                            event: Event::Speech { .. },
                            ..
                        }
                )),
                "plugins never see the built-in help overlay phrase"
            );
        }
    }

    #[test]
    fn guide_menu_entry_shows_exact_voice_name_and_opens_overlay_on_one_press() {
        assert_eq!(
            tool_word(Command::WhatCanISay.label()),
            Some(ToolWord::WhatCanISay),
            "Guide menu entry shows the exact name voice uses"
        );
        let mut world = World::default();
        let mut state = focused_state(&[GLOBAL, EDITOR], Some("org.example.Editor"));
        state = world.run(state, button_input(ButtonName::Guide, ButtonState::Pressed));
        state = world.run(
            state,
            Input::Tick {
                elapsed: Duration::from_millis(500),
            },
        );
        state = world.run(state, button_input(ButtonName::Guide, ButtonState::Released));
        let index = tehuti_core::menu::ITEMS
            .iter()
            .position(|command| *command == Command::WhatCanISay)
            .expect("WhatCanISay in menu::ITEMS");
        for _ in 0..index {
            state = world.tap(state, ButtonName::DpadDown);
        }
        world.trace.clear();
        state = world.tap(state, ButtonName::South);
        assert!(state.core().menu().is_none(), "system menu closes");
        assert!(state.help_overlay().is_some(), "help overlay opens");
        assert!(
            !world
                .trace
                .iter()
                .any(|effect| matches!(effect, Effect::RunMenuCommand(_))),
            "handled in core dispatch without spawning a shell command"
        );
    }

    #[test]
    fn dpad_steps_through_groups_and_entries_confirm_runs_and_back_closes_without_leaking() {
        let mut world = World::default();
        let mut state = focused_state(&[GLOBAL, EDITOR], Some("org.example.Editor"));
        state = world.run(state, speech_event(WHAT_CAN_I_SAY));
        assert_eq!(state.help_overlay().map(HelpOverlay::active_group_index), Some(0));
        assert_eq!(state.help_overlay().map(HelpOverlay::selected_index), Some(0));

        // D-pad Right jumps to FocusedPad; D-pad Left returns to FocusedVoice.
        state = world.tap(state, ButtonName::DpadRight);
        assert_eq!(state.help_overlay().map(HelpOverlay::active_group_index), Some(1));
        state = world.tap(state, ButtonName::DpadLeft);
        assert_eq!(state.help_overlay().map(HelpOverlay::active_group_index), Some(0));

        // Press Confirm on the first focused voice entry (Save -> Ctrl+S).
        world.trace.clear();
        let expected_target = state.help_overlay().unwrap().confirm_target();
        let Target::Action(expected_action) = expected_target else {
            panic!("first focused voice entry should be runnable: {expected_target:?}");
        };
        state = world.tap(state, ButtonName::South);
        assert!(state.help_overlay().is_none(), "confirm closes overlay");
        assert!(
            world.trace.contains(&Effect::Apply(expected_action)),
            "confirm runs the selected action: {:?}",
            world.trace
        );

        // Reopen, close with Back (East), and verify East does not leak Escape to the app.
        state = world.run(state, speech_event(WHAT_CAN_I_SAY));
        world.trace.clear();
        state = world.tap(state, ButtonName::East);
        assert!(state.help_overlay().is_none(), "Back closes overlay");
        assert!(
            !world
                .trace
                .iter()
                .any(|effect| matches!(effect, Effect::Apply(_))),
            "closing with Back leaks no button action: {:?}",
            world.trace
        );
    }

    #[test]
    fn confirming_a_built_in_voice_unit_routes_it_through_dispatch() {
        let mut world = World::default();
        let mut state = focused_state(&[GLOBAL], None);
        state = world.run(state, speech_event(WHAT_CAN_I_SAY));
        // Navigate to "keyboard" inside GlobalVoice.
        let target_index = state
            .help_overlay()
            .unwrap()
            .active_group()
            .unwrap()
            .items
            .iter()
            .position(|item| item.voice_name.as_deref() == Some("keyboard"))
            .expect("keyboard listed in GlobalVoice");
        for _ in 0..target_index {
            state = world.tap(state, ButtonName::DpadDown);
        }
        state = world.tap(state, ButtonName::South);
        assert!(state.help_overlay().is_none(), "confirm closes help overlay");
        assert!(
            state.keyboard().is_open(),
            "confirming \"keyboard\" opens the on-screen keyboard"
        );
    }

    #[test]
    fn speaking_another_command_while_overlay_is_open_closes_it_and_runs_the_command() {
        let mut world = World::default();
        let mut state = focused_state(&[GLOBAL, EDITOR], Some("org.example.Editor"));
        state = world.run(state, speech_event(WHAT_CAN_I_SAY));
        assert!(state.help_overlay().is_some());
        world.trace.clear();
        state = world.run(state, speech_event("Save."));
        assert!(state.help_overlay().is_none(), "overlay closes on spoken command");
        assert!(
            world.trace.contains(&Effect::Apply(Action::Key {
                key: KeyName::S,
                mods: vec![Modifier::Ctrl],
            })),
            "spoken command runs immediately"
        );
    }

    #[test]
    fn dictation_in_open_keyboard_is_not_hijacked_by_help_overlay_phrase() {
        let mut world = World::default();
        let mut state = focused_state(&[GLOBAL], None);
        state = world.run(state, speech_event("keyboard"));
        assert!(state.keyboard().is_open());
        state = world.run(state, speech_event("What can I say"));
        assert!(
            state.help_overlay().is_none(),
            "dictation keeps speech inside the keyboard"
        );
        assert!(state.keyboard().is_open());
    }

    #[test]
    fn every_runnable_built_in_voice_name_matches_the_real_dispatcher() {
        let state = focused_state(&[], None);
        let overlay = state.core().help_overlay(&[]);
        let global_voice = overlay
            .groups()
            .iter()
            .find(|group| group.kind == GroupKind::GlobalVoice)
            .expect("GlobalVoice group");
        for item in &global_voice.items {
            let Target::Speech(phrase) = &item.target else {
                continue;
            };
            let mut world = World::default();
            let after = world.run(focused_state(&[], None), speech_event(phrase));
            let handled = after.keyboard().is_open()
                || after.settings().open
                || after.launcher().is_some()
                || world.trace.iter().any(|effect| {
                    matches!(
                        effect,
                        Effect::MouseMode(_)
                            | Effect::ReadVolume
                            | Effect::ReadWindowSize(_)
                            | Effect::RunMenuCommand(_)
                            | Effect::OpenLauncher(_)
                            | Effect::Apply(_)
                    )
                });
            assert!(
                handled,
                "built-in voice unit {phrase:?} must be recognised by dispatch: {:?}",
                world.trace
            );
        }
    }

    fn core_at_level(list: Vec<Context>, focused: Option<&str>, pad: PadFamily, level: Level) -> CoreState {
        let mut core = CoreState::with_contexts(list);
        core.set_pad_family(pad);
        if let Some(app_id) = focused {
            core = tehuti_core::step(
                core,
                &Event::Focus {
                    app_id: Some(app_id.into()),
                    title: String::new(),
                },
            )
            .0;
        }
        if level == Level::Columns {
            for state in [ButtonState::Pressed, ButtonState::Released] {
                core = tehuti_core::step(
                    core,
                    &Event::Button {
                        name: ButtonName::Guide,
                        state,
                    },
                )
                .0;
            }
        }
        core
    }

    proptest! {
        #[test]
        fn overlay_contains_every_ticker_entry_and_dpad_walks_every_item_cleanly(
            pad in prop::sample::select(vec![PadFamily::Letters, PadFamily::Symbols, PadFamily::Nintendo]),
            level in prop::sample::select(vec![Level::App, Level::Columns]),
            use_editor in any::<bool>(),
        ) {
            let sources: &[&str] = if use_editor { &[GLOBAL, EDITOR] } else { &[GLOBAL] };
            let list = contexts(sources);
            let focused = use_editor.then_some("org.example.Editor");
            let ticker_entries = ticker::entries(&list, focused, pad, level);
            let core = core_at_level(list, focused, pad, level);
            let mut overlay = core.help_overlay(&[]);
            let overlay_triggers: Vec<_> = overlay
                .groups()
                .iter()
                .flat_map(|group| group.items.iter().map(|item| item.trigger.clone()))
                .collect();
            for entry in &ticker_entries {
                prop_assert!(
                    overlay_triggers.contains(&entry.trigger),
                    "ticker trigger {:?} missing from help overlay",
                    entry.trigger
                );
            }
            let total_items: usize = overlay.groups().iter().map(|group| group.items.len()).sum();
            prop_assert!(total_items > 0);
            for _ in 0..total_items.saturating_add(3) {
                overlay.step_entry(true);
                prop_assert!(overlay.selected_item().is_some());
                prop_assert!(overlay.visible_range().contains(&overlay.selected_index()));
            }
            prop_assert_eq!(overlay.active_group_index(), overlay.groups().len().saturating_sub(1));
            for _ in 0..total_items.saturating_add(3) {
                overlay.step_entry(false);
                prop_assert!(overlay.selected_item().is_some());
                prop_assert!(overlay.visible_range().contains(&overlay.selected_index()));
            }
            prop_assert_eq!(overlay.active_group_index(), 0);
            prop_assert_eq!(overlay.selected_index(), 0);
        }
    }
}
