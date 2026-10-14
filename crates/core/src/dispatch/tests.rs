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
}

impl World {
    fn run(&mut self, state: State, input: Input) -> State {
        let depth = state.busy();
        let (mut state, mut effects) = step(state, input);
        loop {
            let mut reply = Input::Resume;
            for effect in effects {
                self.trace.push(effect.clone());
                match effect {
                    Effect::QueryPlugin(_) => reply = Input::PluginActive(self.plugin_active),
                    Effect::DispatchPlugin { .. } => reply = Input::PluginRequest(None),
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
        plugin_active in prop::bool::ANY,
        secret_consumes in prop::bool::ANY,
    ) {
        let mut world = World { plugin_active, secret_consumes, ..World::default() };
        let mut state = State::default();
        for event in events {
            state = world.run(state, Input::Event { event, elapsed: Duration::from_millis(16) });
            prop_assert_eq!(state.busy(), 0);
        }
    }

    #[test]
    fn input_events_tick_before_they_route(event in arbitrary_button(), secret_consumes in prop::bool::ANY) {
        let mut world = World { secret_consumes, ..World::default() };
        let _state = world.run(State::default(), Input::Event { event: event.clone(), elapsed: Duration::ZERO });
        let trace = &world.trace;
        let ticked_first = matches!(trace.first(), Some(Effect::QueryPlugin(Event::Tick { .. })));
        prop_assert!(ticked_first);
        let poll = position(trace, |effect| *effect == Effect::PollSources);
        let routed = trace.iter().position(|effect| *effect == Effect::QueryPlugin(event.clone()));
        let record = event_is(&event, ButtonName::Record);
        let south = event_is(&event, ButtonName::South);
        if record || (south && secret_consumes) {
            prop_assert!(routed.is_none());
        } else {
            prop_assert!(routed.is_some_and(|routed| poll < routed));
        }
        if south {
            let pressed = position(trace, |effect| matches!(effect, Effect::SouthPressed(_)));
            let confirm = position(trace, |effect| matches!(effect, Effect::ConfirmSecret(_)));
            prop_assert!(poll < pressed && pressed < confirm);
        }
    }

    #[test]
    fn without_plugins_dispatch_applies_what_the_step_returns(
        events in prop::collection::vec(arbitrary_event(), 0..40),
    ) {
        let mut world = World::default();
        let mut state = State::default();
        let mut reference = crate::State::default();
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
