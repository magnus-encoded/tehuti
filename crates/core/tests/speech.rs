//! Capture laws over synthetic lifecycle disruptions.

#[cfg(test)]
mod speech {
    use std::collections::BTreeSet;

    use proptest::prelude::*;
    use tehuti_core::speech::{SpeechEffect, SpeechFailure, SpeechInput, SpeechState, speech_step};
    use tehuti_protocol::Text;

    #[test]
    fn failure_reason_reaches_the_effect() {
        for reason in [SpeechFailure::Silent, SpeechFailure::Transcription] {
            let mut state = SpeechState::default();
            for step in [SpeechInput::Available, SpeechInput::Press, SpeechInput::Release] {
                state = speech_step(state, step).0;
            }
            let (_, effects) = speech_step(state, SpeechInput::Failed { session: 0, reason });
            assert_eq!(effects, vec![SpeechEffect::Failed { session: 0, reason }]);
        }
    }

    fn input() -> impl Strategy<Value = SpeechInput> {
        prop_oneof![
            Just(SpeechInput::Press),
            Just(SpeechInput::Release),
            Just(SpeechInput::Cancel),
            Just(SpeechInput::Disconnected),
            Just(SpeechInput::Unavailable),
            Just(SpeechInput::Available),
            (0u64..100, any::<Text>()).prop_map(|(session, text)| SpeechInput::Transcript { session, text }),
            (0u64..100, proptest::bool::ANY).prop_map(|(session, silent)| SpeechInput::Failed {
                session,
                reason: if silent {
                    SpeechFailure::Silent
                } else {
                    SpeechFailure::Transcription
                },
            }),
        ]
    }

    proptest! {
        #[test]
        fn every_capture_has_one_stop_and_only_released_captures_deliver_text(
            inputs in prop::collection::vec(input(), 0..300),
        ) {
            let mut state = SpeechState::default();
            let mut active = None;
            let mut started = BTreeSet::new();
            let mut stopped = BTreeSet::new();
            let mut eligible = None;
            let mut delivered = BTreeSet::new();
            for input in inputs.into_iter().chain([SpeechInput::Disconnected]) {
                if matches!(input, SpeechInput::Cancel | SpeechInput::Disconnected | SpeechInput::Unavailable) {
                    eligible = None;
                }
                let (next, emitted) = speech_step(state, input);
                state = next;
                for effect in emitted {
                    match effect {
                        SpeechEffect::Start { session } => {
                            prop_assert!(active.is_none());
                            prop_assert!(started.insert(session));
                            active = Some(session);
                            eligible = None;
                        }
                        SpeechEffect::Stop { session, transcribe } => {
                            prop_assert_eq!(active.take(), Some(session));
                            prop_assert!(stopped.insert(session));
                            eligible = transcribe.then_some(session);
                        }
                        SpeechEffect::Speech { session, .. } | SpeechEffect::Failed { session, .. } => {
                            prop_assert_eq!(eligible.take(), Some(session));
                            prop_assert!(delivered.insert(session));
                        }
                    }
                }
            }
            prop_assert!(active.is_none());
            prop_assert_eq!(started, stopped);
        }

        #[test]
        fn cancelled_or_crashed_capture_waits_for_a_new_physical_press(
            disruptions in prop::collection::vec(prop_oneof![Just(SpeechInput::Cancel), Just(SpeechInput::Unavailable)], 1..30),
        ) {
            let (state, _) = speech_step(SpeechState::default(), SpeechInput::Available);
            let (mut state, start) = speech_step(state, SpeechInput::Press);
            prop_assert_eq!(start, vec![SpeechEffect::Start { session: 0 }]);
            let mut stops = Vec::new();
            for disruption in disruptions {
                let (cancelled_state, cancelled) = speech_step(state, disruption);
                state = cancelled_state;
                stops.extend(cancelled);
                let (available_state, available_effects) = speech_step(state, SpeechInput::Available);
                state = available_state;
                prop_assert!(available_effects.is_empty());
                let (held_state, held_press) = speech_step(state, SpeechInput::Press);
                state = held_state;
                prop_assert!(held_press.is_empty());
            }
            prop_assert_eq!(stops.len(), 1);
            let (state, released) = speech_step(state, SpeechInput::Release);
            prop_assert!(released.is_empty());
            let (_, fresh_press) = speech_step(state, SpeechInput::Press);
            prop_assert_eq!(fresh_press, vec![SpeechEffect::Start { session: 1 }]);
        }

        #[test]
        fn released_transcript_is_delivered_once_and_preserves_text(text in any::<Text>()) {
            let (state, _) = speech_step(SpeechState::default(), SpeechInput::Available);
            let (state, _) = speech_step(state, SpeechInput::Press);
            let (state, stop) = speech_step(state, SpeechInput::Release);
            prop_assert_eq!(stop, vec![SpeechEffect::Stop { session: 0, transcribe: true }]);
            let input = SpeechInput::Transcript { session: 0, text: text.clone() };
            let (state, transcript) = speech_step(state, input.clone());
            prop_assert_eq!(transcript, vec![SpeechEffect::Speech { session: 0, text }]);
            let (_, duplicate) = speech_step(state, input);
            prop_assert!(duplicate.is_empty());
        }
    }

