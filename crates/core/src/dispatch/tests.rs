use core::fmt::Write as _;

use super::*;
use proptest::prelude::*;
use tehuti_protocol::{KeyName, Text};

/// Scripted answers to query effects, standing in for the compositor.
#[derive(Default)]
struct World {
    plugin_active: bool,
    secret_consumes: bool,
    speech_output: VecDeque<Vec<SpeechInput>>,
    exits: VecDeque<Vec<usize>>,
    trace: Vec<Effect>,
    clock: VecDeque<Duration>,
    request: Option<(RequestKey, Vec<Permission>)>,
    close_focus: Option<Event>,
}

impl World {
    fn run(&mut self, state: State, input: Input) -> State {
        let depth = state.busy();
        self.run_at_depth(state, input, depth)
    }

    fn close_focus(&mut self, state: State) -> State {
        if let Some(event) = self.close_focus.take() {
            self.run(
                state,
                Input::Event {
                    event,
                    elapsed: Duration::from_secs(9),
                },
            )
        } else {
            state
        }
    }

    fn event(&mut self, state: State, event: Event) -> State {
        self.run(
            state,
            Input::Event {
                event,
                elapsed: Duration::ZERO,
            },
        )
    }

    fn run_at_depth(&mut self, state: State, input: Input, depth: usize) -> State {
        let (mut state, mut effects) = step(state, input);
        loop {
            let mut reply = Input::Resume;
            for effect in effects {
                self.trace.push(effect.clone());
                match effect {
                    Effect::ReadClock => {
                        reply = Input::ClockRead(self.clock.pop_front().unwrap_or(Duration::from_millis(16)));
                    }
                    Effect::InvalidReply => panic!("valid world replies"),
                    Effect::QueryPlugin(_) => reply = Input::PluginActive(self.plugin_active),
                    Effect::DispatchPlugin { route, .. } => {
                        reply = Input::PluginRequest(if route { self.request.clone() } else { None });
                    }
                    Effect::Supervise => {
                        reply = Input::Supervised(self.exits.pop_front().unwrap_or_default());
                    }
                    Effect::ConfirmSecret(_) => reply = Input::SecretConfirmation(self.secret_consumes),
                    Effect::SpeechCommand(_) => reply = Input::SpeechSent(true),
                    Effect::PollSpeech(_) => {
                        reply = Input::SpeechPolled(self.speech_output.pop_front().unwrap_or_default());
                    }
                    Effect::Handle(event) => state = self.event(state, event),
                    Effect::Apply(Action::Close) | Effect::PluginAction(Action::Close) => {
                        state = self.close_focus(state);
                    }
                    Effect::SettingsMode(_)
                    | Effect::PersistMode(_)
                    | Effect::SettingsScale(_)
                    | Effect::SettingsController(_)
                    | Effect::PersistController(_)
                    | Effect::SettingsMicrophone(_)
                    | Effect::ListMicrophones
                    | Effect::RunMenuCommand(_)
                    | Effect::PasswordSpeechIgnored
                    | Effect::MovePointer { .. }
                    | Effect::Scroll { .. }
                    | Effect::Click { .. }
                    | Effect::MouseMode(_)
                    | Effect::PollSources
                    | Effect::Chord(_)
                    | Effect::SouthPressed(_)
                    | Effect::ResetSecretFocus
                    | Effect::Apply(_)
                    | Effect::PluginAction(_)
                    | Effect::PluginRejected(_)
                    | Effect::SpeechFailed { .. }
                    | Effect::TranscriptReceived { .. }
                    | Effect::SpeechRouted { .. } => {}
                }
            }
            if state.busy() <= depth {
                return state;
            }
            (state, effects) = step(state, reply);
        }
    }
}

fn focus(app_id: &str) -> Event {
    Event::Focus {
        app_id: Some(app_id.into()),
        title: String::new(),
    }
}

fn position(trace: &[Effect], wanted: impl Fn(&Effect) -> bool) -> usize {
    trace.iter().position(wanted).expect("effect present")
}

#[test]
fn focus_cancels_pending_transcript_before_polling_elapsed_time() {
    let mut world = World::default();
    let mut state = State::default();
    for input in [SpeechInput::Available, SpeechInput::Press, SpeechInput::Release] {
        state = world.run(state, Input::Speech(input));
    }
    world.speech_output.push_back(vec![SpeechInput::Transcript {
        session: 0,
        text: Text::new("stale".into()).unwrap(),
    }]);
    world.trace.clear();
    state = world.run(
        state,
        Input::Event {
            event: focus("other"),
            elapsed: Duration::from_millis(5),
        },
    );
    assert_eq!(state.busy(), 0);
    assert!(world.trace.contains(&Effect::TranscriptReceived {
        session: 0,
        text: Text::new("stale".into()).unwrap(),
        admitted: false,
    }));
    assert!(!world.trace.iter().any(|effect| matches!(
        effect,
        Effect::Handle(_) | Effect::SpeechFailed { .. } | Effect::Apply(_)
    )));
}

#[test]
fn focus_routes_before_its_tick_and_cancels_capture_first() {
    let mut world = World::default();
    let mut state = State::default();
    for input in [SpeechInput::Available, SpeechInput::Press] {
        state = world.run(state, Input::Speech(input));
    }
    world.trace.clear();
    let _state = world.run(
        state,
        Input::Event {
            event: focus("other"),
            elapsed: Duration::ZERO,
        },
    );
    let trace = &world.trace;
    let reset = position(trace, |effect| *effect == Effect::ResetSecretFocus);
    let cancel = position(trace, |effect| {
        matches!(
            effect,
            Effect::SpeechCommand(SpeechEffect::Stop {
                transcribe: false,
                ..
            })
        )
    });
    let route = position(trace, |effect| {
        matches!(effect, Effect::QueryPlugin(Event::Focus { .. }))
    });
    let tick = position(trace, |effect| {
        matches!(effect, Effect::QueryPlugin(Event::Tick { .. }))
    });
    let poll = position(trace, |effect| matches!(effect, Effect::PollSpeech(_)));
    assert_eq!(trace.first(), Some(&Effect::Supervise));
    assert!(reset < cancel && cancel < route && route < tick && tick < poll);
}

fn event_is(event: &Event, button: ButtonName) -> bool {
    matches!(event, Event::Button { name, .. } if *name == button)
}

fn arbitrary_button() -> impl Strategy<Value = Event> {
    (
        prop::sample::select(vec![
            ButtonName::South,
            ButtonName::East,
            ButtonName::Record,
            ButtonName::Guide,
            ButtonName::Start,
        ]),
        prop::bool::ANY,
    )
        .prop_map(|(name, pressed)| Event::Button {
            name,
            state: if pressed {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            },
        })
}

fn arbitrary_event() -> impl Strategy<Value = Event> {
    prop_oneof![
        arbitrary_button(),
        "[a-z]{1,8}".prop_map(|text| Event::Speech {
            text: Text::new(text).unwrap()
        }),
        "[a-z]{1,4}".prop_map(|app| focus(&app)),
        (0_u64..4000).prop_map(|millis| Event::Tick {
            elapsed: Duration::from_millis(millis)
        }),
    ]
}

proptest! {
    #[test]
    fn every_event_finishes_its_frame(
        events in prop::collection::vec(arbitrary_event(), 0..40),
        contexts in arbitrary_contexts(),
        plugin_active in prop::bool::ANY,
        secret_consumes in prop::bool::ANY,
    ) {
        let mut world = World { plugin_active, secret_consumes, ..World::default() };
        let app = contexts[0].app_id().to_owned();
        let mut state = configured(contexts, &app);
        for event in events {
            state = world.run(state, Input::Event { event, elapsed: Duration::from_millis(16) });
            prop_assert_eq!(state.busy(), 0);
        }
    }

    #[test]
    fn input_events_tick_before_they_route(event in arbitrary_button(), secret_consumes in prop::bool::ANY, contexts in arbitrary_contexts()) {
        let mut world = World { secret_consumes, ..World::default() };
        let app = contexts[0].app_id().to_owned();
        let _state = world.run(configured(contexts, &app), Input::Event { event: event.clone(), elapsed: Duration::ZERO });
        let trace = &world.trace;
        let ticked_first = matches!(trace.first(), Some(Effect::QueryPlugin(Event::Tick { .. })));
        prop_assert!(ticked_first);
        let poll = position(trace, |effect| *effect == Effect::PollSources);
        let routed = trace.iter().position(|effect| *effect == Effect::QueryPlugin(event.clone()));
        let supervisions: Vec<_> = trace.iter().enumerate().filter_map(|(index, effect)| (*effect == Effect::Supervise).then_some(index)).collect();
        prop_assert_eq!(supervisions.len(), 2);
        prop_assert!(supervisions[0] < poll && poll < supervisions[1]);
        let record = event_is(&event, ButtonName::Record);
        let south = event_is(&event, ButtonName::South);
        if record || event_is(&event, ButtonName::Guide) || (south && secret_consumes) {
            prop_assert!(routed.is_none());
        } else {
            prop_assert!(routed.is_some_and(|routed| supervisions[1] < routed));
        }
        if south {
            let pressed = position(trace, |effect| matches!(effect, Effect::SouthPressed(_)));
            let confirm = position(trace, |effect| matches!(effect, Effect::ConfirmSecret(_)));
            prop_assert!(supervisions[1] < pressed && pressed < confirm);
        }
    }

    #[test]
    fn without_plugins_dispatch_applies_what_the_step_returns(
        contexts in arbitrary_contexts(),
        events in prop::collection::vec(arbitrary_event(), 0..40),
    ) {
        let mut world = World::default();
        let app = contexts[0].app_id().to_owned();
        let mut reference = crate::step(crate::State::with_contexts(contexts), &focus(&app)).0;
        let mut state = State::new(reference.clone());
        let mut expected = Vec::new();
        for event in events {
            let record = event_is(&event, ButtonName::Record);
            if !matches!(event, Event::Tick { .. } | Event::Focus { .. }) {
                let (next, mut actions) = crate::step(reference, &Event::Tick { elapsed: Duration::from_millis(16) });
                reference = next;
                expected.append(&mut actions);
            }
            if !record {
                let (next, mut actions) = crate::step(reference, &event);
                reference = next;
                expected.append(&mut actions);
            }
            if let Event::Focus { .. } = event {
                let (next, mut actions) = crate::step(reference, &Event::Tick { elapsed: Duration::from_millis(16) });
                reference = next;
                expected.append(&mut actions);
            }
            state = world.run(state, Input::Event { event, elapsed: Duration::from_millis(16) });
        }
        let applied: Vec<Action> = world.trace.into_iter().filter_map(|effect| if let Effect::Apply(action) = effect { Some(action) } else { None }).collect();
        prop_assert_eq!(applied, expected);
    }
}

#[test]
fn a_failed_speech_command_marks_speech_unavailable() {
    let mut state = State::default();
    let mut world = World::default();
    state = world.run(state, Input::Speech(SpeechInput::Available));
    let (state, started) = step(state, Input::Speech(SpeechInput::Press));
    assert!(matches!(
        started.as_slice(),
        [Effect::SpeechCommand(SpeechEffect::Start { .. })]
    ));
    let (state, stopped) = step(state, Input::SpeechSent(false));
    assert!(matches!(
        stopped.as_slice(),
        [Effect::SpeechCommand(SpeechEffect::Stop {
            transcribe: false,
            ..
        })]
    ));
    let (state, finished) = step(state, Input::SpeechSent(true));
    assert_eq!(finished, Vec::new());
    assert_eq!(state.busy(), 0);
    world.trace.clear();
    let _state = world.run(state, Input::Speech(SpeechInput::Press));
    assert_eq!(world.trace, Vec::new());
}

fn arbitrary_mapping() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::sample::select(vec![
            "{ key = 'enter' }",
            "{ key = 'tab' }",
            "{ close = 'focused' }"
        ])
        .prop_map(str::to_owned),
        (
            prop::sample::select(vec!["text", "notify", "launch"]),
            "[a-z]{1,12}"
        )
            .prop_map(|(kind, text)| format!("{{ {kind} = '{text}' }}")),
    ]
}

fn arbitrary_contexts() -> impl Strategy<Value = Vec<tehuti_protocol::Context>> {
    let button_mappings = prop::collection::hash_map(
        prop::sample::select(vec!["east", "west", "north", "south", "guide", "start"]),
        arbitrary_mapping(),
        0..6,
    );
    let speech_mappings = prop::collection::hash_map("[a-z]{1,8}", arbitrary_mapping(), 0..6);
    prop::collection::vec(
        ("[a-z]{1,8}", button_mappings, speech_mappings, prop::bool::ANY),
        1..5,
    )
    .prop_map(|entries| {
        entries
            .into_iter()
            .map(|(app, buttons, speech, unmatched)| {
                let mut source = format!(
                    "app_id = '{app}'\n{}[buttons]\n",
                    if unmatched {
                        "unmatched_speech = 'type'\n"
                    } else {
                        ""
                    }
                );
                for (button, mapping) in buttons {
                    writeln!(source, "{button} = {mapping}").unwrap();
                }
                source.push_str("[speech]\n");
                for (phrase, mapping) in speech {
                    writeln!(source, "'{phrase}' = {mapping}").unwrap();
                }
                tehuti_protocol::parse_context(&source).unwrap()
            })
            .collect()
    })
}

fn configured(contexts: Vec<tehuti_protocol::Context>, app: &str) -> State {
    let core = crate::State::with_contexts(contexts);
    State::new(crate::step(core, &focus(app)).0)
}

