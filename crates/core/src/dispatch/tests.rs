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

    fn run_event(&mut self, state: State, event: Event) -> State {
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
                    Effect::Handle(event) => {
                        state = self.run_event(state, event);
                    }
                    Effect::Apply(Action::Close) | Effect::PluginAction(Action::Close) => {
                        state = self.close_focus(state);
                    }
                    Effect::UnlockWallpapers
                    | Effect::Wallpaper(_)
                    | Effect::AudioRefresh
                    | Effect::AudioDefault(_)
                    | Effect::SettingsMode(_)
                    | Effect::PersistMode(_)
                    | Effect::MovePointer { .. }
                    | Effect::Scroll { .. }
                    | Effect::Click { .. }
                    | Effect::MouseMode(_)
                    | Effect::PollSources
                    | Effect::SouthPressed(_)
                    | Effect::ResetSecretFocus
                    | Effect::Apply(_)
                    | Effect::System(_)
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

fn button(name: ButtonName, state: ButtonState) -> Input {
    Input::Event {
        event: Event::Button { name, state },
        elapsed: Duration::from_millis(16),
    }
}

fn menu_open(world: &mut World) -> State {
    let mut state = world.run(State::default(), button(ButtonName::Guide, ButtonState::Pressed));
    state = world.run(
        state,
        Input::Event {
            event: Event::Tick {
                elapsed: Duration::from_secs(3),
            },
            elapsed: Duration::ZERO,
        },
    );
    assert!(state.core().menu().is_some());
    world.trace.clear();
    state
}

#[test]
fn choosing_an_item_emits_one_system_effect() {
    for (downs, command) in [
        (1, crate::menu::Command::Screenshot),
        (3, crate::menu::Command::LogOut),
        (4, crate::menu::Command::Restart),
        (5, crate::menu::Command::Reboot),
        (6, crate::menu::Command::PowerOff),
    ] {
        let mut world = World::default();
        let mut state = menu_open(&mut world);
        for _ in 0..downs {
            state = world.run(state, button(ButtonName::DpadDown, ButtonState::Pressed));
        }
        assert!(
            !world
                .trace
                .iter()
                .any(|effect| matches!(effect, Effect::System(_)))
        );
        state = world.run(state, button(ButtonName::South, ButtonState::Pressed));
        let systems: Vec<_> = world
            .trace
            .iter()
            .filter(|effect| matches!(effect, Effect::System(_)))
            .collect();
        assert_eq!(systems, vec![&Effect::System(command)]);
        assert!(state.core().menu().is_none());
    }
}

#[test]
fn an_open_menu_keeps_south_and_record_from_secrets_and_speech() {
    let mut world = World {
        secret_consumes: true,
        ..World::default()
    };
    let state = menu_open(&mut world);
    for name in [ButtonName::South, ButtonName::Record] {
        let next = world.run(state.clone(), button(name, ButtonState::Pressed));
        assert!(
            !world.trace.iter().any(|effect| matches!(
                effect,
                Effect::ConfirmSecret(_) | Effect::SouthPressed(_) | Effect::SpeechCommand(_)
            )),
            "{name:?} press reached secrets or speech"
        );
        assert!(!world.trace.contains(&Effect::Apply(Action::Key {
            key: tehuti_protocol::KeyName::Enter,
            mods: Vec::new()
        })));
        assert_eq!(next.busy(), 0);
        world.trace.clear();
    }
}

#[test]
fn close_app_from_the_menu_applies_close_only() {
    let mut world = World::default();
    let state = menu_open(&mut world);
    let _state = world.run(state, button(ButtonName::South, ButtonState::Pressed));
    let applied: Vec<_> = world
        .trace
        .iter()
        .filter_map(|effect| {
            if let Effect::Apply(action) = effect {
                Some(action.clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(applied, vec![Action::Close]);
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

proptest! {
    #[test]
    fn view_waits_for_release_and_short_presses_keep_the_mapping(
        mut contexts in arbitrary_contexts(),
        duration in 0_u64..2000,
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
        let _state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
        let actual: Vec<_> = world.trace.into_iter().filter_map(|effect| if let Effect::Apply(action) = effect { Some(action) } else { None }).collect();
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn each_hold_toggles_once_independent_of_tick_partition(
        contexts in arbitrary_contexts(),
        early in 0_u64..2000,
        late in 0_u64..10000,
        holds in 1_usize..8,
    ) {
        let mut world = World::default();
        let mut state = configured_mouse(contexts);
        for index in 0..holds {
            state = world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed));
            world.trace.clear();
            state = world.run(state, Input::Tick { elapsed: Duration::from_millis(early) });
            prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::MouseMode(_))));
            state = world.run(state, Input::Tick { elapsed: Duration::from_millis(2000_u64.saturating_sub(early)) });
            state = world.run(state, Input::Tick { elapsed: Duration::from_millis(late) });
            state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
            let toggles: Vec<_> = world.trace.iter().filter(|effect| matches!(effect, Effect::MouseMode(_))).cloned().collect();
            prop_assert_eq!(toggles, vec![Effect::MouseMode(index % 2 == 0)]);
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
        let state = world.run(state, Input::Tick { elapsed: Duration::from_secs(2) });
        let state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
        world.trace.clear();
        let state = world.run(state, pad_button(name, ButtonState::Pressed));
        let _state = world.run(state, pad_button(name, ButtonState::Released));
        prop_assert!(!world.trace.iter().any(|effect| matches!(effect, Effect::Apply(_) | Effect::SpeechCommand(_) | Effect::ConfirmSecret(_) | Effect::QueryPlugin(Event::Button { .. }) | Effect::DispatchPlugin { event: Event::Button { .. }, .. })), "mouse buttons stay in core");
        let clicks: Vec<_> = world.trace.into_iter().filter(|effect| matches!(effect, Effect::Click { .. })).collect();
        let button = match name { ButtonName::South => Some(PointerButton::Left), ButtonName::East => Some(PointerButton::Right), ButtonName::North | ButtonName::West | ButtonName::LeftBumper | ButtonName::RightBumper | ButtonName::Select | ButtonName::Record | ButtonName::Start | ButtonName::Guide | ButtonName::LeftStick | ButtonName::RightStick | ButtonName::DpadUp | ButtonName::DpadDown | ButtonName::DpadLeft | ButtonName::GuideTap | ButtonName::KeyboardPtt | ButtonName::Paddle1 | ButtonName::Paddle2 | ButtonName::Paddle3 | ButtonName::Paddle4 | ButtonName::DpadRight => None };
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
        let state = world.run(state, Input::Tick { elapsed: Duration::from_secs(2) });
        let state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
        let name = if right { ButtonName::East } else { ButtonName::South };
        let button = if right { PointerButton::Right } else { PointerButton::Left };
        let state = world.run(state, pad_button(name, ButtonState::Pressed));
        world.trace.clear();
        let state = if disconnect {
            world.run(state, Input::ControllerDisconnected)
        } else {
            let state = world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed));
            world.run(state, Input::Tick { elapsed: Duration::from_secs(2) })
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
        let state = world.run(state, Input::Tick { elapsed: Duration::from_secs(2) });
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
        let mut state = world.run(state, Input::Tick { elapsed: Duration::from_secs(2) });
        let mut distances = Vec::new();
        for axis in [small, large] {
            let axis = if negative { axis.saturating_neg() } else { axis };
            state = world.run(state, Input::Sticks(Sticks { left: (axis, 0), right: (0, axis) }));
            world.trace.clear();
            state = world.run(state, Input::Tick { elapsed: Duration::from_millis(10) });
            let motion = world.trace.iter().find_map(|effect| if let Effect::MovePointer { x, y: 0 } = effect { Some(*x) } else { None }).unwrap();
            prop_assert_eq!(motion.signum(), if negative { -1 } else { 1 });
            prop_assert!(world.trace.iter().any(|effect| matches!(effect, Effect::Scroll { x: 0, y } if y.signum() == motion.signum())), "right stick scrolls independently");
            distances.push(motion.abs());
        }
        prop_assert!(distances[0] < distances[1]);
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
        let state = world.run(state, Input::Tick { elapsed: Duration::from_secs(2) });
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
        let state = world.run(state, Input::Tick { elapsed: Duration::from_secs(2) });
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
    let state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_secs(2),
        },
    );
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
    for name in [ButtonName::Select, ButtonName::Guide, ButtonName::Start] {
        state = world.run(state, pad_button(name, ButtonState::Pressed));
    }
    let state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_secs(3),
        },
    );
    assert!(
        world.trace.contains(&Effect::MouseMode(true)),
        "View reaches its threshold"
    );
    assert!(state.core().menu().is_some(), "the system menu stays available");
}