    proptest! {
        #[test]
        fn disruption_discards_in_flight_transcript(
            text in any::<Text>(),
            disruption in prop_oneof![Just(SpeechInput::Cancel), Just(SpeechInput::Disconnected), Just(SpeechInput::Unavailable)],
            released in any::<bool>(),
        ) {
            let (state, _) = speech_step(SpeechState::default(), SpeechInput::Available);
            let (mut state, _) = speech_step(state, SpeechInput::Press);
            if released {
                (state, _) = speech_step(state, SpeechInput::Release);
            }
            let (state, cancelled) = speech_step(state, disruption);
            prop_assert_eq!(cancelled, if released { vec![] } else { vec![SpeechEffect::Stop { session: 0, transcribe: false }] });
            let (_, stale) = speech_step(state, SpeechInput::Transcript { session: 0, text });
            prop_assert!(stale.is_empty());
        }

        #[test]
        fn only_current_released_session_accepts_a_reply(text in any::<Text>(), wrong_session in 2u64..u64::MAX) {
            let (mut state, _) = speech_step(SpeechState::default(), SpeechInput::Available);
            for input in [SpeechInput::Press, SpeechInput::Release, SpeechInput::Press, SpeechInput::Release] {
                (state, _) = speech_step(state, input);
            }
            for session in [0, wrong_session] {
                let (next, stale) = speech_step(state, SpeechInput::Transcript { session, text: text.clone() });
                state = next;
                prop_assert!(stale.is_empty());
            }
            let (_, current) = speech_step(state, SpeechInput::Transcript { session: 1, text: text.clone() });
            prop_assert_eq!(current, vec![SpeechEffect::Speech { session: 1, text }]);
        }

        #[test]
        fn disconnected_controller_can_begin_a_new_capture_without_old_release(text in any::<Text>()) {
            let (state, _) = speech_step(SpeechState::default(), SpeechInput::Available);
            let (state, _) = speech_step(state, SpeechInput::Press);
            let (state, disconnected) = speech_step(state, SpeechInput::Disconnected);
            prop_assert_eq!(disconnected, vec![SpeechEffect::Stop { session: 0, transcribe: false }]);
            let (state, reconnected) = speech_step(state, SpeechInput::Press);
            prop_assert_eq!(reconnected, vec![SpeechEffect::Start { session: 1 }]);
            let (_, stale) = speech_step(state, SpeechInput::Transcript { session: 0, text });
            prop_assert!(stale.is_empty());
        }
    }
    proptest! {
        #[test]
        fn unavailable_speech_never_captures_or_resumes_a_held_press(
            repeats in 1usize..100,
        ) {
            let mut state = SpeechState::default();
            for _attempt in 0..repeats {
                let (next, effects) = speech_step(state, SpeechInput::Press);
                state = next;
                prop_assert!(effects.is_empty());
            }
            let (state, available_effects) = speech_step(state, SpeechInput::Available);
            prop_assert!(available_effects.is_empty());
            let (state, held_effects) = speech_step(state, SpeechInput::Press);
            prop_assert!(held_effects.is_empty());
            let (state, released_effects) = speech_step(state, SpeechInput::Release);
            prop_assert!(released_effects.is_empty());
            let (_, fresh_effects) = speech_step(state, SpeechInput::Press);
            prop_assert_eq!(fresh_effects, vec![SpeechEffect::Start { session: 0 }]);
        }
    }
}