proptest! {
    #[test]
    fn plugin_dispatch_precedes_fallback_and_requests_defer_it(
        mut contexts in arbitrary_contexts(),
        text in "[a-z]{1,12}",
        plugin in 0_usize..8,
        accepted in prop::bool::ANY,
        focusing in prop::bool::ANY,
    ) {
        let app = String::from("dispatch-test");
        let event = Event::Speech { text: Text::new("dispatch".into()).unwrap() };
        let fallback = Action::Text { text: Text::new(text.clone()).unwrap() };
        contexts.insert(0, tehuti_protocol::parse_context(&format!("app_id = '{app}'\n[buttons]\n[speech]\ndispatch = {{ text = {} }}", toml::Value::String(text))).unwrap());
        let key = RequestKey { plugin, request_id: 1 };
        let mut world = World { plugin_active: true,
            request: accepted.then_some((key, vec![Permission::Text])), ..World::default() };
        let mut initial = configured(contexts, &app);
        let event = if focusing {
            let (core, _, _) = crate::plugin::step(initial.core().clone(), crate::plugin::Input::Request {
                key, event, permissions: vec![Permission::Text],
            });
            initial = State::new(core);
            focus("other")
        } else { event };
        let mut state = world.run(initial, Input::Event { event: event.clone(), elapsed: Duration::ZERO });
        let dispatch = position(&world.trace, |effect| matches!(effect, Effect::DispatchPlugin { event: sent, route } if *sent == event && *route != focusing));
        let applied = world.trace.iter().position(|effect| *effect == Effect::Apply(fallback.clone()));
        if accepted && !focusing {
            prop_assert!(applied.is_none());
            prop_assert_eq!(state.core().pending_decisions(), 1);
            world.trace.clear();
            state = world.run(state, Input::PluginReply { key, result: Ok(vec![]) });
            prop_assert_eq!(&world.trace, &vec![Effect::PluginAction(fallback)]);
            prop_assert_eq!(state.core().pending_decisions(), 0);
        } else {
            prop_assert!(applied.is_some_and(|index| dispatch < index));
            prop_assert_eq!(state.core().pending_decisions(), 0);
        }
    }

    #[test]
    fn close_nested_focus_finishes_before_outer_focus_samples_clock(
        mut contexts in arbitrary_contexts(),
        nested_elapsed in 1_u64..100,
        outer_elapsed in 100_u64..200,
        marker in "[a-z]{1,12}",
        plugin in 0_usize..8,
    ) {
        let app = String::from("dispatch-test");
        let marker_action = Action::Text { text: Text::new(marker.clone()).unwrap() };
        contexts.insert(0, tehuti_protocol::parse_context(&format!("app_id = '{app}'\n[buttons.east]\ntext = {}\n[speech]\nclose = {{ close = 'focused' }}", toml::Value::String(marker))).unwrap());
        let initial = configured(contexts, &app);
        let (core, _, _) = crate::plugin::step(initial.core().clone(), crate::plugin::Input::Request {
            key: RequestKey { plugin, request_id: 1 },
            event: Event::Speech { text: Text::new("close".into()).unwrap() }, permissions: vec![],
        });
        let mut world = World { close_focus: Some(focus(&app)), clock: [Duration::from_millis(nested_elapsed), Duration::from_millis(outer_elapsed)].into(), ..World::default() };
        let state = world.run(State::new(core), Input::Event { event: focus("outer"), elapsed: Duration::from_secs(9) });
        prop_assert_eq!(state.busy(), 0);
        let close = position(&world.trace, |effect| *effect == Effect::Apply(Action::Close));
        let nested = position(&world.trace, |effect| *effect == Effect::QueryPlugin(focus(&app)));
        let clocks: Vec<_> = world.trace.iter().enumerate().filter_map(|(index, effect)| (*effect == Effect::ReadClock).then_some(index)).collect();
        prop_assert_eq!(clocks.len(), 2);
        prop_assert!(close < nested && nested < clocks[0] && clocks[0] < clocks[1]);
        let ticks: Vec<_> = world.trace.iter().filter_map(|effect| if let Effect::QueryPlugin(Event::Tick { elapsed }) = effect { Some(*elapsed) } else { None }).collect();
        prop_assert_eq!(ticks, vec![Duration::from_millis(nested_elapsed), Duration::from_millis(outer_elapsed)]);
        world.trace.clear();
        let _state = world.run(state, Input::Event { event: Event::Button { name: ButtonName::East, state: ButtonState::Pressed }, elapsed: Duration::ZERO });
        prop_assert!(world.trace.contains(&Effect::Apply(marker_action)));
    }
}

#[test]
fn missing_or_mismatched_reply_aborts_only_the_waiting_frame() {
    for reply in [
        Input::Resume,
        Input::Supervised(vec![]),
        Input::ClockRead(Duration::ZERO),
    ] {
        let (outer, _) = step(
            State::default(),
            Input::Event {
                event: focus("outer"),
                elapsed: Duration::ZERO,
            },
        );
        let (nested, _) = step(
            outer,
            Input::Tick {
                elapsed: Duration::ZERO,
            },
        );
        let (mut state, effects) = step(nested, reply);
        assert_eq!(effects, vec![Effect::InvalidReply]);
        assert_eq!(state.busy(), 1);
        (state, _) = step(state, Input::Supervised(vec![]));
        let mut world = World::default();
        state = world.run_at_depth(state, Input::Resume, 0);
        assert_eq!(state.busy(), 0);
        for _ in 0..3 {
            let (next, resumed) = step(state, Input::Resume);
            assert_eq!(resumed, vec![]);
            assert_eq!(next.busy(), 0);
            state = next;
        }
    }
}

#[test]
fn voice_bar_follows_capture_routing_and_expiry_through_dispatch() {
    let mut world = World::default();
    let mut state = world.run(State::default(), Input::Speech(SpeechInput::Available));
    state = world.run(state, Input::Speech(SpeechInput::Press));
    assert_eq!(state.core().bar().segments()[1].text, "Listening…");
    let preview = Text::new("hello".into()).unwrap();
    world.trace.clear();
    state = world.run(
        state,
        Input::Speech(SpeechInput::Interim {
            session: 0,
            text: preview.clone(),
        }),
    );
    assert_eq!(state.core().bar().segments()[1].text, "hello");
    assert!(world.trace.is_empty(), "previews stay in core");
    state = world.run(state, Input::Speech(SpeechInput::Release));
    assert_eq!(state.core().bar().segments()[1].text, "Transcribing…");
    state = world.run(
        state,
        Input::Speech(SpeechInput::Transcript {
            session: 0,
            text: preview,
        }),
    );
    assert_eq!(state.core().bar().segments()[1].text, "No speech mapping");
    state = world.run(
        state,
        Input::Tick {
            elapsed: crate::bar::RESULT_LIFETIME,
        },
    );
    assert_eq!(state.core().bar().segments().len(), 1, "focused context chip");
}

#[test]
fn focus_clears_voice_bar_and_rejects_old_capture_through_dispatch() {
    let mut world = World::default();
    let mut state = world.run(State::default(), Input::Speech(SpeechInput::Available));
    state = world.run(state, Input::Speech(SpeechInput::Press));
    state = world.run(
        state,
        Input::Event {
            event: focus("new"),
            elapsed: Duration::ZERO,
        },
    );
    state = world.run(
        state,
        Input::Speech(SpeechInput::Interim {
            session: 0,
            text: Text::new("stale".into()).unwrap(),
        }),
    );
    assert_eq!(
        state.core().bar().segments(),
        vec![crate::bar::Segment {
            text: "new".into(),
            tone: crate::bar::Tone::Context,
        }]
    );
}

fn configured_mouse(contexts: Vec<tehuti_protocol::Context>) -> State {
    let app = contexts[0].app_id().to_owned();
    configured(contexts, &app)
}

fn pad_button(name: ButtonName, state: ButtonState) -> Input {
    Input::Event {
        event: Event::Button { name, state },
        elapsed: Duration::ZERO,
    }
}

fn finish_view_double_tap(world: &mut World, state: State) -> State {
    let state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
    world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed))
}

proptest! {
    #[test]
    fn single_view_presses_keep_the_mapping_without_toggling(
        mut contexts in arbitrary_contexts(),
        duration in 0_u64..10000,
        key in prop::sample::select(vec!["tab", "space", "escape"]),
    ) {
        contexts.insert(0, tehuti_protocol::parse_context(&format!("app_id = 'mouse-test'\n[buttons.select]\nkey = '{key}'")).unwrap());
        let expected_key = match key { "tab" => tehuti_protocol::KeyName::Tab, "space" => tehuti_protocol::KeyName::Space, _ => tehuti_protocol::KeyName::Escape };
        let expected = vec![Action::Key { key: expected_key, mods: Vec::new() }];
        let mut world = World::default();
        let state = world.run(configured_mouse(contexts), pad_button(ButtonName::Select, ButtonState::Pressed));
        let state = world.run(state, Input::Tick { elapsed: Duration::from_millis(duration) });
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::Apply(_) | Effect::MouseMode(_) | Effect::DispatchPlugin { event: Event::Button { name: ButtonName::Select, .. }, .. })), "View is deferred");
        world.trace.clear();
        let state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
        let state = world.run(state, Input::Tick { elapsed: Duration::from_millis(300_u64.saturating_sub(duration)) });
        if duration <= 300 {
            prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::Apply(_))));
        }
        let state = world.run(state, Input::Tick { elapsed: Duration::from_millis(1) });
        let _state = world.run(state, Input::Tick { elapsed: Duration::from_secs(1) });
        let buttons: Vec<_> = world.trace.iter().filter_map(|effect| if let Effect::QueryPlugin(Event::Button { name: ButtonName::Select, state: button }) = effect { Some(*button) } else { None }).collect();
        prop_assert_eq!(buttons, vec![ButtonState::Pressed, ButtonState::Released]);
        let actual: Vec<_> = world.trace.into_iter().filter_map(|effect| if let Effect::Apply(action) = effect { Some(action) } else { None }).collect();
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn double_taps_toggle_once_and_slow_second_taps_do_not(
        contexts in arbitrary_contexts(),
        gap in prop_oneof![Just(0_u64), Just(300), Just(301), 0_u64..1000],
        split in 0_u64..1000,
    ) {
        let mut world = World::default();
        let mut state = configured_mouse(contexts);
        for active in [true, false] {
            state = world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed));
            state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
            world.trace.clear();
            let first = split.min(gap);
            state = world.run(state, Input::Tick { elapsed: Duration::from_millis(first) });
            state = world.run(state, Input::Tick { elapsed: Duration::from_millis(gap.saturating_sub(first)) });
            state = world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed));
            state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
            state = world.run(state, Input::Tick { elapsed: Duration::from_secs(2) });
            let toggles: Vec<_> = world.trace.iter().filter(|effect| matches!(effect, Effect::MouseMode(_))).cloned().collect();
            let expected = if gap <= 300 { vec![Effect::MouseMode(active)] } else { vec![] };
            prop_assert_eq!(toggles, expected);
        }
    }

    #[test]
    fn mouse_mode_swallows_pointer_buttons_before_plugins_and_mappings(
        contexts in arbitrary_contexts(),
        name in prop::sample::select(vec![ButtonName::South, ButtonName::East, ButtonName::West, ButtonName::North, ButtonName::LeftBumper, ButtonName::RightBumper, ButtonName::DpadUp, ButtonName::DpadDown, ButtonName::DpadLeft, ButtonName::DpadRight, ButtonName::LeftStick, ButtonName::RightStick]),
        plugin_active in prop::bool::ANY,
    ) {
        let mut world = World { plugin_active, ..World::default() };
        let state = world.run(configured_mouse(contexts), pad_button(ButtonName::Select, ButtonState::Pressed));
        let state = finish_view_double_tap(&mut world, state);
        let state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
        world.trace.clear();
        let state = world.run(state, pad_button(name, ButtonState::Pressed));
        let _state = world.run(state, pad_button(name, ButtonState::Released));
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::Apply(_) | Effect::SpeechCommand(_) | Effect::ConfirmSecret(_) | Effect::QueryPlugin(Event::Button { .. }) | Effect::DispatchPlugin { event: Event::Button { .. }, .. })), "mouse buttons stay in core");
        let clicks: Vec<_> = world.trace.into_iter().filter(|effect| matches!(effect, Effect::Click { .. })).collect();
        let button = match name { ButtonName::South => Some(PointerButton::Left), ButtonName::East => Some(PointerButton::Right), ButtonName::North | ButtonName::West | ButtonName::LeftBumper | ButtonName::RightBumper | ButtonName::Select | ButtonName::Record | ButtonName::Start | ButtonName::Guide | ButtonName::LeftStick | ButtonName::RightStick | ButtonName::DpadUp | ButtonName::DpadDown | ButtonName::DpadLeft | ButtonName::DpadRight => None };
        let expected = button.map_or_else(Vec::new, |button| vec![Effect::Click { button, pressed: true }, Effect::Click { button, pressed: false }]);
        prop_assert_eq!(clicks, expected);
    }
}

proptest! {
    #[test]
    fn exit_and_disconnect_balance_drag_buttons(
        contexts in arbitrary_contexts(),
        disconnect in prop::bool::ANY,
        right in prop::bool::ANY,
    ) {
        let mut world = World::default();
        let state = world.run(configured_mouse(contexts), pad_button(ButtonName::Select, ButtonState::Pressed));
        let state = finish_view_double_tap(&mut world, state);
        let state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
        let name = if right { ButtonName::East } else { ButtonName::South };
        let button = if right { PointerButton::Right } else { PointerButton::Left };
        let state = world.run(state, pad_button(name, ButtonState::Pressed));
        world.trace.clear();
        let state = if disconnect {
            world.run(state, Input::ControllerDisconnected)
        } else {
            let state = world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed));
            finish_view_double_tap(&mut world, state)
        };
        let clicks: Vec<_> = world.trace.iter().filter(|effect| matches!(effect, Effect::Click { .. })).cloned().collect();
        prop_assert_eq!(clicks, vec![Effect::Click { button, pressed: false }]);
        prop_assert!(world.trace.contains(&Effect::MouseMode(false)));
        world.trace.clear();
        let _state = world.run(state, Input::Tick { elapsed: Duration::from_secs(20) });
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::Click { .. } | Effect::MouseMode(_))), "cleanup happens once");
    }

    #[test]
    fn full_stick_motion_does_not_depend_on_tick_partition(
        contexts in arbitrary_contexts(),
        parts in prop::collection::vec(1_u64..900_000, 2..80),
        negative in prop::bool::ANY,
    ) {
        let mut world = World::default();
        let state = world.run(configured_mouse(contexts), pad_button(ButtonName::Select, ButtonState::Pressed));
        let state = finish_view_double_tap(&mut world, state);
        let axis = if negative { -1000 } else { 1000 };
        let state = world.run(state, Input::Sticks(Sticks { left: (axis, 0), right: (0, 0) }));
        let mut split = state.clone();
        world.trace.clear();
        for nanos in &parts {
            split = world.run(split, Input::Tick { elapsed: Duration::from_nanos(*nanos) });
        }
        let split_distance: i64 = world.trace.iter().filter_map(|effect| if let Effect::MovePointer { x, y: 0 } = effect { Some(i64::from(*x)) } else { None }).sum();
        world.trace.clear();
        let _state = world.run(state, Input::Tick { elapsed: Duration::from_nanos(parts.iter().sum()) });
        let whole_distance: i64 = world.trace.iter().filter_map(|effect| if let Effect::MovePointer { x, y: 0 } = effect { Some(i64::from(*x)) } else { None }).sum();
        prop_assert_ne!(whole_distance, 0, "a held stick moves between submillisecond wakeups");
        prop_assert_eq!(split_distance, whole_distance, "input wakeups cannot change pointer speed");
    }

    #[test]
    fn stick_motion_is_precise_at_small_deflections_and_accelerates(
        contexts in arbitrary_contexts(),
        small in 1_i16..400,
        large in 600_i16..1001,
        negative in prop::bool::ANY,
    ) {
        let mut world = World::default();
        let state = world.run(configured_mouse(contexts), pad_button(ButtonName::Select, ButtonState::Pressed));
        let mut state = finish_view_double_tap(&mut world, state);
        let mut distances = Vec::new();
        let mut scroll_distances = Vec::new();
        for axis in [small, large] {
            let axis = if negative { axis.saturating_neg() } else { axis };
            state = world.run(state, Input::Sticks(Sticks { left: (axis, 0), right: (0, axis) }));
            world.trace.clear();
            state = world.run(state, Input::Tick { elapsed: Duration::from_millis(10) });
            let motion = world.trace.iter().find_map(|effect| if let Effect::MovePointer { x, y: 0 } = effect { Some(*x) } else { None }).unwrap();
            prop_assert_eq!(motion.signum(), if negative { -1 } else { 1 });
            prop_assert!(world.trace.iter().any(|effect| matches!(effect, Effect::Scroll { x: 0, y } if y.signum() == motion.signum())), "right stick scrolls independently");
            let scroll = world.trace.iter().find_map(|effect| if let Effect::Scroll { x: 0, y } = effect { Some(*y) } else { None }).unwrap();
            scroll_distances.push(scroll.abs());
            distances.push(motion.abs());
        }
        prop_assert!(distances[0] < distances[1]);
        prop_assert!(scroll_distances[0] < scroll_distances[1]);
        prop_assert!(scroll_distances[0] < 1_200_000, "small deflections scroll less than two pixels per tick");
        prop_assert!(distances[0] < 1_600_000, "small deflections move less than two pixels per tick");
    }

    #[test]
    fn disconnect_cancels_a_pending_short_view_mapping(
        mut contexts in arbitrary_contexts(),
        elapsed in 0_u64..2000,
    ) {
        contexts.insert(0, tehuti_protocol::parse_context("app_id = 'mouse-test'\n[buttons.select]\nkey = 'tab'").unwrap());
        let mut world = World::default();
        let state = world.run(configured_mouse(contexts), pad_button(ButtonName::Select, ButtonState::Pressed));
        let state = world.run(state, Input::Tick { elapsed: Duration::from_millis(elapsed) });
        let state = world.run(state, Input::ControllerDisconnected);
        world.trace.clear();
        let state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
        let _state = world.run(state, Input::Tick { elapsed: Duration::from_secs(2) });
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::Apply(_) | Effect::MouseMode(_))));
    }
}