#[test]
fn the_guide_hold_remains_reserved_inside_mouse_mode() {
    let mut world = World::default();
    let state = world.run(
        State::default(),
        pad_button(ButtonName::Select, ButtonState::Pressed),
    );
    let state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_secs(2),
        },
    );
    let state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
    world.trace.clear();
    let state = world.run(state, pad_button(ButtonName::Guide, ButtonState::Pressed));
    let state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_secs(3),
        },
    );
    assert!(
        state.core().menu().is_some(),
        "holding Guide opens the system menu"
    );
    assert_eq!(
        world
            .trace
            .iter()
            .filter(|effect| matches!(effect, Effect::Apply(_)))
            .collect::<Vec<_>>(),
        Vec::<&Effect>::new(),
        "the menu hold never sends keys to the app"
    );
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
    settings_event(
        world,
        state,
        Event::Button {
            name: ButtonName::South,
            state: ButtonState::Pressed,
        },
    )
}

fn settings_button(world: &mut World, state: State, name: ButtonName) -> State {
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
    assert_eq!(state.settings().view, super::SettingsView::Display);
    world.trace.clear();
    for _ in 0..5 {
        state = settings_button(&mut world, state, ButtonName::DpadDown);
    }
    assert_eq!(state.settings().selected, 1);
    state = settings_button(&mut world, state, ButtonName::DpadLeft);
    assert_eq!(state.settings().selected, 1);
    for _ in 0..5 {
        state = settings_button(&mut world, state, ButtonName::DpadUp);
    }
    assert_eq!(state.settings().selected, 0);
    state = settings_button(&mut world, state, ButtonName::DpadRight);
    assert_eq!(state.settings().selected, 0);
    assert!(state.settings().frequency_group);
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::Apply(_) | Effect::ConfirmSecret(_)))
    );
    state = settings_button(&mut world, state, ButtonName::East);
    assert!(state.settings().open);
    assert_eq!(state.settings().view, super::SettingsView::Home);
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
        let state = settings_event(&mut world, state, event.clone());
        assert!(world.trace.contains(&Effect::SettingsMode(0)));
        assert_eq!(state.settings().open, matches!(event, Event::Button { .. }));
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
    assert_eq!(state.settings().button_hint(), "D-PAD: MOVE   A: APPLY   B: BACK");
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

