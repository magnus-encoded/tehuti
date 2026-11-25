#[cfg(test)]
mod quick_info {
    use core::time::Duration;

    use proptest::prelude::*;
    use tehuti_core::{
        dispatch::{Effect, Input, State as DispatchState, step as dispatch_step},
        pad::PadFamily,
        quick_info::{Battery, BatteryStatus, InfoItem, LIFETIME, Reading, from_phrase, notice_text},
    };
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, KeyName, Text, parse_context};

    fn run(mut state: DispatchState, input: Input) -> (DispatchState, Vec<Effect>) {
        let mut trace = Vec::new();
        let (next, mut effects) = dispatch_step(state, input);
        state = next;
        while !effects.is_empty() || state.busy() > 0 {
            let mut reply = None;
            for effect in effects.drain(..) {
                trace.push(effect.clone());
                reply = match effect {
                    Effect::ReadClock => Some(Input::ClockRead(Duration::ZERO)),
                    Effect::QueryPlugin(_) => Some(Input::PluginActive(false)),
                    Effect::DispatchPlugin { .. } => Some(Input::PluginRequest(None)),
                    Effect::Supervise => Some(Input::Supervised(Vec::new())),
                    Effect::ConfirmSecret(_) => Some(Input::SecretConfirmation(false)),
                    Effect::SpeechCommand(_) => Some(Input::SpeechSent(true)),
                    Effect::PollSpeech(_) => Some(Input::SpeechPolled(Vec::new())),
                    Effect::Handle(_) => panic!("unexpected nested handle effect"),
                    Effect::InvalidReply => panic!("unexpected invalid reply"),
                    Effect::ChooseLauncher(_)
                    | Effect::LauncherUsed(_)
                    | Effect::OpenLauncher(_)
                    | Effect::AdjusterValue { .. }
                    | Effect::ReadVolume
                    | Effect::ReadWindowSize(_)
                    | Effect::SetWindowSize { .. }
                    | Effect::SetVolume(_)
                    | Effect::PasswordSpeechIgnored
                    | Effect::ShowInfo(_)
                    | Effect::DismissInfo
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
                };
                if reply.is_some() {
                    break;
                }
            }
            if state.busy() == 0 {
                break;
            }
            let (next, more) = dispatch_step(state, reply.unwrap_or(Input::Resume));
            state = next;
            effects = more;
        }
        (state, trace)
    }

    fn say(state: DispatchState, phrase: &str) -> (DispatchState, Vec<Effect>) {
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

    fn press(state: DispatchState, name: ButtonName) -> (DispatchState, Vec<Effect>) {
        run(
            state,
            Input::Event {
                event: Event::Button {
                    name,
                    state: ButtonState::Pressed,
                },
                elapsed: Duration::ZERO,
            },
        )
    }

    fn release(state: DispatchState, name: ButtonName) -> (DispatchState, Vec<Effect>) {
        run(
            state,
            Input::Event {
                event: Event::Button {
                    name,
                    state: ButtonState::Released,
                },
                elapsed: Duration::ZERO,
            },
        )
    }

    fn with_cycle_button(extra: &str) -> DispatchState {
        let context = parse_context(&format!(
            "app_id = 'app'\n[buttons]\nwest = {{ info = 'cycle' }}\neast = {{ key = 'escape' }}\n{extra}"
        ))
        .unwrap();
        let state = DispatchState::new(tehuti_core::State::with_contexts(vec![context]));
        run(
            state,
            Input::Event {
                event: Event::Focus {
                    app_id: Some("app".into()),
                    title: String::new(),
                },
                elapsed: Duration::ZERO,
            },
        )
        .0
    }

    #[test]
    fn voice_phrases_emit_show_info_for_each_unit() {
        for (phrase, expected) in [
            ("What time is it?", InfoItem::Time),
            ("what time is it", InfoItem::Time),
            ("Date.", InfoItem::Date),
            ("Battery!", InfoItem::Battery),
        ] {
            let (state, effects) = say(DispatchState::default(), phrase);
            assert!(
                effects.contains(&Effect::ShowInfo(expected)),
                "{phrase}: expected ShowInfo({expected:?}), got {effects:?}"
            );
            assert_eq!(state.quick_info(), Some(expected));
        }
    }

    #[test]
    fn pad_cycle_steps_through_time_date_battery_without_a_phrase() {
        let state = with_cycle_button("");
        let (state, first) = press(state, ButtonName::West);
        assert!(
            first.contains(&Effect::ShowInfo(InfoItem::Time)),
            "first press shows time: {first:?}"
        );
        let (state, second) = press(state, ButtonName::West);
        assert!(
            second.contains(&Effect::ShowInfo(InfoItem::Date)),
            "second press shows date: {second:?}"
        );
        let (state, third) = press(state, ButtonName::West);
        assert!(
            third.contains(&Effect::ShowInfo(InfoItem::Battery)),
            "third press shows battery: {third:?}"
        );
        let (state, fourth) = press(state, ButtonName::West);
        assert!(
            fourth.contains(&Effect::ShowInfo(InfoItem::Time)),
            "fourth press wraps to time: {fourth:?}"
        );
        assert_eq!(state.quick_info(), Some(InfoItem::Time));
    }

    #[test]
    fn pad_cycle_advances_from_a_spoken_unit() {
        let state = with_cycle_button("");
        let (state, _) = say(state, "battery");
        let (state, next) = press(state, ButtonName::West);
        assert!(
            next.contains(&Effect::ShowInfo(InfoItem::Time)),
            "cycling after battery wraps to time: {next:?}"
        );
        assert_eq!(state.quick_info(), Some(InfoItem::Time));
    }

    #[test]
    fn back_button_dismisses_open_notice_without_leaking_to_the_app() {
        let state = with_cycle_button("");
        let (state, _) = press(state, ButtonName::West);
        assert_eq!(state.quick_info(), Some(InfoItem::Time));

        let (state, pressed) = press(state, ButtonName::East);
        assert!(
            pressed.contains(&Effect::DismissInfo),
            "B must dismiss the info notice: {pressed:?}"
        );
        assert!(
            !pressed.iter().any(|effect| matches!(effect, Effect::Apply(_))),
            "dismissing press must not reach the app: {pressed:?}"
        );
        assert_eq!(state.quick_info(), None);

        let (state, released) = release(state, ButtonName::East);
        assert!(
            !released.iter().any(|effect| matches!(effect, Effect::Apply(_))),
            "dismissing release must not reach the app: {released:?}"
        );

        let (state, forwarded) = press(state, ButtonName::East);
        assert!(
            forwarded.contains(&Effect::Apply(Action::Key {
                key: KeyName::Escape,
                mods: Vec::new(),
            })),
            "once dismissed, B reaches the app again: {forwarded:?}"
        );

        let (_, restarted) = press(state, ButtonName::West);
        assert!(
            restarted.contains(&Effect::ShowInfo(InfoItem::Time)),
            "next cycle after dismiss starts at time: {restarted:?}"
        );
    }

    #[test]
    fn nintendo_pad_uses_its_back_role_to_dismiss() {
        let mut state = with_cycle_button("");
        state.core_mut().set_pad_family(PadFamily::Nintendo);
        let (state, _) = say(state, "battery");
        let (state, effects) = press(state, ButtonName::South);
        assert!(
            effects.contains(&Effect::DismissInfo),
            "Nintendo Back role (South) dismisses quick info: {effects:?}"
        );
        assert_eq!(state.quick_info(), None);
    }

    #[test]
    fn info_notice_expires_and_resets_the_cycle() {
        let state = with_cycle_button("");
        let (state, _) = press(state, ButtonName::West);
        let (state, early) = run(
            state,
            Input::Tick {
                elapsed: Duration::from_secs(1),
            },
        );
        assert!(
            !early.contains(&Effect::DismissInfo),
            "notice must stay open before lifetime elapses: {early:?}"
        );
        let (state, expired) = run(state, Input::Tick { elapsed: LIFETIME });
        assert!(
            expired.contains(&Effect::DismissInfo),
            "notice must dismiss at lifetime: {expired:?}"
        );
        assert_eq!(state.quick_info(), None);
        let (_, restarted) = press(state, ButtonName::West);
        assert!(
            restarted.contains(&Effect::ShowInfo(InfoItem::Time)),
            "cycle restarts at time after expiry: {restarted:?}"
        );
    }

    #[test]
    fn voice_phrases_outrank_context_mappings_and_work_in_passthrough() {
        let context = parse_context(
            "app_id = 'game'\n[pad]\npassthrough = true\n[speech]\nbattery = { text = 'wrong' }\n",
        )
        .unwrap();
        let state = DispatchState::new(tehuti_core::State::with_contexts(vec![context]));
        let (state, _) = run(
            state,
            Input::Event {
                event: Event::Focus {
                    app_id: Some("game".into()),
                    title: String::new(),
                },
                elapsed: Duration::ZERO,
            },
        );
        let (state, effects) = say(state, "battery");
        assert!(
            effects.contains(&Effect::ShowInfo(InfoItem::Battery)),
            "core owns quick info phrases in passthrough too: {effects:?}"
        );
        assert!(
            !effects.iter().any(|effect| matches!(effect, Effect::Apply(_))),
            "context mapping must not shadow reserved info phrase: {effects:?}"
        );
        assert!(
            !state.overlay_owns_pad(),
            "quick info notice must not steal the pad from passthrough"
        );
    }

    #[test]
    fn voice_phrases_do_not_hijack_keyboard_dictation_or_password_fields() {
        let state = DispatchState::default();
        let (state, _) = say(state, "keyboard");
        assert!(state.keyboard().is_open(), "keyboard should be open");
        let (state, dictated) = say(state, "battery");
        assert!(
            !dictated
                .iter()
                .any(|effect| matches!(effect, Effect::ShowInfo(_))),
            "open keyboard must dictate rather than show info: {dictated:?}"
        );
        assert!(
            state.keyboard().draft().contains("battery"),
            "keyboard draft should receive the dictated word"
        );
    }

    #[test]
    fn core_step_and_resolver_skip_reserved_info_phrases() {
        let context = parse_context("app_id = 'app'\nunmatched_speech = 'type'\n").unwrap();
        let (core, _) = tehuti_core::step(
            tehuti_core::State::with_contexts(vec![context]),
            &Event::Focus {
                app_id: Some("app".into()),
                title: String::new(),
            },
        );
        for item in InfoItem::ALL {
            let phrase = item.voice_name();
            let (_, actions) = tehuti_core::step(
                core.clone(),
                &Event::Speech {
                    text: Text::new(phrase.into()).unwrap(),
                },
            );
            assert!(
                actions.is_empty(),
                "reserved info phrase must not emit fallback actions: {phrase}: {actions:?}"
            );
            assert_eq!(
                tehuti_core::unmatched_speech_commands(&core, phrase),
                None,
                "speech resolver must not claim reserved info phrase: {phrase}"
            );
        }
    }

    #[test]
    fn plugins_cannot_emit_cycle_info() {
        let error = tehuti_core::plugin::validate_actions(vec![Action::CycleInfo], &[]).unwrap_err();
        assert!(
            error.contains("reserved for the core"),
            "CycleInfo must be reserved for core: {error}"
        );
    }

    #[test]
    fn missing_battery_formats_a_clear_no_battery_notice() {
        let reading = Reading {
            time: "09:05".into(),
            date: "Wed 2026-11-25".into(),
            battery: None,
        };
        let text = notice_text(InfoItem::Battery, &reading);
        assert_eq!(text, "battery: no battery");
    }

    fn any_status() -> impl Strategy<Value = BatteryStatus> {
        prop_oneof![
            Just(BatteryStatus::Charging),
            Just(BatteryStatus::Discharging),
            Just(BatteryStatus::Full),
            Just(BatteryStatus::NotCharging),
            Just(BatteryStatus::Unknown),
        ]
    }

    proptest! {
        #[test]
        fn every_unit_notice_shows_the_exact_name_voice_matches(
            hour in 0u8..24,
            minute in 0u8..60,
            percent in 0u8..=120,
            has_battery in any::<bool>(),
            status in any_status(),
        ) {
            let reading = Reading {
                time: format!("{hour:02}:{minute:02}"),
                date: "Wed 2026-11-25".into(),
                battery: has_battery.then(|| Battery::new(percent, status)),
            };
            for item in InfoItem::ALL {
                let text = notice_text(item, &reading);
                let signpost = text.split(':').next().unwrap_or_default();
                prop_assert_eq!(
                    signpost,
                    item.voice_name(),
                    "notice must show the unit's voice name"
                );
                prop_assert_eq!(
                    from_phrase(signpost),
                    Some(item),
                    "saying the displayed signpost must match the same unit"
                );
            }
        }
    }
}