proptest! {
    #[test]
    fn right_stick_scrolls_only_vertically_outside_mouse_mode(
        contexts in arbitrary_contexts(),
        left in (any::<i16>(), any::<i16>()),
        right in (any::<i16>(), prop_oneof![Just(0_i16), any::<i16>()]),
    ) {
        let mut world = World::default();
        let state = world.run(configured_mouse(contexts), Input::Sticks(Sticks { left, right }));
        world.trace.clear();
        let _state = world.run(state, Input::Tick { elapsed: Duration::from_millis(10) });
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::MovePointer { .. })), "normal mode keeps the pointer still");
        let scrolls: Vec<_> = world.trace.iter().filter_map(|effect| {
            if let Effect::Scroll { x, y } = effect { Some((*x, *y)) } else { None }
        }).collect();
        if right.1 == 0 {
            prop_assert!(scrolls.is_empty());
        } else {
            prop_assert_eq!(scrolls.len(), 1);
            prop_assert_eq!(scrolls[0].0, 0);
            prop_assert_eq!(scrolls[0].1.signum(), i32::from(right.1).signum());
        }
    }

    #[test]
    fn right_stick_click_keeps_its_context_binding_while_scrolling(
        mut contexts in arbitrary_contexts(),
        vertical in 1_i16..1001,
    ) {
        contexts.insert(0, tehuti_protocol::parse_context("app_id = 'mouse-test'\n[buttons.right_stick]\nkey = 'tab'").unwrap());
        let mut world = World::default();
        let state = world.run(configured_mouse(contexts), Input::Sticks(Sticks { left: (0, 0), right: (0, vertical) }));
        let state = world.run(state, Input::Tick { elapsed: Duration::from_millis(10) });
        prop_assert!(world.trace.iter().any(|effect| matches!(effect, Effect::Scroll { .. })), "vertical deflection scrolls");
        world.trace.clear();
        let state = world.run(state, pad_button(ButtonName::RightStick, ButtonState::Pressed));
        let _state = world.run(state, pad_button(ButtonName::RightStick, ButtonState::Released));
        prop_assert!(world.trace.contains(&Effect::Apply(Action::Key { key: tehuti_protocol::KeyName::Tab, mods: Vec::new() })), "stick click sends the context key");
    }
}

proptest! {
    #[test]
    fn record_starts_and_transcribes_capture_in_mouse_mode(
        contexts in arbitrary_contexts(),
        plugin_active in prop::bool::ANY,
    ) {
        let mut world = World { plugin_active, ..World::default() };
        let state = world.run(configured_mouse(contexts), Input::Speech(SpeechInput::Available));
        let state = world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed));
        let state = finish_view_double_tap(&mut world, state);
        let state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
        world.trace.clear();
        let state = world.run(state, pad_button(ButtonName::Record, ButtonState::Pressed));
        let _state = world.run(state, pad_button(ButtonName::Record, ButtonState::Released));
        let commands: Vec<_> = world.trace.iter().filter_map(|effect| {
            if let Effect::SpeechCommand(command) = effect { Some(command.clone()) } else { None }
        }).collect();
        prop_assert_eq!(commands, vec![SpeechEffect::Start { session: 0 }, SpeechEffect::Stop { session: 0, transcribe: true }]);
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::Click { .. })), "Record never clicks");
    }

    #[test]
    fn guide_is_reserved_and_start_routes_in_mouse_mode(
        contexts in arbitrary_contexts(),
        plugin_active in prop::bool::ANY,
        name in prop::sample::select(vec![ButtonName::Guide, ButtonName::Start]),
    ) {
        let mut world = World { plugin_active, ..World::default() };
        let state = world.run(configured_mouse(contexts), pad_button(ButtonName::Select, ButtonState::Pressed));
        let state = finish_view_double_tap(&mut world, state);
        let state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
        world.trace.clear();
        let state = world.run(state, pad_button(name, ButtonState::Pressed));
        let _state = world.run(state, pad_button(name, ButtonState::Released));
        let buttons: Vec<_> = world.trace.iter().filter_map(|effect| {
            if let Effect::QueryPlugin(event @ Event::Button { .. }) = effect { Some(event.clone()) } else { None }
        }).collect();
        let expected = if name == ButtonName::Guide { vec![] } else { vec![Event::Button { name, state: ButtonState::Pressed }, Event::Button { name, state: ButtonState::Released }] };
        prop_assert_eq!(buttons, expected);
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::Click { .. })), "Guide and Start never click");
    }
}

#[test]
fn entering_mouse_mode_cancels_recording_and_a_later_release_does_not_restart_it() {
    let mut world = World::default();
    let state = world.run(State::default(), Input::Speech(SpeechInput::Available));
    let state = world.run(state, pad_button(ButtonName::Record, ButtonState::Pressed));
    let state = world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed));
    world.trace.clear();
    let state = finish_view_double_tap(&mut world, state);
    assert!(
        world.trace.iter().any(|effect| matches!(
            effect,
            Effect::SpeechCommand(SpeechEffect::Stop {
                transcribe: false,
                ..
            })
        )),
        "entering mouse mode stops capture"
    );
    world.trace.clear();
    let _state = world.run(state, pad_button(ButtonName::Record, ButtonState::Released));
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::SpeechCommand(_))),
        "release after cancellation cannot restart capture"
    );
}

#[test]
fn escape_still_quits_when_view_toggles_mouse_mode_during_the_chord() {
    let mut world = World::default();
    let mut state = State::default();
    state = world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed));
    state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
    for name in [ButtonName::Select, ButtonName::Guide, ButtonName::Start] {
        state = world.run(state, pad_button(name, ButtonState::Pressed));
    }
    let _state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_secs(3),
        },
    );
    assert!(
        world.trace.contains(&Effect::MouseMode(true)),
        "double tapping View toggles mouse mode"
    );
    assert!(
        world.trace.contains(&Effect::Apply(Action::Quit)),
        "escape stays available"
    );
}

#[test]
fn escape_buttons_remain_reserved_inside_mouse_mode() {
    let mut world = World::default();
    let state = world.run(
        State::default(),
        pad_button(ButtonName::Select, ButtonState::Pressed),
    );
    let state = finish_view_double_tap(&mut world, state);
    let mut state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
    world.trace.clear();
    for name in [ButtonName::Guide, ButtonName::Start] {
        state = world.run(state, pad_button(name, ButtonState::Pressed));
    }
    let _state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_secs(3),
        },
    );
    assert_eq!(
        world
            .trace
            .iter()
            .filter(|effect| matches!(effect, Effect::Apply(_)))
            .collect::<Vec<_>>(),
        vec![&Effect::Apply(Action::Quit)],
        "escape never sends keys to the app"
    );
}

proptest! {
    #[test]
    fn double_tap_releases_never_send_the_normal_mapping(
        mut contexts in arbitrary_contexts(),
        taps in 2_usize..8,
        held in 0_u64..=150,
        gap in 0_u64..=150,
    ) {
        contexts.insert(0, tehuti_protocol::parse_context("app_id = 'mouse-test'\n[buttons.select]\nkey = 'tab'").unwrap());
        let mut world = World::default();
        let mut state = configured_mouse(contexts);
        for _ in 0..taps {
            state = world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed));
            state = world.run(state, Input::Tick { elapsed: Duration::from_millis(held) });
            state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
            state = world.run(state, Input::Tick { elapsed: Duration::from_millis(gap) });
            state = world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed));
            state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
            state = world.run(state, Input::Tick { elapsed: Duration::from_secs(1) });
            prop_assert!(!world.trace.iter().any(|effect| matches!(effect,
                Effect::Apply(_) | Effect::QueryPlugin(Event::Button { name: ButtonName::Select, .. })
                | Effect::DispatchPlugin { event: Event::Button { name: ButtonName::Select, .. }, .. }
            )), "a toggle consumes View through release");
        }
    }

    #[test]
    fn mouse_mode_preserves_escape_with_any_context_and_plugin(
        contexts in arbitrary_contexts(),
        plugin_active in prop::bool::ANY,
    ) {
        let mut world = World { plugin_active, ..World::default() };
        let state = world.run(configured_mouse(contexts), pad_button(ButtonName::Select, ButtonState::Pressed));
        let state = finish_view_double_tap(&mut world, state);
        let mut state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
        world.trace.clear();
        for name in [ButtonName::Guide, ButtonName::Start] {
            state = world.run(state, pad_button(name, ButtonState::Pressed));
        }
        let _state = world.run(state, Input::Tick { elapsed: Duration::from_secs(3) });
        prop_assert!(world.trace.contains(&Effect::Apply(Action::Quit)), "escape remains reachable");
    }
}

#[test]
fn guide_tap_switches_level_inside_mouse_mode() {
    let mut world = World::default();
    let mut state = State::default();
    for button in [
        ButtonState::Pressed,
        ButtonState::Released,
        ButtonState::Pressed,
        ButtonState::Released,
    ] {
        state = world.run(state, pad_button(ButtonName::Select, button));
    }
    assert!(world.trace.contains(&Effect::MouseMode(true)));
    for button in [ButtonState::Pressed, ButtonState::Released] {
        state = world.run(state, pad_button(ButtonName::Guide, button));
    }
    assert_eq!(state.core().level(), crate::Level::Columns);
}

fn speech_event(text: &str) -> Input {
    Input::Event {
        event: Event::Speech {
            text: Text::new(text.into()).unwrap(),
        },
        elapsed: Duration::ZERO,
    }
}

fn tap_button(world: &mut World, state: State, name: ButtonName) -> State {
    let state = world.run(state, pad_button(name, ButtonState::Pressed));
    world.run(state, pad_button(name, ButtonState::Released))
}

fn with_open_keyboard(world: &mut World, state: State) -> State {
    let state = world.run(state, speech_event("keyboard"));
    assert!(state.keyboard().is_open());
    state
}

fn every_button() -> impl Strategy<Value = Event> {
    (
        prop::sample::select(vec![
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
        ]),
        prop::bool::ANY,
    )
        .prop_map(|(name, pressed)| Event::Button {
            name,
            state: if pressed {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            },
        })
}

proptest! {
    #[test]
    fn only_saying_keyboard_opens_the_keyboard(
        events in prop::collection::vec(prop_oneof![every_button(), arbitrary_event()], 0..60),
        sticks in prop::collection::vec(any::<(i16, i16, i16, i16)>(), 0..10),
        contexts in arbitrary_contexts(),
    ) {
        let mut world = World::default();
        let app = contexts[0].app_id().to_owned();
        let mut state = configured(contexts, &app);
        for (left_x, left_y, right_x, right_y) in sticks {
            state = world.run(state, Input::Sticks(Sticks { left: (left_x, left_y), right: (right_x, right_y) }));
        }
        for event in events {
            let opens = matches!(&event, Event::Speech { text } if crate::keyboard::is_open_phrase(text.as_str()));
            state = world.run(state, Input::Event { event, elapsed: Duration::from_millis(16) });
            prop_assert!(opens || !state.keyboard().is_open());
            if state.keyboard().is_open() {
                break;
            }
        }
        state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Released));
        if state.core().menu().is_some() {
            state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Pressed));
            state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Released));
        }
        let state = world.run(state, speech_event("keyboard"));
        prop_assert!(state.keyboard().is_open());
    }

    #[test]
    fn speech_while_open_is_dictation_and_never_a_command(
        contexts in arbitrary_contexts(),
        phrase in "[a-z]{1,8}( [a-z]{1,8}){0,3}",
    ) {
        let mut world = World::default();
        let app = contexts[0].app_id().to_owned();
        let state = with_open_keyboard(&mut world, configured(contexts, &app));
        world.trace.clear();
        let state = world.run(state, speech_event(&phrase));
        prop_assert_eq!(state.keyboard().draft(), phrase.as_str());
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::Apply(_) | Effect::QueryPlugin(Event::Speech { .. }) | Effect::SpeechRouted { .. })), "dictation never routes");
    }

    #[test]
    fn the_open_keyboard_keeps_its_buttons_from_apps_and_plugins(
        contexts in arbitrary_contexts(),
        name in prop::sample::select(vec![ButtonName::South, ButtonName::East, ButtonName::North, ButtonName::West, ButtonName::DpadUp, ButtonName::DpadLeft, ButtonName::LeftBumper, ButtonName::RightBumper]),
    ) {
        let mut world = World { plugin_active: true, ..World::default() };
        let app = contexts[0].app_id().to_owned();
        let state = with_open_keyboard(&mut world, configured(contexts, &app));
        world.trace.clear();
        let _state = tap_button(&mut world, state, name);
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::Apply(_) | Effect::QueryPlugin(Event::Button { .. }) | Effect::ConfirmSecret(_))), "no app or plugin sees the button");
    }
}

#[test]
fn menu_types_the_draft_then_presses_enter_and_stays_open() {
    let mut world = World::default();
    let state = with_open_keyboard(&mut world, State::default());
    let state = world.run(state, speech_event("see you soon"));
    world.trace.clear();
    let state = tap_button(&mut world, state, ButtonName::Start);
    assert!(state.keyboard().is_open());
    assert_eq!(
        world
            .trace
            .iter()
            .filter(|effect| matches!(effect, Effect::Apply(_) | Effect::Chord(_)))
            .collect::<Vec<_>>(),
        vec![
            &Effect::Apply(Action::Text {
                text: Text::new("see you soon".into()).unwrap()
            }),
            &Effect::Chord(crate::keyboard::Chord {
                key: crate::keyboard::ChordKey::Named(KeyName::Enter),
                modifiers: crate::keyboard::Modifiers::default()
            })
        ]
    );
}

#[test]
fn b_closes_without_sending_anything() {
    let mut world = World::default();
    let state = with_open_keyboard(&mut world, State::default());
    let state = world.run(state, speech_event("draft"));
    world.trace.clear();
    let state = tap_button(&mut world, state, ButtonName::East);
    assert!(!state.keyboard().is_open());
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::Apply(_)))
    );
}

#[test]
fn the_keyboard_opens_and_types_in_mouse_mode() {
    let mut world = World::default();
    let mut state = State::default();
    for button in [
        ButtonState::Pressed,
        ButtonState::Released,
        ButtonState::Pressed,
        ButtonState::Released,
    ] {
        state = world.run(state, pad_button(ButtonName::Select, button));
    }
    assert!(world.trace.contains(&Effect::MouseMode(true)));
    let state = with_open_keyboard(&mut world, state);
    world.trace.clear();
    let state = tap_button(&mut world, state, ButtonName::DpadRight);
    let state = tap_button(&mut world, state, ButtonName::South);
    assert_eq!(state.keyboard().draft(), "w");
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::Click { .. }))
    );
}