fn pointer_mode(world: &mut World) -> State {
    let state = world.run(
        State::default(),
        pad_button(ButtonName::Select, ButtonState::Pressed),
    );
    let state = world.run(
        state,
        Input::Tick {
            elapsed: Duration::from_secs(2),
        },
    );
    world.run(state, pad_button(ButtonName::Select, ButtonState::Released))
}

#[test]
fn full_stick_starts_at_two_second_4k_speed_and_ramps_to_one_second_speed() {
    let mut world = World::default();
    let mut state = pointer_mode(&mut world);
    for axis in [1000, -1000] {
        state = world.run(
            state,
            Input::Sticks(Sticks {
                left: (axis, 0),
                right: (0, 0),
            }),
        );
        world.trace.clear();
        state = world.run(
            state,
            Input::Tick {
                elapsed: Duration::from_millis(1),
            },
        );
        let first = world
            .trace
            .iter()
            .find_map(|effect| {
                if let Effect::MovePointer { x, .. } = effect {
                    Some(x.abs())
                } else {
                    None
                }
            })
            .unwrap();
        assert!(
            (1_920_000..1_925_000).contains(&first),
            "initial speed is about 1920 pixels per second"
        );
        for _ in 0..5 {
            state = world.run(
                state,
                Input::Tick {
                    elapsed: Duration::from_millis(100),
                },
            );
        }
        world.trace.clear();
        state = world.run(
            state,
            Input::Tick {
                elapsed: Duration::from_millis(10),
            },
        );
        assert!(
            world.trace.contains(&Effect::MovePointer {
                x: 38_400_000_i32.saturating_mul(i32::from(axis.signum())),
                y: 0
            }),
            "held speed is 3840 pixels per second"
        );
        state = world.run(state, Input::Sticks(Sticks::default()));
    }
}

