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
                        if let Some(event) = self.close_focus.take() {
                            state = self.run(
                                state,
                                Input::Event {
                                    event,
                                    elapsed: Duration::from_secs(9),
                                },
                            );
                        }
                    }
                    Effect::PollSources
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
    assert_eq!(state.core().bar().segments().len(), 1);
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