#[test]
fn right_stick_still_scrolls_while_the_keyboard_is_open() {
    let mut world = World::default();
    let mut state = State::default();
    for button in [
        ButtonState::Pressed,
        ButtonState::Released,
        ButtonState::Pressed,
        ButtonState::Released,
    ] {
        state = world.run(state, pad_button(ButtonName::Select, button));
    }
    let state = with_open_keyboard(&mut world, state);
    let state = world.run(
        state,
        Input::Sticks(Sticks {
            left: (900, 0),
            right: (0, 800),
        }),
    );
    world.trace.clear();
    let _state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_millis(10),
        },
    );
    assert!(
        world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::Scroll { y, .. } if *y > 0))
    );
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::MovePointer { .. })),
        "the left stick picks words instead of moving the pointer"
    );
}

#[test]
fn guide_tap_switches_level_while_open_and_shoulders_then_move_columns() {
    let mut world = World::default();
    let state = with_open_keyboard(&mut world, State::default());
    let state = tap_button(&mut world, state, ButtonName::Guide);
    assert_eq!(state.core().level(), crate::Level::Columns);
    world.trace.clear();
    let state = tap_button(&mut world, state, ButtonName::RightBumper);
    assert!(state.keyboard().is_open());
    assert!(world.trace.contains(&Effect::Apply(Action::Layout {
        action: tehuti_protocol::LayoutAction::FocusColumnRight
    })));
}

#[test]
fn escape_chord_quits_while_the_keyboard_is_open() {
    let mut world = World::default();
    let mut state = with_open_keyboard(&mut world, State::default());
    state = world.run(state, speech_event("unsent"));
    for name in [ButtonName::Guide, ButtonName::Start] {
        state = world.run(state, pad_button(name, ButtonState::Pressed));
    }
    let _state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_secs(3),
        },
    );
    assert_eq!(
        world
            .trace
            .iter()
            .filter(|effect| matches!(effect, Effect::Apply(_)))
            .collect::<Vec<_>>(),
        vec![&Effect::Apply(Action::Quit)],
        "Start inside the chord never sends the draft"
    );
}

#[test]
fn start_first_escape_chord_quits_without_sending_while_open() {
    let mut world = World::default();
    let mut state = with_open_keyboard(&mut world, State::default());
    state = world.run(state, speech_event("unsent"));
    for name in [ButtonName::Start, ButtonName::Guide] {
        state = world.run(state, pad_button(name, ButtonState::Pressed));
    }
    state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_secs(3),
        },
    );
    let _state = world.run(state, pad_button(ButtonName::Start, ButtonState::Released));
    assert_eq!(
        world
            .trace
            .iter()
            .filter(|effect| matches!(effect, Effect::Apply(_)))
            .collect::<Vec<_>>(),
        vec![&Effect::Apply(Action::Quit)],
        "Start pressed before Guide joins the chord and never sends"
    );
}

proptest! {
    #[test]
    fn field_policy_requires_y_even_with_a_configured_button(
        mode in prop::sample::select(vec!["open", "button", "off"]),
        opening in prop::sample::select(vec![("Y", ButtonName::North), ("west", ButtonName::West)]),
        mouse in prop::bool::ANY,
        events in prop::collection::vec(every_button(), 0..30),
    ) {
        let context = tehuti_protocol::parse_context(&format!(
            "app_id = 'editor'\n[buttons]\n[keyboard]\non_text_field = '{mode}'\nbutton = '{}'", opening.0
        )).unwrap();
        let mut world = World::default();
        let mut state = configured(vec![context], "editor");
        if mouse {
            for _ in 0..2 {
                state = tap_button(&mut world, state, ButtonName::Select);
            }
        }
        state = world.run(state, Input::TextField { active: true, changed: true, kind: crate::keyboard::FieldKind::Normal });
        prop_assert!(!state.keyboard().is_open());
        state = tap_button(&mut world, state, ButtonName::East);
        for event in events {
            let opens = mode != "off" && matches!(event, Event::Button { name, state: ButtonState::Pressed } if name == ButtonName::North);
            state = world.run(state, Input::Event { event, elapsed: Duration::ZERO });
            if state.keyboard().is_open() {
                prop_assert!(opens);
                state = tap_button(&mut world, state, ButtonName::East);
            }
            state = world.run(state, Input::TextField { active: true, changed: false, kind: crate::keyboard::FieldKind::Normal });
            prop_assert!(!state.keyboard().is_open());
        }
        state = world.run(state, Input::TextField { active: true, changed: true, kind: crate::keyboard::FieldKind::Normal });
        prop_assert!(!state.keyboard().is_open());
    }
}

#[test]
fn inactive_fields_pass_the_opening_button_to_the_context() {
    let context =
        tehuti_protocol::parse_context("app_id = 'editor'\n[buttons]\nnorth = { key = 'tab' }").unwrap();
    let mut world = World::default();
    let state = configured(vec![context], "editor");
    let state = tap_button(&mut world, state, ButtonName::North);
    assert!(!state.keyboard().is_open());
    assert!(world.trace.contains(&Effect::Apply(Action::Key {
        key: tehuti_protocol::KeyName::Tab,
        mods: Vec::new()
    })));
}

#[test]
fn application_keyboard_options_inherit_the_global_context() {
    let global = tehuti_protocol::parse_context(
        "app_id = '*'\n[buttons]\n[keyboard]\non_text_field = 'off'\nbutton = 'west'",
    )
    .unwrap();
    let local =
        tehuti_protocol::parse_context("app_id = 'editor'\n[buttons]\n[keyboard]\non_text_field = 'button'")
            .unwrap();
    let state = configured(vec![global, local], "editor");
    assert_eq!(
        state.core().keyboard_config(),
        (tehuti_protocol::OnTextField::Button, ButtonName::West)
    );
}

#[test]
fn password_fields_ignore_record_and_speech_even_with_keyboard_closed() {
    for passthrough in [false, true] {
        let context = tehuti_protocol::parse_context(&format!(
            "app_id = 'org.example.Emulator'\nbuttons = {{}}\n[pad]\npassthrough = {passthrough}"
        ))
        .unwrap();
        password_fields_in_context(&configured(vec![context], "org.example.Emulator"));
    }
}

fn password_fields_in_context(base: &State) {
    for open in [false, true] {
        let mut world = World::default();
        let mut state = world.run(base.clone(), Input::Speech(SpeechInput::Available));
        state = world.run(
            state,
            Input::TextField {
                active: true,
                changed: true,
                kind: crate::keyboard::FieldKind::Password,
            },
        );
        if open {
            state = world.run(state, keyboard_button(ButtonName::North, ButtonState::Pressed));
        }
        world.trace.clear();
        for event in [
            Event::Button {
                name: ButtonName::Record,
                state: ButtonState::Pressed,
            },
            Event::Button {
                name: ButtonName::Record,
                state: ButtonState::Released,
            },
            Event::Speech {
                text: Text::new("keyboard".into()).unwrap(),
            },
        ] {
            state = world.run(
                state,
                Input::Event {
                    event,
                    elapsed: Duration::ZERO,
                },
            );
        }
        assert_eq!(state.keyboard().is_open(), open && !state.core().passthrough());
        assert!(state.keyboard().password_field_active());
        assert_eq!(state.keyboard().draft(), "");
        assert_eq!(
            world
                .trace
                .iter()
                .filter(|effect| **effect == Effect::PasswordSpeechIgnored)
                .count(),
            3
        );
        assert!(!world.trace.iter().any(|effect| matches!(
            effect,
            Effect::SpeechCommand(_)
                | Effect::Apply(_)
                | Effect::DispatchPlugin {
                    event: Event::Speech { .. },
                    ..
                }
        )));
    }
}

fn keyboard_button(name: ButtonName, state: ButtonState) -> Input {
    Input::Event {
        event: Event::Button { name, state },
        elapsed: Duration::ZERO,
    }
}

#[test]
fn entering_password_field_cancels_capture_without_transcribing() {
    let mut world = World::default();
    let mut state = State::default();
    for input in [SpeechInput::Available, SpeechInput::Press] {
        state = world.run(state, Input::Speech(input));
    }
    world.trace.clear();
    state = world.run(
        state,
        Input::TextField {
            active: true,
            changed: true,
            kind: crate::keyboard::FieldKind::Password,
        },
    );
    assert!(world.trace.contains(&Effect::SpeechCommand(SpeechEffect::Stop {
        session: 0,
        transcribe: false,
    })));
    state = world.run(
        state,
        Input::TextField {
            active: false,
            changed: false,
            kind: crate::keyboard::FieldKind::Normal,
        },
    );
    assert!(!state.keyboard().password_field_active());
}

fn terminal_by_context(open_on_app_level: Option<bool>) -> State {
    let option = open_on_app_level.map_or_else(String::new, |opens| format!("open_on_app_level = {opens}\n"));
    let context = tehuti_protocol::parse_context(&format!(
        "app_id = 'console'\n[buttons]\n[keyboard]\nterminal = true\non_text_field = 'open'\n{option}"
    ))
    .unwrap();
    configured(vec![context], "console")
}

fn terminal_field() -> Input {
    Input::TextField {
        active: true,
        changed: true,
        kind: crate::keyboard::FieldKind::Terminal,
    }
}

fn applied(world: &World) -> Vec<&Effect> {
    world
        .trace
        .iter()
        .filter(|effect| matches!(effect, Effect::Apply(_)))
        .collect()
}

proptest! {
    #[test]
    fn focusing_a_terminal_or_its_field_never_opens_the_keyboard(
        mode in prop::sample::select(vec!["open", "button", "off"]),
        by_context in prop::bool::ANY,
        events in prop::collection::vec(every_button(), 0..30),
    ) {
        let terminal = if by_context { "terminal = true\n" } else { "" };
        let context = tehuti_protocol::parse_context(&format!(
            "app_id = 'console'\n[buttons]\n[keyboard]\non_text_field = '{mode}'\n{terminal}"
        )).unwrap();
        let mut world = World::default();
        let mut state = configured(vec![context], "console");
        state = world.run(state, Input::TextField {
            active: true,
            changed: true,
            kind: if by_context { crate::keyboard::FieldKind::Normal } else { crate::keyboard::FieldKind::Terminal },
        });
        prop_assert!(!state.keyboard().is_open());
        for event in events {
            let opens = mode != "off" && matches!(event, Event::Button { name: ButtonName::North, state: ButtonState::Pressed });
            state = world.run(state, Input::Event { event, elapsed: Duration::ZERO });
            if opens {
                break;
            }
            prop_assert!(!state.keyboard().is_open(), "only Y opens it in a terminal");
        }
    }
}

#[test]
fn a_guide_tap_into_app_level_never_opens_the_terminal_keyboard() {
    for state in [
        terminal_by_context(None),
        World::default().run(State::default(), terminal_field()),
    ] {
        let mut world = World::default();
        let state = tap_button(&mut world, state, ButtonName::Guide);
        assert_eq!(state.core().level(), crate::Level::Columns);
        assert!(!state.keyboard().is_open());
        let state = tap_button(&mut world, state, ButtonName::Guide);
        assert!(!state.keyboard().is_open());
        assert!(state.keyboard().terminal());
    }
}

#[test]
fn a_context_can_keep_the_guide_tap_from_opening_the_terminal_keyboard() {
    let mut world = World::default();
    let mut state = terminal_by_context(Some(false));
    for _ in 0..2 {
        state = tap_button(&mut world, state, ButtonName::Guide);
    }
    assert_eq!(state.core().level(), crate::Level::App);
    assert!(!state.keyboard().is_open());
}

#[test]
fn guide_taps_outside_terminals_never_open_the_keyboard() {
    let mut world = World::default();
    let mut state = State::default();
    for _ in 0..2 {
        state = tap_button(&mut world, state, ButtonName::Guide);
    }
    assert!(!state.keyboard().is_open());
}

#[test]
fn b_closes_the_keyboard_in_terminals_and_other_applications() {
    for state in [terminal_by_context(None), State::default()] {
        let mut world = World::default();
        let state = with_open_keyboard(&mut world, state);
        world.trace.clear();
        let state = tap_button(&mut world, state, ButtonName::East);
        assert!(!state.keyboard().is_open());
        assert_eq!(applied(&world), Vec::<&Effect>::new());
    }
}

#[test]
fn terminal_strip_keys_send_the_draft_then_the_key() {
    let mut world = World::default();
    let mut state = with_open_keyboard(&mut world, terminal_by_context(None));
    state = world.run(state, speech_event("ls"));
    state = tap_button(&mut world, state, ButtonName::DpadUp);
    state = tap_button(&mut world, state, ButtonName::DpadLeft);
    world.trace.clear();
    let state = tap_button(&mut world, state, ButtonName::South);
    assert!(state.keyboard().is_open());
    assert_eq!(state.keyboard().draft(), "");
    assert_eq!(
        applied(&world),
        vec![&Effect::Apply(Action::Text {
            text: Text::new("ls".into()).unwrap()
        })]
    );
    assert!(world.trace.contains(&Effect::Chord(crate::keyboard::Chord {
        key: crate::keyboard::ChordKey::Named(tehuti_protocol::KeyName::Tab),
        modifiers: crate::keyboard::Modifiers::default(),
    })));
}

#[test]
fn dictation_into_a_terminal_drops_commas_and_full_stops() {
    let mut world = World::default();
    let state = with_open_keyboard(&mut world, terminal_by_context(None));
    let state = world.run(state, speech_event("Claude, --model, sonnet. notes.txt"));
    assert_eq!(state.keyboard().draft(), "Claude --model sonnet notes.txt");
}

#[test]
fn typed_speech_into_a_terminal_drops_commas_and_full_stops() {
    let context = tehuti_protocol::parse_context(
        "app_id = 'console'\nunmatched_speech = 'type'\n[buttons]\n[keyboard]\nterminal = true",
    )
    .unwrap();
    let mut world = World::default();
    let state = configured(vec![context], "console");
    let _state = world.run(state, speech_event("git, status."));
    assert_eq!(
        applied(&world),
        vec![&Effect::Apply(Action::Text {
            text: Text::new("git status ".into()).unwrap()
        })]
    );
}

proptest! {
    #[test]
    fn guide_gestures_stay_above_contexts_and_plugins_in_every_mode(
        contexts in arbitrary_contexts(),
        plugin_active in any::<bool>(),
    ) {
        for mode in 0..3 {
            for millis in [499, 500] {
                let mut world = World { plugin_active, ..World::default() };
                let app = contexts[0].app_id().to_owned();
                let mut state = configured(contexts.clone(), &app);
                if mode == 1 {
                    state = tap_button(&mut world, state, ButtonName::Select);
                    state = tap_button(&mut world, state, ButtonName::Select);
                    prop_assert!(world.trace.contains(&Effect::MouseMode(true)));
                } else if mode == 2 {
                    state = with_open_keyboard(&mut world, state);
                }
                world.trace.clear();
                state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Pressed));
                state = world.run(state, Input::Tick { elapsed: Duration::from_millis(millis) });
                state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Released));
                if millis == 499 {
                    prop_assert_eq!(state.core().level(), crate::Level::Columns);
                    prop_assert!(state.core().menu().is_none());
                } else {
                    prop_assert_eq!(state.core().level(), crate::Level::App);
                    prop_assert!(state.core().menu().is_some());
                    state = tap_button(&mut world, state, ButtonName::Guide);
                    prop_assert!(state.core().menu().is_none());
                    prop_assert_eq!(state.core().level(), crate::Level::App);
                }
                prop_assert_eq!(state.keyboard().is_open(), mode == 2);
                prop_assert!(!world.trace.iter().any(|effect| matches!(effect,
                    Effect::Apply(_) | Effect::SettingsMode(_)
                    | Effect::PersistMode(_)
                    | Effect::RunMenuCommand(_)
                    | Effect::Click { .. } | Effect::DispatchPlugin { event: Event::Button { .. }, .. }
                )), "Guide gestures never reach applications or plugins");
            }
        }
    }
}

