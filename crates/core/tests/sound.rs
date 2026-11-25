//! Sound voice destinations, controller step chords, bar state and ADR 0008 signposts.

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use proptest::prelude::*;
    use tehuti_core::{
        Level,
        bar::Tone,
        dispatch::{Effect, Input, State, step},
        launcher::Launcher,
        sound::{AudioOutput, SoundCommand, match_output, next_output},
    };
    use tehuti_protocol::{ButtonName, ButtonState, Event, Text, parse_context};

    #[expect(clippy::too_many_lines, reason = "exhaustive dispatch test harness")]
    fn run(mut state: State, initial: Input) -> (State, Vec<Effect>) {
        let mut trace = Vec::new();
        let depth = state.busy();
        let (mut next, mut effects) = step(state, initial);
        state = next;
        loop {
            let mut reply = Input::Resume;
            for effect in effects {
                trace.push(effect.clone());
                match effect {
                    Effect::ReadClock => reply = Input::ClockRead(Duration::from_millis(16)),
                    Effect::QueryPlugin(_) => reply = Input::PluginActive(false),
                    Effect::DispatchPlugin { .. } => reply = Input::PluginRequest(None),
                    Effect::Supervise => reply = Input::Supervised(Vec::new()),
                    Effect::ConfirmSecret(_) => reply = Input::SecretConfirmation(false),
                    Effect::SpeechCommand(_) => reply = Input::SpeechSent(true),
                    Effect::PollSpeech(_) => reply = Input::SpeechPolled(Vec::new()),
                    Effect::Handle(event) => {
                        let (handled, mut nested) = run(
                            state,
                            Input::Event {
                                event,
                                elapsed: Duration::ZERO,
                            },
                        );
                        state = handled;
                        trace.append(&mut nested);
                    }
                    Effect::ChooseLauncher(_)
                    | Effect::LauncherUsed(_)
                    | Effect::OpenLauncher(_)
                    | Effect::AdjusterValue { .. }
                    | Effect::ReadVolume
                    | Effect::ReadWindowSize(_)
                    | Effect::SetWindowSize { .. }
                    | Effect::SetVolume(_)
                    | Effect::SetMute(_)
                    | Effect::ToggleMute
                    | Effect::SetMicrophoneMute(_)
                    | Effect::ToggleMicrophoneMute
                    | Effect::NextAudioOutput
                    | Effect::SelectAudioOutput(_)
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
                    | Effect::SpeechRouted { .. } => {}
                }
            }
            if state.busy() <= depth {
                return (state, trace);
            }
            (next, effects) = step(state, reply);
            state = next;
        }
    }

    fn say(state: State, phrase: &str) -> (State, Vec<Effect>) {
        run(
            state,
            Input::Event {
                event: Event::Speech {
                    text: Text::new(phrase.into()).unwrap(),
                },
                elapsed: Duration::ZERO,
            },
        )
    }

    fn button(state: State, name: ButtonName, button_state: ButtonState) -> (State, Vec<Effect>) {
        run(
            state,
            Input::Event {
                event: Event::Button {
                    name,
                    state: button_state,
                },
                elapsed: Duration::ZERO,
            },
        )
    }

    fn output(id: u32, name: &str, default: bool) -> AudioOutput {
        AudioOutput {
            id,
            name: name.into(),
            default,
        }
    }

    #[test]
    fn spoken_sound_words_emit_destinations_and_reject_step_phrases() {
        let cases = [
            ("Mute.", Effect::SetMute(true)),
            ("Unmute!", Effect::SetMute(false)),
            ("Microphone mute.", Effect::SetMicrophoneMute(true)),
            ("Microphone on!", Effect::SetMicrophoneMute(false)),
            (
                "Speaker headphones.",
                Effect::SelectAudioOutput("headphones".into()),
            ),
        ];
        for (phrase, expected) in cases {
            let (_state, effects) = say(State::default(), phrase);
            assert!(
                effects.contains(&expected),
                "{phrase} must emit {expected:?}: {effects:?}"
            );
            assert!(
                !effects.iter().any(|effect| matches!(
                    effect,
                    Effect::QueryPlugin(Event::Speech { .. }) | Effect::SpeechRouted { .. }
                )),
                "contexts and plugins must never shadow {phrase}"
            );
        }
        assert_eq!(
            SoundCommand::parse("next speaker"),
            None,
            "steps never go on voice (ADR 0008)"
        );
        assert_eq!(
            SoundCommand::parse("speaker"),
            None,
            "bare speaker without an output name is not a destination"
        );
    }

    #[test]
    fn guide_chords_toggle_mute_microphone_mute_and_cycle_output() {
        let cases = [
            (ButtonName::West, Effect::ToggleMute),
            (ButtonName::North, Effect::ToggleMicrophoneMute),
            (ButtonName::East, Effect::NextAudioOutput),
        ];
        for (chord_button, expected) in cases {
            let (state, _) = button(State::default(), ButtonName::Guide, ButtonState::Pressed);
            let (state, effects) = button(state, chord_button, ButtonState::Pressed);
            assert!(
                effects.contains(&expected),
                "Guide + {chord_button:?} must emit {expected:?}: {effects:?}"
            );
            let (state, repeat_effects) = button(state, chord_button, ButtonState::Pressed);
            assert!(
                !repeat_effects.contains(&expected),
                "holding {chord_button:?} must not fire repeatedly"
            );
            let (state, _) = button(state, ButtonName::Guide, ButtonState::Released);
            assert_eq!(
                state.core().level(),
                Level::App,
                "a sound chord must cancel the Guide level tap"
            );
            assert!(
                state.core().menu().is_none(),
                "a sound chord must cancel the Guide menu"
            );
            let (_state, release_effects) = button(state, chord_button, ButtonState::Released);
            assert!(
                !release_effects
                    .iter()
                    .any(|effect| matches!(effect, Effect::Apply(_))),
                "chord button release after Guide must never reach the app"
            );
        }
    }

    #[test]
    fn sound_words_and_chords_work_in_passthrough_and_over_invitation() {
        let context = parse_context(
            "app_id = 'org.example.Game'\nbuttons = {}\n[pad]\npassthrough = true\n[speech]\n'mute' = { key = 'm' }",
        )
        .unwrap();
        let state = State::new(tehuti_core::State::with_contexts(vec![context]));
        let (state, _) = run(
            state,
            Input::Event {
                event: Event::Focus {
                    app_id: Some("org.example.Game".into()),
                    title: String::new(),
                },
                elapsed: Duration::ZERO,
            },
        );
        let (state, voice_effects) = say(state, "mute");
        assert!(
            voice_effects.contains(&Effect::SetMute(true)),
            "built-in mute beats passthrough context mappings"
        );
        let (state, _) = button(state, ButtonName::Guide, ButtonState::Pressed);
        let (_state, chord_effects) = button(state, ButtonName::West, ButtonState::Pressed);
        assert!(
            chord_effects.contains(&Effect::ToggleMute),
            "Guide + West works in passthrough"
        );

        let (invited, _) = run(
            State::default(),
            Input::Launcher(Box::new(Launcher::invitation(None))),
        );
        let (_after, effects) = say(invited, "speaker headphones");
        assert!(
            effects.contains(&Effect::SelectAudioOutput("headphones".into())),
            "sound word runs over the empty-desktop invitation"
        );
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::OpenLauncher(_))),
            "sound word must not search the launcher"
        );
    }

    #[test]
    fn bar_shows_mute_microphone_mute_and_output_signposts() {
        let (state, _) = run(State::default(), Input::AudioMuted(Some(true)));
        assert!(state.core().bar().muted(), "muted state tracked on bar");
        assert!(
            state
                .core()
                .bar()
                .segments()
                .iter()
                .any(|segment| segment.text == "Mute" && segment.tone == Tone::Error),
            "Mute chip must appear on the bar"
        );
        let (state, _) = run(state, Input::AudioMuted(Some(false)));
        assert!(!state.core().bar().muted(), "unmuted state clears bar flag");
        assert!(
            !state
                .core()
                .bar()
                .segments()
                .iter()
                .any(|segment| segment.text == "Mute"),
            "Mute chip must disappear after unmute"
        );
        assert!(
            state
                .core()
                .bar()
                .segments()
                .iter()
                .any(|segment| segment.text == "Unmute" && segment.tone == Tone::Result),
            "Unmute feedback must teach the voice word"
        );

        let (state, _) = run(state, Input::MicrophoneMuted(Some(true)));
        assert!(
            state
                .core()
                .bar()
                .segments()
                .iter()
                .any(|segment| segment.text == "Microphone mute" && segment.tone == Tone::Error),
            "Microphone mute chip must appear on the bar"
        );
        let (state, _) = run(state, Input::MicrophoneMuted(Some(false)));
        assert!(
            state
                .core()
                .bar()
                .segments()
                .iter()
                .any(|segment| segment.text == "Microphone on" && segment.tone == Tone::Result),
            "Microphone on feedback must teach the voice word"
        );

        let headphones = output(47, "Headphones", true);
        let (state, _) = run(state, Input::AudioOutputSelected(Ok(headphones)));
        assert!(
            state
                .core()
                .bar()
                .segments()
                .iter()
                .any(|segment| segment.text == "Speaker headphones" && segment.tone == Tone::Result),
            "output switch must show the exact voice phrase"
        );
    }

    #[test]
    fn output_matching_handles_exact_word_fuzzy_and_ambiguous_names() {
        let outputs = vec![
            output(41, "Built-in Audio Analog Stereo", true),
            output(47, "USB Headphones", false),
            output(52, "HDMI Monitor", false),
        ];
        assert_eq!(
            match_output(&outputs, "usb headphones").map(|chosen| chosen.id),
            Some(47)
        );
        assert_eq!(
            match_output(&outputs, "headphones").map(|chosen| chosen.id),
            Some(47)
        );
        assert_eq!(
            match_output(&outputs, "hedphones").map(|chosen| chosen.id),
            Some(47)
        );
        assert_eq!(match_output(&outputs, "toaster"), None);

        let ambiguous = vec![
            output(47, "USB Headphones", true),
            output(48, "Bluetooth Headphones", false),
        ];
        assert_eq!(
            match_output(&ambiguous, "headphones"),
            None,
            "ambiguous matches must not guess"
        );
    }

    proptest! {
        /// ADR 0008: every unit's signpost parses back to that exact same unit.
        #[test]
        fn every_sound_signpost_matches_what_voice_parses(
            name in "[A-Za-z][A-Za-z0-9 ]{2,24}",
            id in 1_u32..1000,
        ) {
            for muted in [true, false] {
                prop_assert_eq!(
                    SoundCommand::parse(SoundCommand::mute_signpost(muted)),
                    Some(SoundCommand::SetMute(muted))
                );
                prop_assert_eq!(
                    SoundCommand::parse(SoundCommand::microphone_signpost(muted)),
                    Some(SoundCommand::SetMicrophoneMute(muted))
                );
            }
            let sink = AudioOutput {
                id,
                name,
                default: true,
            };
            let parsed = SoundCommand::parse(&sink.signpost());
            prop_assert_eq!(parsed.clone(), Some(SoundCommand::SelectOutput(sink.spoken_name())));
            if let Some(SoundCommand::SelectOutput(query)) = parsed {
                prop_assert_eq!(match_output(core::slice::from_ref(&sink), &query), Some(&sink));
            }
        }

        /// Output cycling visits every sink in order and wraps back to the start.
        #[test]
        fn cycling_outputs_visits_every_sink_and_wraps(
            count in 1_usize..8,
            start in 0_usize..8,
        ) {
            let start = start.checked_rem(count).unwrap_or(0);
            let mut outputs: Vec<AudioOutput> = (0..count)
                .map(|index| AudioOutput {
                    id: u32::try_from(index.saturating_add(10)).unwrap_or(10),
                    name: format!("Output {index}"),
                    default: index == start,
                })
                .collect();
            let mut visited = Vec::with_capacity(count);
            for _ in 0..count {
                let chosen = next_output(&outputs).unwrap().id;
                visited.push(chosen);
                for sink in &mut outputs {
                    sink.default = sink.id == chosen;
                }
            }
            let unique: std::collections::HashSet<u32> = visited.iter().copied().collect();
            prop_assert_eq!(unique.len(), count);
            prop_assert_eq!(visited.last().copied(), outputs.get(start).map(|sink| sink.id));
        }
    }
}
