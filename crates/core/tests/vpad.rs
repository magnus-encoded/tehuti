//! Virtual pad overrides: matched inputs go to a plugin, the rest pass through in order.

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tehuti_core::vpad::{Filter, Verdict};
    use tehuti_protocol::{ButtonName, ButtonState, PadOverride};

    const fn press(button: ButtonName) -> (ButtonName, ButtonState) {
        (button, ButtonState::Pressed)
    }

    const fn release(button: ButtonName) -> (ButtonName, ButtonState) {
        (button, ButtonState::Released)
    }

    fn input() -> impl Strategy<Value = (ButtonName, ButtonState)> {
        (any::<ButtonName>(), any::<bool>()).prop_map(|(button, pressed)| {
            (
                button,
                if pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                },
            )
        })
    }

    fn chord() -> PadOverride {
        PadOverride {
            button: ButtonName::South,
            held: vec![ButtonName::LeftBumper],
        }
    }

    #[test]
    fn a_chord_press_and_its_release_go_to_the_plugin() {
        let mut filter = Filter::default();
        filter.set(3, vec![chord()]);
        let series = [
            press(ButtonName::LeftBumper),
            press(ButtonName::South),
            release(ButtonName::LeftBumper),
            release(ButtonName::South),
            press(ButtonName::South),
            release(ButtonName::South),
        ];
        let verdicts: Vec<_> = series
            .iter()
            .map(|&(button, state)| filter.step(button, state, &[3], &[]))
            .collect();
        assert_eq!(
            verdicts,
            [
                Verdict::Pass,
                Verdict::Plugin(3),
                Verdict::Pass,
                Verdict::Plugin(3),
                Verdict::Pass,
                Verdict::Pass,
            ]
        );
    }

    #[test]
    fn south_can_be_overridden_alone() {
        let mut filter = Filter::default();
        filter.set(
            0,
            vec![PadOverride {
                button: ButtonName::South,
                held: Vec::new(),
            }],
        );
        assert_eq!(
            filter.step(ButtonName::South, ButtonState::Pressed, &[0], &[]),
            Verdict::Plugin(0)
        );
        assert_eq!(
            filter.step(ButtonName::South, ButtonState::Released, &[0], &[]),
            Verdict::Plugin(0)
        );
    }

    #[test]
    fn ineligible_and_cleared_plugins_never_consume() {
        let mut filter = Filter::default();
        filter.set(
            1,
            vec![PadOverride {
                button: ButtonName::East,
                held: Vec::new(),
            }],
        );
        assert_eq!(
            filter.step(ButtonName::East, ButtonState::Pressed, &[], &[]),
            Verdict::Pass
        );
        assert_eq!(
            filter.step(ButtonName::East, ButtonState::Released, &[1], &[]),
            Verdict::Pass
        );
        filter.clear(1);
        assert_eq!(
            filter.step(ButtonName::East, ButtonState::Pressed, &[1], &[]),
            Verdict::Pass
        );
    }

    #[test]
    fn the_first_eligible_plugin_wins() {
        let mut filter = Filter::default();
        let east = vec![PadOverride {
            button: ButtonName::East,
            held: Vec::new(),
        }];
        filter.set(0, east.clone());
        filter.set(2, east);
        assert_eq!(
            filter.step(ButtonName::East, ButtonState::Pressed, &[2, 0], &[]),
            Verdict::Plugin(2)
        );
    }

    proptest! {
        #[test]
        fn push_to_talk_buttons_reach_push_to_talk_whatever_the_overrides(
            overrides in prop::collection::vec(any::<PadOverride>(), 0..6),
            talk in prop::collection::vec(any::<ButtonName>(), 1..3),
            series in prop::collection::vec(input(), 0..80),
        ) {
            let mut filter = Filter::default();
            let mut covering = overrides;
            covering.extend(talk.iter().map(|&button| PadOverride { button, held: Vec::new() }));
            filter.set(0, covering);
            for (button, state) in series {
                let verdict = filter.step(button, state, &[0], &talk);
                if talk.contains(&button) {
                    prop_assert_eq!(verdict, Verdict::Pass);
                }
            }
        }

        #[test]
        fn inputs_that_match_no_override_pass_through_unchanged_and_in_order(
            overridden in prop::collection::vec(any::<ButtonName>(), 0..4),
            held in prop::collection::vec(any::<ButtonName>(), 0..3),
            series in prop::collection::vec(input(), 0..80),
        ) {
            let mut filter = Filter::default();
            filter.set(0, overridden.iter().map(|&button| PadOverride { button, held: held.clone() }).collect());
            let free: Vec<_> = series.into_iter().filter(|(button, _)| !overridden.contains(button)).collect();
            let passed: Vec<_> = free
                .iter()
                .copied()
                .filter(|&(button, state)| filter.step(button, state, &[0], &[]) == Verdict::Pass)
                .collect();
            prop_assert_eq!(passed, free);
        }

        #[test]
        fn a_release_follows_its_press(
            overrides in prop::collection::vec(any::<PadOverride>(), 0..6),
            series in prop::collection::vec(input(), 0..80),
            swap_at in any::<prop::sample::Index>(),
        ) {
            let mut filter = Filter::default();
            let swap = swap_at.index(series.len().max(1));
            let mut down = std::collections::HashMap::new();
            for (index, (button, state)) in series.into_iter().enumerate() {
                if index == swap {
                    filter.set(0, overrides.iter().rev().cloned().collect());
                } else if index == 0 {
                    filter.set(0, overrides.clone());
                }
                let verdict = filter.step(button, state, &[0], &[]);
                match state {
                    ButtonState::Pressed => {
                        if let Some(previous) = down.insert(button, verdict) {
                            prop_assert_eq!(verdict, previous);
                        }
                    }
                    ButtonState::Released => {
                        prop_assert_eq!(verdict, down.remove(&button).unwrap_or(Verdict::Pass));
                    }
                }
            }
        }
    }
}