#[test]
fn fine_aiming_keeps_its_existing_speed() {
    let mut world = World::default();
    let state = pointer_mode(&mut world);
    let state = world.run(
        state,
        Input::Sticks(Sticks {
            left: (200, 0),
            right: (0, 0),
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
        world.trace.contains(&Effect::MovePointer { x: 400_000, y: 0 }),
        "fine aiming keeps the existing response"
    );
}

#[test]
fn full_stick_ramp_covers_the_same_distance_at_different_tick_rates() {
    for millis in [1_u64, 10, 100] {
        let mut world = World::default();
        let state = world.run(
            State::default(),
            pad_button(ButtonName::Select, ButtonState::Pressed),
        );
        let state = world.run(
            state,
            Input::Tick {
                elapsed: Duration::from_secs(2),
            },
        );
        let mut state = world.run(
            state,
            Input::Sticks(Sticks {
                left: (1000, 0),
                right: (0, 0),
            }),
        );
        world.trace.clear();
        for _ in 0..500_u64.saturating_div(millis) {
            state = world.run(
                state,
                Input::Tick {
                    elapsed: Duration::from_millis(millis),
                },
            );
        }
        let distance: i64 = world
            .trace
            .iter()
            .filter_map(|effect| {
                if let Effect::MovePointer { x, y: 0 } = effect {
                    Some(i64::from(*x))
                } else {
                    None
                }
            })
            .sum();
        assert_eq!(
            distance, 1_440_000_000,
            "the half-second ramp travels 1440 pixels at every tick rate"
        );
    }
}

#[test]
fn wallpaper_section_applies_immediately_and_returns_to_settings() -> Result<(), Box<dyn core::error::Error>>
{
    let mut world = World::default();
    let state = world.run(
        State::default(),
        Input::Wallpapers {
            names: vec!["apteribis".into(), "bostrychia".into()],
            current: Some(0),
        },
    );
    let mut state = settings_event(
        &mut world,
        state,
        Event::Speech {
            text: Text::new("Pop the hood.".into())?,
        },
    );
    for name in [
        ButtonName::DpadDown,
        ButtonName::South,
        ButtonName::DpadDown,
        ButtonName::South,
    ] {
        state = settings_button(&mut world, state, name);
    }
    assert_eq!(state.settings().view, super::SettingsView::Wallpaper);
    assert_eq!(state.settings().wallpaper_current, Some(1));
    assert!(world.trace.contains(&Effect::Wallpaper(1)));
    assert_eq!(state.settings().remaining, None);
    state = settings_button(&mut world, state, ButtonName::East);
    assert!(state.settings().open);
    assert_eq!(state.settings().view, super::SettingsView::Home);
    Ok(())
}

#[test]
fn unlock_phrase_is_consumed_before_plugins_and_contexts_and_can_be_configured() -> Result<(), String> {
    for (phrases, accepted, rejected) in [
        (
            super::Phrases::default(),
            "BIRDS, of a feather!",
            "birds of feather",
        ),
        (
            super::Phrases::new("settings", "all art")?,
            "All art.",
            "birds of a feather",
        ),
    ] {
        let mut world = World::default();
        let state = world.run(State::default(), Input::Phrases(phrases));
        let state = settings_event(
            &mut world,
            state,
            Event::Speech {
                text: Text::new(accepted.into())?,
            },
        );
        assert!(world.trace.contains(&Effect::UnlockWallpapers));
        assert!(!world.trace.iter().any(|effect| matches!(
            effect,
            Effect::QueryPlugin(Event::Speech { .. })
                | Effect::DispatchPlugin {
                    event: Event::Speech { .. },
                    ..
                }
                | Effect::SpeechRouted { .. }
                | Effect::Apply(_)
        )));
        world.trace.clear();
        let _state = settings_event(
            &mut world,
            state,
            Event::Speech {
                text: Text::new(rejected.into())?,
            },
        );
        assert!(!world.trace.contains(&Effect::UnlockWallpapers));
    }
    Ok(())
}

#[test]
fn frequency_choices_follow_size_and_keep_the_available_rate() {
    let mut world = World::default();
    let mut state = open_display(&mut world);
    assert_eq!(state.settings().rates(), [0]);
    state = settings_button(&mut world, state, ButtonName::DpadRight);
    state = settings_button(&mut world, state, ButtonName::DpadDown);
    assert_eq!(state.settings().selected, 0);
    state = settings_button(&mut world, state, ButtonName::DpadLeft);
    state = settings_button(&mut world, state, ButtonName::DpadDown);
    assert_eq!(state.settings().sizes(), [(1920, 1080), (3840, 2160)]);
    assert_eq!(state.settings().rates(), [1, 2]);
    state = settings_button(&mut world, state, ButtonName::DpadRight);
    state = settings_button(&mut world, state, ButtonName::DpadDown);
    assert_eq!(state.settings().selected, 2);
    state = settings_button(&mut world, state, ButtonName::DpadLeft);
    state = settings_button(&mut world, state, ButtonName::DpadUp);
    assert_eq!(state.settings().selected, 0);
    assert_eq!(state.settings().rates(), [0]);
}

#[test]
fn locked_wallpaper_consumes_apply_without_selecting_until_unlocked() {
    let mut world = World::default();
    let state = world.run(
        State::default(),
        Input::Wallpapers {
            names: vec!["apteribis".into(), "bostrychia".into()],
            current: Some(0),
        },
    );
    let state = world.run(state, Input::WallpaperLocks(vec![false, true]));
    let mut state = settings_event(
        &mut world,
        state,
        Event::Speech {
            text: Text::new("Pop the hood.".into()).unwrap(),
        },
    );
    for name in [
        ButtonName::DpadDown,
        ButtonName::South,
        ButtonName::DpadRight,
        ButtonName::South,
    ] {
        state = settings_button(&mut world, state, name);
    }
    assert_eq!(state.settings().wallpaper_current, Some(0));
    assert!(!world.trace.contains(&Effect::Wallpaper(1)));
    state = world.run(state, Input::WallpaperLocks(vec![false, false]));
    state = settings_button(&mut world, state, ButtonName::South);
    assert_eq!(state.settings().wallpaper_current, Some(1));
    assert!(world.trace.contains(&Effect::Wallpaper(1)));
}

#[test]
fn context_sticks_repeat_keys_and_stop_on_focus_change() {
    use tehuti_protocol::KeyName;
    let context = tehuti_protocol::parse_context(
        "app_id = 'mouse-test'\n[buttons]\n[sticks]\nleft_up = { key = 'up' }\nright_down = { key = 'down' }",
    )
    .unwrap();
    let tick = || Input::Tick {
        elapsed: Duration::from_millis(180),
    };
    let mut world = World::default();
    let state = world.run(
        configured_mouse(vec![context]),
        Input::Sticks(Sticks {
            left: (0, -1000),
            right: (0, 1000),
        }),
    );
    world.trace.clear();
    let state = world.run(state, tick());
    assert!(
        world
            .trace
            .contains(&Effect::Apply(Action::Key { key: KeyName::Up })),
        "left stick sends its mapped key"
    );
    assert!(
        world
            .trace
            .contains(&Effect::Apply(Action::Key { key: KeyName::Down })),
        "right stick sends its mapped key"
    );
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::Scroll { .. })),
        "mapped sticks do not also scroll"
    );
    let state = world.run(
        state,
        Input::Event {
            event: Event::Focus {
                app_id: None,
                title: String::new(),
            },
            elapsed: Duration::ZERO,
        },
    );
    world.trace.clear();
    let _state = world.run(state, tick());
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::Apply(Action::Key { .. }))),
        "keys stop when the app loses focus"
    );
}