proptest! {
    #[test]
    fn menu_reachable_and_escapable_in_every_mode(
        contexts in arbitrary_contexts(),
        plugin_active in any::<bool>(),
        mode in 0u8..3,
        close_with_guide in any::<bool>(),
        held in prop::sample::select(vec![ButtonName::DpadDown, ButtonName::Record, ButtonName::South]),
    ) {
        let mut world = World { plugin_active, ..World::default() };
        let app = contexts[0].app_id().to_owned();
        let mut state = configured(contexts, &app);
        if mode == 1 {
            for button in [ButtonState::Pressed, ButtonState::Released, ButtonState::Pressed, ButtonState::Released] {
                state = world.run(state, pad_button(ButtonName::Select, button));
            }
        } else if mode == 2 {
            state = world.run(state, Input::TextField {
                active: true, changed: true, kind: crate::keyboard::FieldKind::Terminal,
            });
            state = world.run(state, speech_event("keyboard"));
            prop_assert!(state.keyboard().is_open());
        }
        state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Pressed));
        state = world.run(state, Input::Tick { elapsed: Duration::from_millis(500) });
        prop_assert!(state.core().menu().is_some());
        prop_assert_eq!(state.core().level(), crate::Level::App);
        state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Released));
        world.trace.clear();
        state = world.run(state, pad_button(held, ButtonState::Pressed));
        let close = if close_with_guide { ButtonName::Guide } else { ButtonName::East };
        state = world.run(state, pad_button(close, ButtonState::Pressed));
        state = world.run(state, pad_button(close, ButtonState::Released));
        state = world.run(state, pad_button(held, ButtonState::Released));
        prop_assert!(state.core().menu().is_none());
        prop_assert_eq!(state.core().level(), crate::Level::App);
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect,
            Effect::Apply(_) | Effect::SettingsMode(_)
                    | Effect::PersistMode(_)
                    | Effect::RunMenuCommand(_)
            | Effect::Click { .. } | Effect::DispatchPlugin { event: Event::Button { .. }, .. }
        )), "closing the menu never reaches applications or plugins");
    }

    #[test]
    fn emergency_escape_works_without_drawing_or_plugin_replies(
        contexts in arbitrary_contexts(),
        plugin_active in any::<bool>(),
    ) {
        let mut world = World { plugin_active, ..World::default() };
        let app = contexts[0].app_id().to_owned();
        let mut state = configured(contexts, &app);
        state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Pressed));
        state = world.run(state, Input::Tick { elapsed: Duration::from_millis(500) });
        prop_assert!(state.core().menu().is_some());
        state = world.run(state, pad_button(ButtonName::Start, ButtonState::Pressed));
        world.trace.clear();
        state = world.run(state, Input::Tick { elapsed: Duration::from_millis(2999) });
        prop_assert!(!world.trace.contains(&Effect::Apply(Action::Quit)));
        let _state = world.run(state, Input::Tick { elapsed: Duration::from_millis(1) });
        prop_assert!(world.trace.contains(&Effect::Apply(Action::Quit)));
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::SettingsMode(_)
                    | Effect::PersistMode(_)
                    | Effect::RunMenuCommand(_))));
    }
}

fn settings_event(world: &mut World, state: State, event: Event) -> State {
    world.run(
        state,
        Input::Event {
            event,
            elapsed: Duration::ZERO,
        },
    )
}

fn open_display(world: &mut World) -> State {
    let state = world.run(
        State::default(),
        Input::DisplayModes {
            modes: vec![
                "1920x1080@60".into(),
                "3840x2160@60".into(),
                "3840x2160@120".into(),
            ],
            current: 0,
        },
    );
    let state = settings_event(
        world,
        state,
        Event::Speech {
            text: Text::new("Pop the hood.".into()).unwrap(),
        },
    );
    settings_button(world, state, ButtonName::South)
}

fn settings_button(world: &mut World, state: State, name: ButtonName) -> State {
    let state = settings_event(
        world,
        state,
        Event::Button {
            name,
            state: ButtonState::Released,
        },
    );
    settings_event(
        world,
        state,
        Event::Button {
            name,
            state: ButtonState::Pressed,
        },
    )
}

#[test]
fn settings_navigation_is_bounded_and_consumes_client_input() {
    let mut world = World::default();
    let mut state = open_display(&mut world);
    assert!(state.settings().open);
    assert_eq!(state.settings().page, Page::Display);
    world.trace.clear();
    for _ in 0..5 {
        state = settings_button(&mut world, state, ButtonName::DpadDown);
    }
    assert_eq!(state.settings().selected, 3);
    state = settings_button(&mut world, state, ButtonName::DpadLeft);
    assert_eq!(state.settings().selected, 3);
    assert_eq!(state.settings().scale.get(), 1);
    state = settings_button(&mut world, state, ButtonName::DpadUp);
    assert_eq!(state.settings().selected, 2);
    state = settings_button(&mut world, state, ButtonName::DpadLeft);
    assert_eq!(state.settings().selected, 1);
    for _ in 0..5 {
        state = settings_button(&mut world, state, ButtonName::DpadUp);
    }
    assert_eq!(state.settings().selected, 0);
    state = settings_button(&mut world, state, ButtonName::DpadRight);
    assert_eq!(state.settings().selected, 1);
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::Apply(_) | Effect::ConfirmSecret(_)))
    );
    state = settings_button(&mut world, state, ButtonName::East);
    assert!(state.settings().open);
    assert_eq!(state.settings().page, Page::Categories);
    state = settings_button(&mut world, state, ButtonName::East);
    assert!(!state.settings().open);
}

#[test]
fn successful_mode_apply_starts_countdown_and_south_confirms_once() {
    let mut world = World::default();
    let state = open_display(&mut world);
    let state = settings_button(&mut world, state, ButtonName::DpadDown);
    world.trace.clear();
    let state = settings_button(&mut world, state, ButtonName::South);
    assert!(world.trace.contains(&Effect::SettingsMode(1)));
    assert_eq!(state.settings().remaining, None);
    world.trace.clear();
    let state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_secs(20),
        },
    );
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::SettingsMode(_) | Effect::PersistMode(_)))
    );
    let state = world.run(state, Input::ModeApplied(Some(1)));
    assert_eq!(state.settings().current, 1);
    assert_eq!(state.settings().remaining, Some(Duration::from_secs(15)));
    let state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_secs(14),
        },
    );
    assert_eq!(state.settings().remaining, Some(Duration::from_secs(1)));
    world.trace.clear();
    let state = settings_button(&mut world, state, ButtonName::South);
    assert!(world.trace.contains(&Effect::PersistMode(1)));
    assert_eq!(state.settings().remaining, None);
    world.trace.clear();
    let state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_secs(20),
        },
    );
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::SettingsMode(_) | Effect::PersistMode(_)))
    );
    assert_eq!(state.settings().current, 1);
}

#[test]
fn unconfirmed_mode_reverts_at_fifteen_seconds_without_persisting() {
    for final_tick in [Duration::from_secs(1), Duration::from_secs(20)] {
        let mut world = World::default();
        let state = open_display(&mut world);
        let state = settings_button(&mut world, state, ButtonName::DpadDown);
        let state = settings_button(&mut world, state, ButtonName::South);
        let state = world.run(state, Input::ModeApplied(Some(1)));
        world.trace.clear();
        let state = world.run(
            state,
            Input::Tick {
                elapsed: Duration::from_secs(14),
            },
        );
        assert!(!world.trace.contains(&Effect::SettingsMode(0)));
        let state = world.run(state, Input::Tick { elapsed: final_tick });
        assert!(world.trace.contains(&Effect::SettingsMode(0)));
        assert!(
            !world
                .trace
                .iter()
                .any(|effect| matches!(effect, Effect::PersistMode(_)))
        );
        let state = world.run(state, Input::ModeApplied(Some(0)));
        assert_eq!(state.settings().current, 0);
        assert_eq!(state.settings().remaining, None);
    }
}

#[test]
fn rejected_modes_keep_current_choice_and_do_not_offer_confirmation() {
    let mut world = World::default();
    let state = open_display(&mut world);
    let state = settings_button(&mut world, state, ButtonName::DpadDown);
    let state = settings_button(&mut world, state, ButtonName::South);
    let state = world.run(state, Input::ModeApplied(None));
    assert_eq!(state.settings().current, 0);
    assert!(state.settings().failed());
    assert_eq!(state.settings().remaining, None);
    world.trace.clear();
    let _state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_secs(30),
        },
    );
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::SettingsMode(_) | Effect::PersistMode(_)))
    );
}

#[test]
fn east_and_the_global_phrase_cancel_mode_preview() {
    for event in [
        Event::Button {
            name: ButtonName::East,
            state: ButtonState::Pressed,
        },
        Event::Speech {
            text: Text::new("POP THE HOOD".into()).unwrap(),
        },
    ] {
        let mut world = World::default();
        let state = open_display(&mut world);
        let state = settings_button(&mut world, state, ButtonName::DpadDown);
        let state = settings_button(&mut world, state, ButtonName::South);
        let state = world.run(state, Input::ModeApplied(Some(1)));
        world.trace.clear();
        let backs = matches!(event, Event::Button { .. });
        let state = settings_event(&mut world, state, event.clone());
        assert!(world.trace.contains(&Effect::SettingsMode(0)));
        assert_eq!(state.settings().open, backs);
        assert_eq!(state.settings().page, Page::Categories);
        assert_eq!(state.settings().remaining, None);
    }
}

#[test]
fn closing_during_apply_reverts_after_the_backend_reports_success() {
    let mut world = World::default();
    let state = open_display(&mut world);
    let state = settings_button(&mut world, state, ButtonName::DpadDown);
    let state = settings_button(&mut world, state, ButtonName::South);
    let state = settings_event(
        &mut world,
        state,
        Event::Speech {
            text: Text::new("Pop the hood.".into()).unwrap(),
        },
    );
    assert!(!state.settings().open);
    world.trace.clear();
    let state = world.run(state, Input::ModeApplied(Some(1)));
    assert!(world.trace.contains(&Effect::SettingsMode(0)));
    assert_eq!(state.settings().remaining, None);
    let state = world.run(state, Input::ModeApplied(Some(0)));
    assert_eq!(state.settings().current, 0);
}

#[test]
fn confirmation_at_the_deadline_cannot_persist_the_expired_preview() {
    let mut world = World::default();
    let state = open_display(&mut world);
    let state = settings_button(&mut world, state, ButtonName::DpadDown);
    let state = settings_button(&mut world, state, ButtonName::South);
    let state = world.run(state, Input::ModeApplied(Some(1)));
    world.trace.clear();
    let _state = world.run(
        state,
        Input::Event {
            event: Event::Button {
                name: ButtonName::South,
                state: ButtonState::Pressed,
            },
            elapsed: Duration::from_secs(15),
        },
    );
    assert!(world.trace.contains(&Effect::SettingsMode(0)));
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::PersistMode(_)))
    );
}

#[test]
fn settings_button_hints_name_a_and_b_through_navigation_and_confirmation() {
    let mut world = World::default();
    let state = open_display(&mut world);
    assert_eq!(state.settings().button_hint(), "D-PAD: MOVE   A: APPLY   B: BACK");
    let state = settings_button(&mut world, state, ButtonName::DpadDown);
    let state = settings_button(&mut world, state, ButtonName::South);
    let state = world.run(state, Input::ModeApplied(Some(1)));
    assert_eq!(state.settings().button_hint(), "A: KEEP THIS MODE");
    let state = settings_button(&mut world, state, ButtonName::South);
    assert_eq!(state.settings().button_hint(), "D-PAD: MOVE   A: APPLY   B: BACK");
    let state = settings_button(&mut world, state, ButtonName::East);
    assert_eq!(state.settings().button_hint(), "D-PAD: MOVE   A: OPEN   B: CLOSE");
}

proptest! {
    #[test]
    fn settings_keeps_speech_commands_and_transcript_mapping(plugin_active in prop::bool::ANY) {
        let context = tehuti_protocol::parse_context("app_id = 'voice-test'\n[buttons]\n[speech.screenshot]\nnotify = 'capture'").unwrap();
        let mut world = World { plugin_active, ..World::default() };
        let state = configured(vec![context], "voice-test");
        let state = settings_event(&mut world, state, Event::Speech { text: Text::new("pop the hood".into()).unwrap() });
        let state = world.run(state, Input::Speech(SpeechInput::Available));
        let state = settings_button(&mut world, state, ButtonName::Record);
        let state = world.run(state, pad_button(ButtonName::Record, ButtonState::Released));
        prop_assert!(world.trace.contains(&Effect::SpeechCommand(SpeechEffect::Stop { session: 0, transcribe: true })), "record transcribes while settings is open");
        world.trace.clear();
        let text = Text::new("Screenshot.".into()).unwrap();
        let state = world.run(state, Input::Speech(SpeechInput::Transcript { session: 0, text: text.clone() }));
        let expected = Action::Notice { text: Text::new("capture".into()).unwrap() };
        prop_assert!(state.settings().open);
        prop_assert!(world.trace.contains(&Effect::TranscriptReceived { session: 0, text: text.clone(), admitted: true }), "transcript is admitted");
        prop_assert!(world.trace.contains(&Effect::SpeechRouted { text, first: Some(expected.clone()), plugin: plugin_active }), "transcript is mapped while settings is open");
        prop_assert!(world.trace.contains(&Effect::Apply(expected)));
    }
}

#[test]
fn settings_confirmation_requires_a_release_after_the_apply_press() {
    let mut world = World::default();
    let state = open_display(&mut world);
    let state = settings_button(&mut world, state, ButtonName::DpadDown);
    let state = settings_button(&mut world, state, ButtonName::South);
    let mut state = world.run(state, Input::ModeApplied(Some(1)));
    world.trace.clear();
    for _ in 0..3 {
        state = settings_event(
            &mut world,
            state,
            Event::Button {
                name: ButtonName::South,
                state: ButtonState::Pressed,
            },
        );
    }
    assert_eq!(state.settings().remaining, Some(Duration::from_secs(15)));
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::PersistMode(_)))
    );
    let _state = settings_button(&mut world, state, ButtonName::South);
    assert!(world.trace.contains(&Effect::PersistMode(1)));
}

#[test]
fn system_menu_settings_entry_opens_the_view_without_a_shell_command() {
    let mut world = World::default();
    let mut state = world.run(
        State::default(),
        pad_button(ButtonName::Guide, ButtonState::Pressed),
    );
    state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_millis(500),
        },
    );
    state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Released));
    state = tap_button(&mut world, state, ButtonName::DpadUp);
    world.trace.clear();
    state = tap_button(&mut world, state, ButtonName::South);
    assert!(state.settings().open);
    assert!(state.core().menu().is_none());
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::RunMenuCommand(_)))
    );
}

#[test]
fn guide_closes_settings_and_preserves_menu_and_emergency_escape() {
    let mut world = World::default();
    let state = open_display(&mut world);
    let mut state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Pressed));
    assert!(!state.settings().open);
    state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_millis(500),
        },
    );
    assert!(state.core().menu().is_some());
    state = world.run(state, pad_button(ButtonName::Start, ButtonState::Pressed));
    world.trace.clear();
    state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_millis(2999),
        },
    );
    assert!(!world.trace.contains(&Effect::Apply(Action::Quit)));
    let _state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_millis(1),
        },
    );
    assert!(world.trace.contains(&Effect::Apply(Action::Quit)));
}

