//! An unmatched transcript resolves by the model's confidence band.

#[cfg(test)]
mod speech_resolver {
    use proptest::prelude::*;
    use tehuti_core::speech::resolver::{
        Answer, Bands, CANCEL_WINDOW, MAX_OFFERED, Outcome, Ranked, Resolution, Turn, cancelled_in_time,
        resolve,
    };

    fn commands() -> impl Strategy<Value = Vec<String>> {
        proptest::collection::hash_set("[a-z]{1,8}", 1..8).prop_map(|set| set.into_iter().collect())
    }

    fn bands() -> impl Strategy<Value = Bands> {
        (0u16..=1000, 0u16..=1000).prop_map(|(high, low)| Bands {
            run_from: high.max(low),
            offer_from: high.min(low),
        })
    }

    fn answer_over(commands: Vec<String>) -> impl Strategy<Value = Answer> {
        let count = commands.len();
        (
            0..count,
            0u16..=1000,
            proptest::collection::vec(0u16..=1000, count..=count),
        )
            .prop_map(move |(chosen, confidence, probabilities)| Answer {
                command: commands.get(chosen).cloned().unwrap_or_default(),
                confidence,
                ranked: commands
                    .iter()
                    .cloned()
                    .zip(probabilities)
                    .map(|(command, probability)| Ranked { command, probability })
                    .collect(),
            })
    }

    fn scenario() -> impl Strategy<Value = (Vec<String>, Answer, Bands)> {
        commands().prop_flat_map(|commands| (Just(commands.clone()), answer_over(commands), bands()))
    }

    proptest! {
        #[test]
        fn no_answer_dictates(commands in commands(), bands in bands(), heard in ".{0,20}") {
            prop_assert_eq!(resolve(&heard, &commands, None, bands), Resolution::Dictate);
        }

        #[test]
        fn the_band_decides((commands, answer, bands) in scenario()) {
            let resolution = resolve("open the thing", &commands, Some(&answer), bands);
            if answer.confidence >= bands.run_from {
                prop_assert_eq!(resolution, Resolution::Run(answer.command));
            } else if answer.confidence < bands.offer_from {
                prop_assert_eq!(resolution, Resolution::Dictate);
            } else {
                match resolution {
                    Resolution::Offer(offered) => {
                        prop_assert!((2..=MAX_OFFERED).contains(&offered.len()));
                        prop_assert_eq!(offered.first(), Some(&answer.command));
                        prop_assert!(offered.iter().all(|command| commands.contains(command)));
                        let unique: std::collections::HashSet<_> = offered.iter().collect();
                        prop_assert_eq!(unique.len(), offered.len());
                    }
                    Resolution::Dictate => prop_assert!(commands.len() < 2),
                    Resolution::Run(_) => prop_assert!(false, "a middle band never runs"),
                }
            }
        }

        #[test]
        fn a_command_outside_the_context_dictates((commands, mut answer, bands) in scenario()) {
            answer.command = "0 not a command".into();
            prop_assert_eq!(resolve("open", &commands, Some(&answer), bands), Resolution::Dictate);
        }

        #[test]
        fn an_empty_transcript_dictates((commands, answer, bands) in scenario(), blanks in " {0,4}") {
            prop_assert_eq!(resolve(&blanks, &commands, Some(&answer), bands), Resolution::Dictate);
        }
    }

    #[test]
    fn the_offer_lists_the_likeliest_runners_up_after_the_choice() {
        let commands: Vec<String> = ["mute", "volume", "pause", "stop"].map(String::from).into();
        let ranked = [("mute", 100), ("volume", 500), ("pause", 300), ("stop", 400)]
            .map(|(command, probability)| Ranked {
                command: command.into(),
                probability,
            })
            .into();
        let answer = Answer {
            command: "volume".into(),
            confidence: 600,
            ranked,
        };
        assert_eq!(
            resolve("louder", &commands, Some(&answer), Bands::default()),
            Resolution::Offer(vec!["volume".into(), "stop".into(), "pause".into()])
        );
    }

    #[test]
    fn the_turn_records_what_was_heard_chosen_and_run() {
        let answer = Answer {
            command: "mute".into(),
            confidence: 900,
            ranked: Vec::new(),
        };
        let resolution = Resolution::Run("mute".into());
        let turn = Turn::new("quiet please", Some(&answer), &resolution);
        assert_eq!(turn.heard, "quiet please");
        assert_eq!(turn.chosen, Some(("mute".into(), 900)));
        assert_eq!(turn.outcome, Outcome::Ran("mute".into()));
    }

    #[test]
    fn an_undo_inside_the_window_is_a_cancellation() {
        assert!(cancelled_in_time(CANCEL_WINDOW));
        assert!(!cancelled_in_time(
            CANCEL_WINDOW.saturating_add(core::time::Duration::from_millis(1))
        ));
    }

    mod fall_through {
        use proptest::prelude::*;
        use tehuti_core::{State, step, unmatched_speech_commands};
        use tehuti_protocol::{Event, parse_context};

        fn focused(phrases: &[String], unmatched: bool) -> State {
            let mut source = String::from("app_id = 'editor'\n");
            if unmatched {
                source.push_str("unmatched_speech = 'type'\n");
            }
            source.push_str("[buttons]\n[speech]\n");
            for phrase in phrases {
                source.push_str(&format!("'{phrase}' = {{ notify = 'ok' }}\n"));
            }
            let state = State::with_contexts(vec![parse_context(&source).unwrap()]);
            step(
                state,
                &Event::Focus {
                    app_id: Some("editor".into()),
                    title: String::new(),
                },
            )
            .0
        }

        proptest! {
            #[test]
            fn a_mapped_phrase_never_goes_to_the_model(
                phrases in proptest::collection::hash_set("[a-z]{4,8}", 1..6),
                unmatched in any::<bool>(),
            ) {
                let phrases: Vec<String> = phrases.into_iter().collect();
                let state = focused(&phrases, unmatched);
                for phrase in &phrases {
                    prop_assert_eq!(unmatched_speech_commands(&state, phrase), None);
                }
            }

            #[test]
            fn an_unknown_phrase_offers_every_mapped_phrase_once(
                phrases in proptest::collection::hash_set("[a-z]{4,8}", 1..6),
                unmatched in any::<bool>(),
            ) {
                let phrases: Vec<String> = phrases.into_iter().collect();
                let state = focused(&phrases, unmatched);
                let offered = unmatched_speech_commands(&state, "ZZZ 123 unlisted").unwrap();
                let mut expected = phrases.clone();
                expected.sort();
                prop_assert_eq!(offered, expected);
            }
        }

        #[test]
        fn nothing_is_offered_when_no_phrase_is_mapped() {
            assert_eq!(
                unmatched_speech_commands(&focused(&[], true), "hello there"),
                None
            );
        }
    }
}
