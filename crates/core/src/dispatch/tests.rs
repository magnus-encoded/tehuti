use core::fmt::Write as _;

use super::*;
use proptest::prelude::*;
use tehuti_protocol::Text;

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
                    Effect::Handle(event) => {
                        state = self.run(
                            state,
                            Input::Event {
                                event,
                                elapsed: Duration::ZERO,
                            },
                        );
                    }
                    Effect::Apply(Action::Close) | Effect::PluginAction(Action::Close) => {
                        state = self.close_focus(state);
                    }
                    Effect::PasswordSpeechIgnored
                    | Effect::MovePointer { .. }
                    | Effect::Scroll { .. }
                    | Effect::Click { .. }
                    | Effect::MouseMode(_)
                    | Effect::PollSources
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
        if record || (south && secret_consumes) {
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
    assert_eq!(state.core().bar().segments().len(), 2, "context and level chips");
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
        vec![
            crate::bar::Segment {
                text: "new".into(),
                tone: crate::bar::Tone::Context,
            },
            crate::bar::Segment {
                text: "App".into(),
                tone: crate::bar::Tone::Context,
            }
        ]
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
        let expected = vec![Action::Key { key: expected_key }];
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
        prop_assert!(world.trace.contains(&Effect::Apply(Action::Key { key: tehuti_protocol::KeyName::Tab })), "stick click sends the context key");
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
    fn guide_and_start_reach_normal_dispatch_in_mouse_mode(
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
        prop_assert_eq!(buttons, vec![Event::Button { name, state: ButtonState::Pressed }, Event::Button { name, state: ButtonState::Released }]);
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
fn menu_sends_the_draft_as_text_to_the_focused_app() {
    let mut world = World::default();
    let state = with_open_keyboard(&mut world, State::default());
    let state = world.run(state, speech_event("see you soon"));
    world.trace.clear();
    let state = tap_button(&mut world, state, ButtonName::Start);
    assert!(!state.keyboard().is_open());
    assert_eq!(
        world
            .trace
            .iter()
            .filter(|effect| matches!(effect, Effect::Apply(_)))
            .collect::<Vec<_>>(),
        vec![&Effect::Apply(Action::Text {
            text: Text::new("see you soon".into()).unwrap()
        })]
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
    fn field_policy_opens_only_for_intent_or_the_configured_button(
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
        prop_assert_eq!(state.keyboard().is_open(), mode == "open");
        state = tap_button(&mut world, state, ButtonName::East);
        for event in events {
            let opens = mode != "off" && matches!(event, Event::Button { name, state: ButtonState::Pressed } if name == opening.1);
            state = world.run(state, Input::Event { event, elapsed: Duration::ZERO });
            if state.keyboard().is_open() {
                prop_assert!(opens);
                state = tap_button(&mut world, state, ButtonName::East);
            }
            state = world.run(state, Input::TextField { active: true, changed: false, kind: crate::keyboard::FieldKind::Normal });
            prop_assert!(!state.keyboard().is_open());
        }
        state = world.run(state, Input::TextField { active: true, changed: true, kind: crate::keyboard::FieldKind::Normal });
        prop_assert_eq!(state.keyboard().is_open(), mode == "open");
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
        key: tehuti_protocol::KeyName::Tab
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
    for open in [false, true] {
        let mut world = World::default();
        let mut state = world.run(State::default(), Input::Speech(SpeechInput::Available));
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
        assert_eq!(state.keyboard().is_open(), open);
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