#[test]
fn b_backs_out_of_settings_without_forwarding_its_release_to_the_application() {
    let mut world = World::default();
    let state = open_display(&mut world);
    world.trace.clear();
    let state = tap_button(&mut world, state, ButtonName::East);
    assert!(state.settings().open);
    let state = tap_button(&mut world, state, ButtonName::East);
    assert!(!state.settings().open);
    assert!(!world.trace.iter().any(|effect| matches!(
        effect,
        Effect::Apply(_)
            | Effect::DispatchPlugin {
                event: Event::Button { .. },
                ..
            }
    )));
}

#[test]
fn emergency_escape_survives_switching_and_the_confirmation_countdown() {
    for applied in [false, true] {
        let mut world = World::default();
        let state = open_display(&mut world);
        let state = settings_button(&mut world, state, ButtonName::DpadDown);
        let mut state = settings_button(&mut world, state, ButtonName::South);
        if applied {
            state = world.run(state, Input::ModeApplied(Some(1)));
            assert_eq!(state.settings().remaining, Some(Duration::from_secs(15)));
        }
        assert!(state.settings().open);
        state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Pressed));
        state = world.run(state, pad_button(ButtonName::Start, ButtonState::Pressed));
        world.trace.clear();
        state = world.run(
            state,
            Input::Tick {
                elapsed: Duration::from_millis(2999),
            },
        );
        assert!(!world.trace.contains(&Effect::Apply(Action::Quit)));
        let _state = world.run(
            state,
            Input::Tick {
                elapsed: Duration::from_millis(1),
            },
        );
        assert!(world.trace.contains(&Effect::Apply(Action::Quit)));
    }
}

#[test]
fn south_held_before_opening_cannot_enter_display_until_released() {
    let mut world = World::default();
    let state = world.run(
        State::default(),
        pad_button(ButtonName::South, ButtonState::Pressed),
    );
    let state = settings_event(
        &mut world,
        state,
        Event::Speech {
            text: Text::new("Pop the hood".into()).unwrap(),
        },
    );
    let state = world.run(state, pad_button(ButtonName::South, ButtonState::Pressed));
    assert!(state.settings().open);
    assert_eq!(state.settings().page, Page::Categories);
    let state = settings_button(&mut world, state, ButtonName::South);
    assert_eq!(state.settings().page, Page::Display);
}

#[test]
fn open_settings_takes_only_its_voice_phrase_from_contexts_and_plugins() {
    for plugin_active in [false, true] {
        let context = tehuti_protocol::parse_context(
            "app_id = 'example-app'\n[buttons]\n[speech.screenshot]\nnotify = 'capture'",
        )
        .unwrap();
        let mut world = World {
            plugin_active,
            ..World::default()
        };
        let state = configured(vec![context], "example-app");
        let state = settings_event(
            &mut world,
            state,
            Event::Speech {
                text: Text::new("Pop the hood".into()).unwrap(),
            },
        );
        assert!(state.settings().open);
        assert!(
            !world
                .trace
                .iter()
                .any(|effect| matches!(effect, Effect::SpeechRouted { .. }))
        );
        world.trace.clear();
        let text = Text::new("Screenshot.".into()).unwrap();
        let state = settings_event(&mut world, state, Event::Speech { text: text.clone() });
        let expected = Action::Notice {
            text: Text::new("capture".into()).unwrap(),
        };
        assert!(state.settings().open);
        assert!(world.trace.contains(&Effect::SpeechRouted {
            text,
            first: Some(expected.clone()),
            plugin: plugin_active,
        }));
        assert!(world.trace.contains(&Effect::Apply(expected)));
    }
}

#[test]
fn controller_ptt_uses_global_buttons_and_defaults_to_share() {
    for (setting, name, talks) in [
        ("", ButtonName::Record, true),
        ("", ButtonName::Start, false),
        ("push_to_talk = ['start']", ButtonName::Start, true),
        ("push_to_talk = ['start']", ButtonName::Record, false),
        ("push_to_talk = []", ButtonName::Record, false),
    ] {
        let context = tehuti_protocol::parse_context(&format!(
            "app_id = '*'\n{setting}\n[buttons]\nstart = {{ key = 'enter' }}\nrecord = {{ key = 'enter' }}"
        ))
        .unwrap();
        let mut world = World::default();
        let state = State::new(crate::State::with_contexts(vec![context]));
        let state = world.run(state, Input::Speech(SpeechInput::Available));
        let state = world.run(state, pad_button(name, ButtonState::Pressed));
        let _state = world.run(state, pad_button(name, ButtonState::Released));
        let commands: Vec<_> = world
            .trace
            .iter()
            .filter_map(|effect| {
                if let Effect::SpeechCommand(command) = effect {
                    Some(command.clone())
                } else {
                    None
                }
            })
            .collect();
        let expected = if talks {
            vec![
                SpeechEffect::Start { session: 0 },
                SpeechEffect::Stop {
                    session: 0,
                    transcribe: true,
                },
            ]
        } else {
            Vec::new()
        };
        assert_eq!(commands, expected, "{setting}, {name:?}");
        assert_eq!(
            world.trace.contains(&Effect::Apply(Action::Key {
                key: tehuti_protocol::KeyName::Enter,
                mods: Vec::new()
            })),
            !talks
        );
    }
}

#[test]
fn configured_start_ptt_preserves_guide_start_escape() {
    let context =
        tehuti_protocol::parse_context("app_id = '*'\npush_to_talk = ['start']\n[buttons]").unwrap();
    let mut world = World::default();
    let state = State::new(crate::State::with_contexts(vec![context]));
    let mut state = world.run(state, Input::Speech(SpeechInput::Available));
    for name in [ButtonName::Guide, ButtonName::Start] {
        state = world.run(state, pad_button(name, ButtonState::Pressed));
    }
    let _state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_secs(3),
        },
    );
    assert!(world.trace.contains(&Effect::Apply(Action::Quit)));
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::SpeechCommand(_)))
    );
}

#[test]
fn display_scale_changes_immediately_without_mode_confirmation() {
    let mut world = World::default();
    let mut state = open_display(&mut world);
    for _ in 0..3 {
        state = settings_button(&mut world, state, ButtonName::DpadDown);
    }
    state = settings_button(&mut world, state, ButtonName::DpadRight);
    state = settings_button(&mut world, state, ButtonName::DpadRight);
    assert_eq!(state.settings().scale.get(), 3);
    assert!(state.settings().remaining.is_none());
    assert!(
        world
            .trace
            .contains(&Effect::SettingsScale(core::num::NonZeroU32::new(3).unwrap()))
    );
    state = settings_button(&mut world, state, ButtonName::DpadLeft);
    assert_eq!(state.settings().scale.get(), 2);
}
proptest! {
    #[test]
    fn passthrough_keeps_only_escape_and_record(
        mapping in arbitrary_mapping(),
        mut contexts in arbitrary_contexts(),
        buttons in prop::collection::vec(prop::sample::select(vec![ButtonName::South, ButtonName::East, ButtonName::North, ButtonName::West, ButtonName::Select, ButtonName::LeftBumper, ButtonName::RightBumper, ButtonName::Start]), 1..32),
    ) {
        let context = tehuti_protocol::parse_context(&format!("app_id = 'org.example.Emulator'\n[buttons]\neast = {mapping}\nnorth = {mapping}\n[pad]\npassthrough = true")).unwrap();
        contexts.insert(0, context);
        let mut state = configured(contexts, "org.example.Emulator");
        let mut world = World { plugin_active: true, ..World::default() };
        state = world.run(state, Input::Speech(SpeechInput::Available));
        for name in buttons {
            for button in [ButtonState::Pressed, ButtonState::Released] {
                state = world.run(state, pad_button(name, button));
            }
        }
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::Apply(_) | Effect::DispatchPlugin { .. } | Effect::ConfirmSecret(_) | Effect::Chord(_) | Effect::Click { .. } | Effect::MouseMode(_) | Effect::SpeechCommand(_))), "passthrough suppresses other controller actions");
        state = world.run(state, pad_button(ButtonName::Record, ButtonState::Pressed));
        state = world.run(state, pad_button(ButtonName::Record, ButtonState::Released));
        let commands: Vec<_> = world.trace.iter().filter_map(|effect| if let Effect::SpeechCommand(command) = effect { Some(command.clone()) } else { None }).collect();
        prop_assert_eq!(commands, vec![SpeechEffect::Start { session: 0 }, SpeechEffect::Stop { session: 0, transcribe: true }]);
        for name in [ButtonName::Guide, ButtonName::Start] {
            state = world.run(state, pad_button(name, ButtonState::Pressed));
        }
        state = world.run(state, Input::Tick { elapsed: Duration::from_secs(3) });
        prop_assert!(world.trace.contains(&Effect::Apply(Action::Quit)));
        prop_assert!(state.core().menu().is_none());
        prop_assert_eq!(state.core().level(), crate::Level::App);
        state = world.event(state, focus("org.example.Editor"));
        prop_assert!(!state.core().passthrough());
        world.trace.clear();
        let _state = world.run(state, pad_button(ButtonName::South, ButtonState::Pressed));
        prop_assert!(world.trace.iter().any(|effect| matches!(effect, Effect::ConfirmSecret(_))));
    }
}

proptest! {
    #[test]
    fn passthrough_ptt_chord_starts_once_and_stops_on_either_release(reverse in any::<bool>()) {
        let context = tehuti_protocol::parse_context(include_str!("../../../../examples/emulator/context.toml")).unwrap();
        let mut state = configured(vec![context], "org.example.Emulator");
        let mut world = World::default();
        state = world.run(state, Input::Speech(SpeechInput::Available));
        let buttons = if reverse { [ButtonName::LeftBumper, ButtonName::Select] } else { [ButtonName::Select, ButtonName::LeftBumper] };
        state = world.run(state, pad_button(buttons[0], ButtonState::Pressed));
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::SpeechCommand(_))));
        state = world.run(state, pad_button(buttons[1], ButtonState::Pressed));
        state = world.run(state, pad_button(buttons[1], ButtonState::Pressed));
        state = world.run(state, pad_button(ButtonName::South, ButtonState::Pressed));
        state = world.run(state, pad_button(buttons[0], ButtonState::Released));
        let commands: Vec<_> = world.trace.iter().filter_map(|effect| if let Effect::SpeechCommand(command) = effect { Some(command.clone()) } else { None }).collect();
        prop_assert_eq!(commands, vec![SpeechEffect::Start { session: 0 }, SpeechEffect::Stop { session: 0, transcribe: true }]);
        state = world.run(state, pad_button(buttons[0], ButtonState::Pressed));
        state = world.event(state, focus("org.example.Editor"));
        prop_assert!(world.trace.contains(&Effect::SpeechCommand(SpeechEffect::Stop { session: 1, transcribe: false })), "focus cancels passthrough PTT");
        state = world.event(state, focus("org.example.Emulator"));
        world.trace.clear();
        let _state = world.run(state, pad_button(buttons[0], ButtonState::Pressed));
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::SpeechCommand(_))));
    }
}

#[test]
fn title_updates_reach_plugins_without_cancelling_capture() {
    let mut world = World::default();
    let mut state = world.event(State::default(), focus("editor"));
    for input in [SpeechInput::Available, SpeechInput::Press] {
        state = world.run(state, Input::Speech(input));
    }
    world.trace.clear();
    state = world.run(
        state,
        Input::FocusTitle {
            app_id: Some("editor".into()),
            title: "Updated document".into(),
        },
    );
    assert!(world.trace.iter().any(|effect| matches!(
        effect,
        Effect::DispatchPlugin { event: Event::Focus { title, .. }, .. }
            if title == "Updated document"
    )));
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::SpeechCommand(SpeechEffect::Stop { .. })))
    );
    world.trace.clear();
    let _state = world.run(state, Input::Speech(SpeechInput::Release));
    assert!(world.trace.iter().any(|effect| matches!(
        effect,
        Effect::SpeechCommand(SpeechEffect::Stop { transcribe: true, .. })
    )));
}

#[test]
fn focus_changes_cancel_capture_even_with_the_same_application() {
    for app_id in ["editor", "browser"] {
        let mut world = World::default();
        let mut state = world.event(State::default(), focus("editor"));
        for input in [SpeechInput::Available, SpeechInput::Press] {
            state = world.run(state, Input::Speech(input));
        }
        world.trace.clear();
        let _state = world.event(state, focus(app_id));
        assert!(world.trace.iter().any(|effect| matches!(
            effect,
            Effect::SpeechCommand(SpeechEffect::Stop {
                transcribe: false,
                ..
            })
        )));
    }
}

#[test]
fn y_opening_obeys_fields_terminals_and_context_overrides() {
    for (terminal, kind) in [
        (false, crate::keyboard::FieldKind::Normal),
        (true, crate::keyboard::FieldKind::Normal),
        (false, crate::keyboard::FieldKind::Terminal),
    ] {
        for active in [false, true] {
            for global_mode in ["button", "open", "off"] {
                for local_mode in [None, Some("button"), Some("open"), Some("off")] {
                    let global = tehuti_protocol::parse_context(&format!(
                        "app_id = '*'\n[buttons]\n[keyboard]\non_text_field = '{global_mode}'"
                    ))
                    .unwrap();
                    let local_option =
                        local_mode.map_or_else(String::new, |mode| format!("on_text_field = '{mode}'\n"));
                    let local = tehuti_protocol::parse_context(&format!(
                        "app_id = 'sample'\n[buttons]\nnorth = {{ key = 'tab' }}\n[keyboard]\nterminal = {terminal}\n{local_option}"
                    )).unwrap();
                    let mut world = World::default();
                    let state = configured(vec![global, local], "sample");
                    let state = world.run(
                        state,
                        Input::TextField {
                            active,
                            changed: true,
                            kind,
                        },
                    );
                    assert!(!state.keyboard().is_open());
                    let state = tap_button(&mut world, state, ButtonName::North);
                    let allowed = match local_mode.unwrap_or(global_mode) {
                        "open" => true,
                        "off" => false,
                        _ => terminal || active,
                    };
                    assert_eq!(
                        state.keyboard().is_open(),
                        allowed,
                        "terminal={terminal}, kind={kind:?}, active={active}, global={global_mode}, local={local_mode:?}"
                    );
                    if allowed {
                        assert_eq!(state.keyboard().draft(), "");
                        let state = tap_button(&mut world, state, ButtonName::North);
                        assert_eq!(state.keyboard().draft(), " ");
                    } else {
                        assert!(world.trace.contains(&Effect::Apply(Action::Key {
                            key: tehuti_protocol::KeyName::Tab,
                            mods: Vec::new(),
                        })));
                    }
                }
            }
        }
    }
}

fn takeback_context(ptt: &str) -> State {
    configured(
        vec![
            tehuti_protocol::parse_context(&format!(
                "app_id = 'org.example.Game'\nbuttons = {{}}\n[pad]\npassthrough = true\npush_to_talk = {ptt}"
            ))
            .unwrap(),
        ],
        "org.example.Game",
    )
}