#[test]
fn context_sticks_keep_the_mouse_mode_exit_hold() {
    let context = tehuti_protocol::parse_context(
        "app_id = 'mouse-test'\n[buttons]\nsouth = { key = 'enter' }\n[sticks]\nleft_up = { key = 'up' }",
    )
    .unwrap();
    let mut world = World::default();
    let mut state = configured_mouse(vec![context]);
    for active in [true, false] {
        state = world.run(state, pad_button(ButtonName::Select, ButtonState::Pressed));
        world.trace.clear();
        state = world.run(
            state,
            Input::Tick {
                elapsed: Duration::from_secs(2),
            },
        );
        assert!(
            world.trace.contains(&Effect::MouseMode(active)),
            "View still toggles mouse mode in a stick context"
        );
        state = world.run(state, pad_button(ButtonName::Select, ButtonState::Released));
    }
    world.trace.clear();
    let _state = world.run(state, pad_button(ButtonName::South, ButtonState::Pressed));
    assert!(
        world.trace.contains(&Effect::Apply(Action::Key {
            key: tehuti_protocol::KeyName::Enter
        })),
        "pad mappings work after leaving mouse mode"
    );
}

#[test]
fn start_ptt_setting_replaces_its_binding_only_when_listed() {
    for (setting, enabled) in [("['record', 'start']", true), ("['record']", false)] {
        let context = tehuti_protocol::parse_context(&format!(
            "app_id = '*'\npush_to_talk = {setting}\n[buttons]\nstart = {{ key = 'enter' }}"
        ))
        .unwrap();
        let mut world = World::default();
        let state = State::new(crate::State::with_contexts(vec![context]));
        let state = world.run(state, Input::Speech(SpeechInput::Available));
        let state = world.run(state, button(ButtonName::Start, ButtonState::Pressed));
        let _state = world.run(state, button(ButtonName::Start, ButtonState::Released));
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
        if enabled {
            assert_eq!(
                commands,
                vec![
                    SpeechEffect::Start { session: 0 },
                    SpeechEffect::Stop {
                        session: 0,
                        transcribe: true
                    }
                ]
            );
        } else {
            assert_eq!(commands, Vec::<SpeechEffect>::new());
        }
        assert_eq!(
            world.trace.contains(&Effect::Apply(Action::Key {
                key: tehuti_protocol::KeyName::Enter
            })),
            !enabled
        );
    }
}