proptest! {
    #[test]
    fn passthrough_sticks_and_global_buttons_stay_with_the_application(
        mut contexts in arbitrary_contexts(),
        mapping in arbitrary_mapping(),
        right_y in 1_i16..=1000,
    ) {
        for passthrough in [false, true] {
            let context = tehuti_protocol::parse_context(&format!("app_id = 'org.example.Game'\nbuttons = {{}}\n[pad]\npassthrough = {passthrough}")).unwrap();
            let global = tehuti_protocol::parse_context(&format!("app_id = '*'\n[buttons]\nwest = {mapping}")).unwrap();
            contexts.insert(0, global);
            contexts.insert(0, context);
            let mut world = World::default();
            let mut state = configured(contexts.clone(), "org.example.Game");
            state = world.run(state, Input::Sticks(Sticks { left: (1000, 1000), right: (1000, right_y) }));
            state = world.run(state, Input::Tick { elapsed: Duration::from_millis(10) });
            prop_assert_eq!(world.trace.iter().any(|effect| matches!(effect, Effect::Scroll { y, .. } if *y > 0)), !passthrough);
            world.trace.clear();
            state = world.run(state, pad_button(ButtonName::South, ButtonState::Pressed));
            prop_assert_eq!(world.trace.contains(&Effect::Apply(Action::Key { key: tehuti_protocol::KeyName::Enter, mods: Vec::new() })), !passthrough);
            world.trace.clear();
            state = world.run(state, pad_button(ButtonName::West, ButtonState::Pressed));
            prop_assert_eq!(world.trace.iter().any(|effect| matches!(effect, Effect::Apply(_))), !passthrough);
            for button in [ButtonState::Pressed, ButtonState::Released, ButtonState::Pressed, ButtonState::Released] {
                state = world.run(state, pad_button(ButtonName::Select, button));
            }
            world.trace.clear();
            state = world.run(state, Input::Sticks(Sticks { left: (1000, 1000), right: (1000, right_y) }));
            let _state = world.run(state, Input::Tick { elapsed: Duration::from_millis(10) });
            prop_assert_eq!(world.trace.iter().any(|effect| matches!(effect, Effect::MovePointer { .. })), !passthrough);
        }
    }

    #[test]
    fn passthrough_shoulders_follow_the_focus_level(mapping in arbitrary_mapping()) {
        let context = tehuti_protocol::parse_context("app_id = 'org.example.Game'\nbuttons = {}\n[pad]\npassthrough = true").unwrap();
        let global = tehuti_protocol::parse_context(&format!("app_id = '*'\n[buttons]\nleft_bumper = {mapping}\nright_bumper = {mapping}")).unwrap();
        let mut state = configured(vec![context, global], "org.example.Game");
        let mut world = World::default();
        for columns in [false, true] {
            world.trace.clear();
            for name in [ButtonName::LeftBumper, ButtonName::RightBumper] {
                for button in [ButtonState::Pressed, ButtonState::Released] {
                    state = world.run(state, pad_button(name, button));
                }
            }
            let actions: Vec<_> = world.trace.iter().filter_map(|effect| if let Effect::Apply(action) = effect { Some(action.clone()) } else { None }).collect();
            if columns {
                prop_assert_eq!(actions, vec![Action::Layout { action: tehuti_protocol::LayoutAction::FocusColumnLeft }, Action::Layout { action: tehuti_protocol::LayoutAction::FocusColumnRight }]);
            } else {
                prop_assert!(actions.is_empty());
            }
            for button in [ButtonState::Pressed, ButtonState::Released] {
                state = world.run(state, pad_button(ButtonName::Guide, button));
            }
            prop_assert_eq!(state.core().level(), if columns { crate::Level::App } else { crate::Level::Columns });
        }
    }

    #[test]
    fn passthrough_held_guide_opens_system_menu(mapping in arbitrary_mapping(), mut contexts in arbitrary_contexts()) {
        let context = tehuti_protocol::parse_context(&format!("app_id = 'org.example.Game'\n[buttons]\nguide = {mapping}\n[pad]\npassthrough = true")).unwrap();
        contexts.insert(0, context);
        let mut state = configured(contexts, "org.example.Game");
        let mut world = World::default();
        state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Pressed));
        state = world.run(state, Input::Tick { elapsed: Duration::from_millis(500) });
        prop_assert!(state.core().menu().is_some());
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::Apply(_))));
        state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Released));
        prop_assert!(state.core().menu().is_some());
        state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Pressed));
        prop_assert!(state.core().menu().is_none());
        state = world.run(state, pad_button(ButtonName::Start, ButtonState::Pressed));
        let _state = world.run(state, Input::Tick { elapsed: Duration::from_secs(3) });
        prop_assert!(world.trace.contains(&Effect::Apply(Action::Quit)));
    }
}

proptest! {
    #[test]
    fn solo_view_hold_toggles_once_and_release_keeps_the_mode(extra_ms in 0u64..5000) {
        let mut world = World::default();
        let mut state = takeback_context("['select']");
        state = world.run(state, Input::Speech(SpeechInput::Available));
        for expected in [false, true] {
            state = world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed));
            prop_assert_eq!(state.core().passthrough(), !expected);
            state = world.run(state, Input::Tick { elapsed: Duration::from_millis(499) });
            prop_assert_eq!(state.core().passthrough(), !expected);
            state = world.run(state, Input::Tick { elapsed: Duration::from_millis(1) });
            prop_assert_eq!(state.core().passthrough(), expected);
            state = world.run(state, Input::Tick { elapsed: Duration::from_millis(extra_ms) });
            state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
            prop_assert_eq!(state.core().passthrough(), expected);
        }
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::SpeechCommand(_) | Effect::MouseMode(_))));
    }

    #[test]
    fn short_view_keeps_passthrough_and_defers_single_button_ptt(millis in 0u64..500) {
        let mut world = World::default();
        let mut state = takeback_context("['select']");
        state = world.run(state, Input::Speech(SpeechInput::Available));
        state = world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed));
        state = world.run(state, Input::Tick { elapsed: Duration::from_millis(millis) });
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::SpeechCommand(_))));
        state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
        prop_assert!(state.core().passthrough());
        let commands: Vec<_> = world.trace.iter().filter_map(|effect| if let Effect::SpeechCommand(command) = effect { Some(command.clone()) } else { None }).collect();
        prop_assert_eq!(commands, vec![SpeechEffect::Start { session: 0 }, SpeechEffect::Stop { session: 0, transcribe: true }]);
    }

    #[test]
    fn another_button_cancels_view_hold_and_focus_restores_context(reverse in any::<bool>()) {
        let mut world = World::default();
        let mut state = takeback_context("['select', 'left_bumper']");
        state = world.run(state, Input::Speech(SpeechInput::Available));
        let buttons = if reverse { [ButtonName::Select, ButtonName::LeftBumper] } else { [ButtonName::LeftBumper, ButtonName::Select] };
        for name in buttons {
            state = world.run(state, pad_button(name, ButtonState::Pressed));
        }
        state = world.run(state, Input::Tick { elapsed: Duration::from_secs(1) });
        prop_assert!(state.core().passthrough());
        prop_assert!(world.trace.contains(&Effect::SpeechCommand(SpeechEffect::Start { session: 0 })), "chord starts capture");
        for name in buttons {
            state = world.run(state, pad_button(name, ButtonState::Released));
        }
        state = world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed));
        state = world.run(state, Input::Tick { elapsed: Duration::from_millis(500) });
        prop_assert!(!state.core().passthrough());
        state = world.event(state, focus("org.example.Game"));
        prop_assert!(state.core().passthrough());
        state = world.run(state, Input::Tick { elapsed: Duration::from_secs(1) });
        prop_assert!(state.core().passthrough());
    }
}

#[test]
fn guide_hold_opens_and_closes_menu_in_passthrough() {
    let mut world = World::default();
    let mut state = takeback_context("['record']");
    state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Pressed));
    state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_millis(500),
        },
    );
    assert!(state.core().menu().is_some());
    state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Released));
    state = world.run(state, pad_button(ButtonName::East, ButtonState::Pressed));
    assert!(state.core().menu().is_none());
    assert!(state.core().passthrough());
}

#[test]
fn takeback_survives_title_updates_and_short_view_taps_toggle_mouse() {
    let mut world = World::default();
    let mut state = takeback_context("['record']");
    state = world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed));
    state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_millis(500),
        },
    );
    state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
    state = world.run(
        state,
        Input::FocusTitle {
            app_id: Some("org.example.Game".into()),
            title: "Playing".into(),
        },
    );
    assert!(!state.core().passthrough());
    for _tap in 0..2 {
        state = world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed));
        state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
    }
    assert!(world.trace.contains(&Effect::MouseMode(true)));
    state = world.run(state, Input::ControllerDisconnected);
    assert!(state.core().passthrough());
}

/// The input-settings effects in the trace, without the routine tick queries.
fn input_effects(world: &World) -> Vec<Effect> {
    world
        .trace
        .iter()
        .filter(|effect| {
            matches!(
                effect,
                Effect::SettingsController(_)
                    | Effect::PersistController(_)
                    | Effect::SettingsMicrophone(_)
                    | Effect::ListMicrophones
            )
        })
        .cloned()
        .collect()
}

fn open_input(world: &mut World) -> State {
    let state = world.run(
        State::default(),
        Input::Controllers(vec!["Wireless Controller".into(), "Xbox Controller".into()]),
    );
    let state = settings_event(
        world,
        state,
        Event::Speech {
            text: Text::new("Pop the hood.".into()).unwrap(),
        },
    );
    let state = settings_button(world, state, ButtonName::DpadDown);
    assert_eq!(state.settings().category, Category::Input);
    let state = settings_button(world, state, ButtonName::South);
    assert_eq!(state.settings().page, Page::Input);
    state
}

#[test]
fn opening_the_input_page_lists_microphones_and_starts_on_the_controller_row() {
    let mut world = World::default();
    let state = open_input(&mut world);
    assert!(world.trace.contains(&Effect::ListMicrophones));
    assert_eq!(state.settings().input.row, InputRow::Controller);
    assert_eq!(state.settings().input.controller_label(), "AUTO (EVERY PAD)");
    let state = world.run(
        state,
        Input::Microphones(vec![Microphone {
            name: "alsa_input.analog".into(),
            description: "Built-in Audio".into(),
        }]),
    );
    assert_eq!(state.settings().input.microphones.len(), 1);
}

#[test]
fn a_named_controller_previews_until_a_confirms_it() {
    let mut world = World::default();
    let state = open_input(&mut world);
    world.trace.clear();
    let state = settings_button(&mut world, state, ButtonName::DpadRight);
    let wireless = ControllerChoice::Named("Wireless Controller".into());
    assert_eq!(
        input_effects(&world),
        vec![Effect::SettingsController(wireless.clone())]
    );
    assert_eq!(state.settings().remaining, Some(Duration::from_secs(15)));
    assert_eq!(state.settings().button_hint(), "A ON THE NEW PAD: KEEP IT");
    world.trace.clear();
    let state = settings_button(&mut world, state, ButtonName::South);
    assert_eq!(
        input_effects(&world),
        vec![Effect::PersistController(wireless.clone())]
    );
    assert_eq!(state.settings().remaining, None);
    let state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_secs(30),
        },
    );
    assert_eq!(state.settings().input.controller, wireless);
}

#[test]
fn an_unconfirmed_controller_reverts_on_timeout_back_or_close() {
    for leave in 0..3 {
        let mut world = World::default();
        let state = open_input(&mut world);
        let state = settings_button(&mut world, state, ButtonName::DpadLeft);
        assert_eq!(
            state.settings().input.controller,
            ControllerChoice::Named("Xbox Controller".into())
        );
        world.trace.clear();
        let state = match leave {
            0 => world.run(
                state,
                Input::Tick {
                    elapsed: Duration::from_secs(15),
                },
            ),
            1 => settings_button(&mut world, state, ButtonName::East),
            _ => settings_event(
                &mut world,
                state,
                Event::Speech {
                    text: Text::new("Pop the hood".into()).unwrap(),
                },
            ),
        };
        assert!(
            world
                .trace
                .contains(&Effect::SettingsController(ControllerChoice::Auto))
        );
        assert_eq!(state.settings().input.controller, ControllerChoice::Auto);
        assert_eq!(state.settings().remaining, None);
    }
}

#[test]
fn going_back_to_auto_applies_and_persists_at_once() {
    let mut world = World::default();
    let state = world.run(
        State::default(),
        Input::InputChoices {
            controller: ControllerChoice::Named("Wireless Controller".into()),
            microphone: MicrophoneChoice::All,
        },
    );
    let state = world.run(state, Input::Controllers(vec!["Wireless Controller".into()]));
    let state = settings_event(
        &mut world,
        state,
        Event::Speech {
            text: Text::new("Pop the hood".into()).unwrap(),
        },
    );
    let state = settings_button(&mut world, state, ButtonName::DpadDown);
    let state = settings_button(&mut world, state, ButtonName::South);
    world.trace.clear();
    let state = settings_button(&mut world, state, ButtonName::DpadRight);
    assert_eq!(
        input_effects(&world),
        vec![Effect::PersistController(ControllerChoice::Auto)]
    );
    assert_eq!(state.settings().remaining, None);
}

#[test]
fn a_saved_controller_that_is_not_connected_stays_selectable() {
    let input = InputSettings {
        controllers: vec!["Pad".into(), "Pad".into()],
        controller: ControllerChoice::Named("Gone".into()),
        ..InputSettings::default()
    };
    assert_eq!(
        input.controller_options(),
        vec![
            ControllerChoice::Auto,
            ControllerChoice::Named("Pad".into()),
            ControllerChoice::Named("Gone".into()),
        ]
    );
    assert_eq!(input.controller_label(), "GONE (OFF)");
}

#[test]
fn the_microphone_row_cycles_all_auto_and_each_source_without_confirmation() {
    let mut world = World::default();
    let state = open_input(&mut world);
    let state = world.run(
        state,
        Input::Microphones(vec![Microphone {
            name: "alsa_input.analog".into(),
            description: "Built-in Audio".into(),
        }]),
    );
    let mut state = settings_button(&mut world, state, ButtonName::DpadDown);
    assert_eq!(state.settings().input.row, InputRow::Microphone);
    assert_eq!(state.settings().input.microphone_label(), "ALL MICROPHONES");
    let expected = [
        (MicrophoneChoice::Auto, "AUTO (SYSTEM DEFAULT)"),
        (
            MicrophoneChoice::Named("alsa_input.analog".into()),
            "BUILT-IN AUDIO",
        ),
        (MicrophoneChoice::All, "ALL MICROPHONES"),
    ];
    for (choice, label) in expected {
        world.trace.clear();
        state = settings_button(&mut world, state, ButtonName::DpadRight);
        assert_eq!(input_effects(&world), vec![Effect::SettingsMicrophone(choice)]);
        assert_eq!(state.settings().input.microphone_label(), label);
        assert_eq!(state.settings().remaining, None);
    }
    world.trace.clear();
    let state = settings_button(&mut world, state, ButtonName::DpadLeft);
    assert_eq!(
        input_effects(&world),
        vec![Effect::SettingsMicrophone(MicrophoneChoice::Named(
            "alsa_input.analog".into()
        ))]
    );
    let state = settings_button(&mut world, state, ButtonName::East);
    assert_eq!(state.settings().page, Page::Categories);
    assert!(state.settings().open);
}

fn hold_guide(world: &mut World, mut state: State) -> State {
    state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Pressed));
    state = world.run(
        state,
        Input::Tick {
            elapsed: crate::GUIDE_TAP_WINDOW,
        },
    );
    world.run(state, pad_button(ButtonName::Guide, ButtonState::Released))
}