#[test]
fn keyboard_ptt_obeys_the_menu_rule() {
    let mut world = World::default();
    let state = menu_open(&mut world);
    let _state = world.run(state, button(ButtonName::KeyboardPtt, ButtonState::Pressed));
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::SpeechCommand(_)))
    );
}

#[test]
fn keyboard_ptt_captures_with_controller_ptt_disabled() {
    let context = tehuti_protocol::parse_context("app_id = '*'\npush_to_talk = []\n[buttons]").unwrap();
    let mut world = World::default();
    let state = State::new(crate::State::with_contexts(vec![context]));
    let state = world.run(state, Input::Speech(SpeechInput::Available));
    let state = world.run(state, button(ButtonName::KeyboardPtt, ButtonState::Pressed));
    let _state = world.run(state, button(ButtonName::KeyboardPtt, ButtonState::Released));
    assert!(
        world
            .trace
            .contains(&Effect::SpeechCommand(SpeechEffect::Start { session: 0 }))
    );
    assert!(world.trace.contains(&Effect::SpeechCommand(SpeechEffect::Stop {
        session: 0,
        transcribe: true
    })));
}

#[test]
fn audio_refreshes_on_entry_and_applies_without_a_preview() {
    let mut world = World::default();
    let mut state = open_display(&mut world);
    state = settings_button(&mut world, state, ButtonName::East);
    for _ in 0..2 {
        state = settings_button(&mut world, state, ButtonName::DpadDown);
    }
    world.trace.clear();
    state = settings_button(&mut world, state, ButtonName::South);
    assert_eq!(state.settings().view, super::SettingsView::Audio);
    assert!(world.trace.contains(&Effect::AudioRefresh));
    assert_eq!(state.settings().sinks, None);
    state = world.run(
        state,
        Input::AudioSinks(Some(vec![
            super::Sink {
                sink_id: 52,
                description: "Headset".into(),
                kind: super::SinkKind::Headset,
                is_default: true,
            },
            super::Sink {
                sink_id: 61,
                description: "HDMI".into(),
                kind: super::SinkKind::Hdmi,
                is_default: false,
            },
        ])),
    );
    state = settings_button(&mut world, state, ButtonName::DpadRight);
    world.trace.clear();
    state = settings_button(&mut world, state, ButtonName::South);
    assert!(world.trace.contains(&Effect::AudioDefault(61)));
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::SettingsMode(_) | Effect::PersistMode(_)))
    );
    assert_eq!(state.settings().remaining, None);
    state = settings_button(&mut world, state, ButtonName::East);
    world.trace.clear();
    state = settings_button(&mut world, state, ButtonName::South);
    assert!(world.trace.contains(&Effect::AudioRefresh));
    state = world.run(state, Input::AudioSinks(None));
    world.trace.clear();
    state = settings_button(&mut world, state, ButtonName::South);
    assert_eq!(state.settings().sinks, None);
    assert!(
        !world
            .trace
            .iter()
            .any(|effect| matches!(effect, Effect::AudioDefault(_)))
    );
}