fn held_emulator(world: &mut World) -> State {
    let context = tehuti_protocol::parse_context(
        "app_id = 'org.example.Emulator'\n[buttons]\nsouth = { key = 'x', hold = true }\neast = { key = 'z' }",
    )
    .unwrap();
    let state = configured(vec![context], "org.example.Emulator");
    let state = world.run(state, pad_button(ButtonName::South, ButtonState::Pressed));
    assert_eq!(held_key_effects(world), vec![Action::KeyDown { key: KeyName::X }]);
    world.trace.clear();
    state
}

fn held_key_effects(world: &World) -> Vec<Action> {
    world
        .trace
        .iter()
        .filter_map(|effect| {
            if let Effect::Apply(action @ (Action::KeyDown { .. } | Action::KeyUp { .. })) = effect {
                Some(action.clone())
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn a_focus_change_releases_a_held_key_and_the_later_release_sends_nothing() {
    let mut world = World::default();
    let state = held_emulator(&mut world);
    let state = world.event(state, focus("other"));
    assert_eq!(held_key_effects(&world), vec![Action::KeyUp { key: KeyName::X }]);
    world.trace.clear();
    let _state = world.run(state, pad_button(ButtonName::South, ButtonState::Released));
    assert_eq!(held_key_effects(&world), Vec::new());
}

#[test]
fn a_title_change_keeps_a_held_key_down() {
    let mut world = World::default();
    let state = held_emulator(&mut world);
    let state = world.run(
        state,
        Input::FocusTitle {
            app_id: Some("org.example.Emulator".into()),
            title: "Level 2".into(),
        },
    );
    assert_eq!(held_key_effects(&world), Vec::new());
    let _state = world.run(state, pad_button(ButtonName::South, ButtonState::Released));
    assert_eq!(held_key_effects(&world), vec![Action::KeyUp { key: KeyName::X }]);
}

#[test]
fn a_controller_disconnect_releases_a_held_key() {
    let mut world = World::default();
    let state = held_emulator(&mut world);
    let _state = world.run(state, Input::ControllerDisconnected);
    assert_eq!(held_key_effects(&world), vec![Action::KeyUp { key: KeyName::X }]);
}

#[test]
fn opening_the_system_menu_releases_a_held_key() {
    let mut world = World::default();
    let state = held_emulator(&mut world);
    let state = hold_guide(&mut world, state);
    assert!(state.core().menu().is_some());
    assert_eq!(held_key_effects(&world), vec![Action::KeyUp { key: KeyName::X }]);
}

#[test]
fn a_release_the_open_keyboard_consumes_still_lifts_the_held_key() {
    let mut world = World::default();
    let state = held_emulator(&mut world);
    let state = with_open_keyboard(&mut world, state);
    assert_eq!(held_key_effects(&world), Vec::new());
    let _state = world.run(state, pad_button(ButtonName::South, ButtonState::Released));
    assert_eq!(held_key_effects(&world), vec![Action::KeyUp { key: KeyName::X }]);
}

proptest! {
    #[test]
    fn held_keys_balance_and_none_outlives_its_buttons(
        holds in prop::collection::hash_map(
            prop::sample::select(vec!["south", "east", "north", "west", "dpad_up", "left_bumper"]),
            prop::sample::select(vec!["x", "z", "up"]),
            0..6,
        ),
        inputs in prop::collection::vec(prop_oneof![
            8 => every_button().prop_map(|event| Input::Event { event, elapsed: Duration::from_millis(16) }),
            1 => Just(speech_event("keyboard")),
            1 => Just(Input::ControllerDisconnected),
            1 => prop::sample::select(vec!["org.example.Emulator", "other"]).prop_map(|app| Input::Event { event: focus(app), elapsed: Duration::ZERO }),
            1 => (0_u64..1000).prop_map(|millis| Input::Tick { elapsed: Duration::from_millis(millis) }),
        ], 0..60),
    ) {
        let mut source = String::from("app_id = 'org.example.Emulator'\n[buttons]\n");
        for (button, key) in &holds {
            writeln!(source, "{button} = {{ key = '{key}', hold = true }}").unwrap();
        }
        let context = tehuti_protocol::parse_context(&source).unwrap();
        let mut world = World::default();
        let mut state = configured(vec![context], "org.example.Emulator");
        for input in inputs {
            state = world.run(state, input);
        }
        for event in every_button_release() {
            state = world.run(state, Input::Event { event, elapsed: Duration::ZERO });
        }
        let mut down: Vec<KeyName> = Vec::new();
        for action in held_key_effects(&world) {
            if let Action::KeyDown { key } = action {
                prop_assert!(!down.contains(&key), "a held key goes down once");
                down.push(key);
            } else if let Action::KeyUp { key } = action {
                prop_assert!(down.contains(&key), "only a held key goes up");
                down.retain(|held| *held != key);
            }
        }
        prop_assert_eq!(down, Vec::new(), "releasing every button lifts every held key");
    }
}

fn every_button_release() -> Vec<Event> {
    [
        ButtonName::South,
        ButtonName::East,
        ButtonName::North,
        ButtonName::West,
        ButtonName::DpadUp,
        ButtonName::LeftBumper,
    ]
    .into_iter()
    .map(|name| Event::Button {
        name,
        state: ButtonState::Released,
    })
    .collect()
}

fn hold_view(world: &mut World, mut state: State) -> State {
    state = world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed));
    state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_millis(600),
        },
    );
    world.run(state, pad_button(ButtonName::Select, ButtonState::Released))
}

#[test]
fn held_keys_follow_the_pad_through_a_view_takeback() {
    let context = tehuti_protocol::parse_context(
        "app_id = 'org.example.Game'\n[buttons]\nsouth = { key = 'x', hold = true }\n[pad]\npassthrough = true",
    )
    .unwrap();
    let mut world = World::default();
    let mut state = configured(vec![context], "org.example.Game");
    state = world.run(state, pad_button(ButtonName::South, ButtonState::Pressed));
    state = world.run(state, pad_button(ButtonName::South, ButtonState::Released));
    assert_eq!(held_key_effects(&world), Vec::new(), "passthrough holds nothing");
    state = hold_view(&mut world, state);
    assert!(!state.core().passthrough());
    state = world.run(state, pad_button(ButtonName::South, ButtonState::Pressed));
    assert_eq!(
        held_key_effects(&world),
        vec![Action::KeyDown { key: KeyName::X }]
    );
    state = world.run(state, pad_button(ButtonName::South, ButtonState::Released));
    assert_eq!(
        held_key_effects(&world),
        vec![
            Action::KeyDown { key: KeyName::X },
            Action::KeyUp { key: KeyName::X }
        ]
    );
    world.trace.clear();
    state = hold_view(&mut world, state);
    assert!(state.core().passthrough());
    let _state = world.run(state, pad_button(ButtonName::South, ButtonState::Pressed));
    assert_eq!(
        held_key_effects(&world),
        Vec::new(),
        "the pad is the application's again"
    );
}

#[test]
fn holding_guide_opens_the_menu_whatever_a_context_maps_on_guide() {
    for mapping in ["{ key = 'x' }", "{ key = 'x', hold = true }"] {
        for passthrough in [false, true] {
            for held_for in [crate::GUIDE_TAP_WINDOW, Duration::from_secs(3)] {
                let context = tehuti_protocol::parse_context(&format!(
                    "app_id = 'org.example.Emulator'\n[buttons]\nguide = {mapping}\n[pad]\npassthrough = {passthrough}"
                ))
                .unwrap();
                let mut world = World::default();
                let mut state = configured(vec![context], "org.example.Emulator");
                state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Pressed));
                state = world.run(state, Input::Tick { elapsed: held_for });
                state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Released));
                assert!(
                    state.core().menu().is_some(),
                    "{mapping}, passthrough {passthrough}, {held_for:?}"
                );
                assert!(
                    !world.trace.iter().any(|effect| matches!(
                        effect,
                        Effect::Apply(Action::Key { .. } | Action::KeyDown { .. } | Action::KeyUp { .. })
                    )),
                    "the Guide mapping never fires: {mapping}, passthrough {passthrough}"
                );
            }
        }
    }
}

#[test]
fn tab_phrases_dictate_while_the_keyboard_is_open() {
    let global =
        tehuti_protocol::parse_context(include_str!("../../../../examples/global/context.toml")).unwrap();
    let mut world = World::default();
    let state = configured(vec![global], "org.example.Editor");
    let mut state = with_open_keyboard(&mut world, state);
    world.trace.clear();
    for phrase in ["tab left", "tab right"] {
        state = world.run(state, speech_event(phrase));
    }
    assert_eq!(state.keyboard().draft(), "tab left tab right");
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::Apply(_)))
    );
}

#[test]
fn menu_with_an_empty_draft_presses_only_enter_and_stays_open() {
    let mut world = World::default();
    let state = with_open_keyboard(&mut world, State::default());
    world.trace.clear();
    let state = tap_button(&mut world, state, ButtonName::Start);
    assert!(state.keyboard().is_open());
    assert_eq!(
        world
            .trace
            .iter()
            .filter(|effect| matches!(effect, Effect::Apply(_) | Effect::Chord(_)))
            .collect::<Vec<_>>(),
        vec![&Effect::Chord(crate::keyboard::Chord {
            key: crate::keyboard::ChordKey::Named(KeyName::Enter),
            modifiers: crate::keyboard::Modifiers::default(),
        })]
    );
}

proptest! {
    #[test]
    fn a_different_text_field_clears_even_a_retained_draft(
        global in any::<bool>(),
        closed in any::<bool>(),
        text in "[a-zé]{1,80}",
    ) {
        let context = tehuti_protocol::parse_context(&format!(
            "app_id = '{}'\n[buttons]\n[keyboard]\nkeep_draft = true",
            if global { "*" } else { "editor" },
        )).unwrap();
        let mut world = World::default();
        let mut state = configured(vec![context], "editor");
        let field = |changed| Input::TextField {
            active: true,
            changed,
            kind: crate::keyboard::FieldKind::Normal,
        };
        state = world.run(state, field(true));
        state = with_open_keyboard(&mut world, state);
        state = world.run(state, speech_event(&text));
        if closed {
            state = tap_button(&mut world, state, ButtonName::East);
            prop_assert!(!state.keyboard().is_open());
        }
        state = world.run(state, field(false));
        prop_assert_eq!(state.keyboard().draft(), text.as_str());
        state = world.run(state, field(true));
        prop_assert_eq!(state.keyboard().draft(), "");
        if !state.keyboard().is_open() {
            state = with_open_keyboard(&mut world, state);
        }
        prop_assert_eq!(state.keyboard().draft(), "");
    }

    #[test]
    fn closing_keeps_the_draft_only_when_the_context_opts_in(
        global in prop::option::of(any::<bool>()),
        local in prop::option::of(any::<bool>()),
        text in "[a-zé]{1,80}",
    ) {
        let context = |app: &str, keep: Option<bool>| {
            let option = keep.map_or_else(String::new, |keep| format!("keep_draft = {keep}"));
            tehuti_protocol::parse_context(&format!("app_id = '{app}'\n[buttons]\n[keyboard]\n{option}")).unwrap()
        };
        let mut world = World::default();
        let state = configured(vec![context("*", global), context("editor", local)], "editor");
        let state = with_open_keyboard(&mut world, state);
        let state = world.run(state, speech_event(&text));
        let state = tap_button(&mut world, state, ButtonName::East);
        prop_assert!(!state.keyboard().is_open());
        let expected = if local.or(global).unwrap_or(false) { text.as_str() } else { "" };
        prop_assert_eq!(state.keyboard().draft(), expected);
        let state = with_open_keyboard(&mut world, state);
        prop_assert_eq!(state.keyboard().draft(), expected);
        let state = tap_button(&mut world, state, ButtonName::North);
        prop_assert_eq!(state.keyboard().draft(), format!("{expected} "));
    }
}

#[test]
fn left_stick_moves_the_draft_caret_between_lines() {
    let mut world = World::default();
    let state = with_open_keyboard(&mut world, State::default());
    let state = world.run(state, speech_event("alpha\nbéta\nend"));
    let highlight = state.keyboard().highlight();
    let state = world.run(
        state,
        Input::Sticks(Sticks {
            left: (0, -1000),
            right: (0, 0),
        }),
    );
    assert_eq!(state.keyboard().caret(), 10);
    assert_eq!(state.keyboard().picked_word(), None);
    assert_eq!(state.keyboard().highlight(), highlight);
    let state = world.run(state, Input::Sticks(Sticks::default()));
    let state = world.run(
        state,
        Input::Sticks(Sticks {
            left: (0, 1000),
            right: (0, 0),
        }),
    );
    assert_eq!(state.keyboard().caret(), 15);
    assert_eq!(state.keyboard().draft(), "alpha\nbéta\nend");
}

#[test]
fn settings_phrase_is_dictation_while_the_keyboard_is_open() {
    let mut world = World::default();
    let state = with_open_keyboard(&mut world, State::default());
    world.trace.clear();
    let state = world.run(state, speech_event("pop the hood"));
    assert!(state.keyboard().is_open());
    assert_eq!(state.keyboard().draft(), "pop the hood");
    assert!(!state.settings().open);
}

proptest! {
    #[test]
    fn push_to_talk_keeps_interim_on_the_powerline_and_finishes_at_the_draft_aim(
        contexts in arbitrary_contexts(),
        picked in any::<bool>(),
        phrase in prop_oneof![Just("delete".to_owned()), Just("pop the hood".to_owned()), "[a-z]{1,8}( [a-z]{1,8}){0,3}"],
    ) {
        let mut world = World { plugin_active: true, ..World::default() };
        let app = contexts[0].app_id().to_owned();
        let state = with_open_keyboard(&mut world, configured(contexts, &app));
        let mut state = world.run(state, speech_event("hello world"));
        if picked {
            state = world.run(state, Input::Sticks(Sticks { left: (-1000, 0), right: (0, 0) }));
            state = world.run(state, Input::Sticks(Sticks::default()));
            prop_assert_eq!(state.keyboard().picked_word(), Some(6..11));
        }
        state = world.run(state, Input::Speech(SpeechInput::Available));
        world.trace.clear();
        state = world.run(state, pad_button(ButtonName::Record, ButtonState::Pressed));
        prop_assert!(world.trace.contains(&Effect::SpeechCommand(SpeechEffect::Start { session: 0 })), "press starts capture");
        state = world.run(state, Input::Speech(SpeechInput::Interim {
            session: 0, text: Text::new("unfinished".into()).unwrap(),
        }));
        prop_assert_eq!(&state.core().bar().segments()[1].text, "unfinished");
        prop_assert_eq!(state.keyboard().draft(), "hello world");
        state = world.run(state, pad_button(ButtonName::Record, ButtonState::Released));
        prop_assert!(world.trace.contains(&Effect::SpeechCommand(SpeechEffect::Stop { session: 0, transcribe: true })), "release transcribes");
        prop_assert_eq!(state.keyboard().draft(), "hello world");
        world.trace.clear();
        let state = world.run(state, Input::Speech(SpeechInput::Transcript {
            session: 0, text: Text::new(phrase.clone()).unwrap(),
        }));
        let expected = if picked { format!("hello {phrase}") } else { format!("hello world {phrase}") };
        prop_assert_eq!(state.keyboard().draft(), expected);
        prop_assert_eq!(state.keyboard().picked_word(), None);
        prop_assert!(state.keyboard().is_open());
        prop_assert!(!state.settings().open);
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect,
            Effect::Apply(_) | Effect::QueryPlugin(Event::Speech { .. }) | Effect::DispatchPlugin { event: Event::Speech { .. }, .. } | Effect::SpeechRouted { .. }
        )), "finished speech never routes as a command");
    }
}
