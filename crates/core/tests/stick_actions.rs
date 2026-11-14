//! `[sticks]`: directions fire their actions, repeat while held and respect face-button modifiers.

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use proptest::prelude::*;
    use tehuti_core::{
        Level, State, step,
        stick_actions::{FIRE_DEFLECTION, StickActions},
        ticker::{self, PadFamily},
    };
    use tehuti_protocol::{Action, ButtonName, ButtonState, Context, Event, Text, parse_context};

    const APP: &str = "org.example.Player";
    const FRAME: Duration = Duration::from_millis(16);

    const DIRECTIONS: [&str; 8] = [
        "up",
        "up-right",
        "right",
        "down-right",
        "down",
        "down-left",
        "left",
        "up-left",
    ];

    /// Unit vectors for [`DIRECTIONS`], screen up negative, in thousandths.
    const VECTORS: [(i16, i16); 8] = [
        (0, -1000),
        (707, -707),
        (1000, 0),
        (707, 707),
        (0, 1000),
        (-707, 707),
        (-1000, 0),
        (-707, -707),
    ];

    fn text(word: &str) -> Action {
        Action::Text {
            text: Text::new(word.into()).unwrap(),
        }
    }

    fn context(sticks: &str) -> Context {
        parse_context(&format!(
            "app_id = '{APP}'\n[buttons]\nnorth = {{ text = 'north' }}\n[sticks]\n{sticks}"
        ))
        .unwrap()
    }

    fn all_eight(stick: &str, extra: &str) -> Context {
        let lines: String = DIRECTIONS
            .iter()
            .map(|name| format!("{stick}.{name} = {{ text = '{name}'{extra} }}\n"))
            .collect();
        context(&lines)
    }

    fn scaled(vector: (i16, i16), depth: i16) -> (i16, i16) {
        let scale = |axis: i16| i16::try_from(i32::from(axis) * i32::from(depth) / 1000).unwrap();
        (scale(vector.0), scale(vector.1))
    }

    /// Holds `sample` on the left stick for `frames` ticks and collects what fires.
    fn hold(sticks: &mut StickActions, context: &Context, sample: (i16, i16), frames: usize) -> Vec<Action> {
        (0..frames)
            .flat_map(|_| sticks.tick(context, FRAME, sample, (0, 0)))
            .collect()
    }

    proptest! {
        #[test]
        fn each_of_eight_sectors_fires_its_own_direction(
            index in 0..8_usize,
            depth in 600..=1000_i16,
            nudge in -150..=150_i16,
        ) {
            let context = all_eight("left", "");
            let (x, y) = scaled(VECTORS[index], depth);
            // A nudge across the sector stays well inside its 45° wedge.
            let sample = if index % 2 == 0 {
                if x == 0 { (nudge, y) } else { (x, nudge) }
            } else {
                (x.saturating_add(nudge / 3), y.saturating_sub(nudge / 3))
            };
            let mut sticks = StickActions::default();
            let fired = sticks.tick(&context, FRAME, sample, (0, 0));
            prop_assert_eq!(fired, vec![text(DIRECTIONS[index])]);
        }

        #[test]
        fn an_unmapped_diagonal_falls_back_to_the_nearer_cardinal(
            quadrant in 0..4_usize,
            major in 700..=1000_i16,
            ratio in 45..=95_i16,
            horizontal_major in any::<bool>(),
        ) {
            let context = context(
                "left.up = { text = 'up' }\nleft.down = { text = 'down' }\nleft.left = { text = 'left' }\nleft.right = { text = 'right' }\n",
            );
            let minor = major * ratio / 100;
            let (sign_x, sign_y) = [(1, -1), (1, 1), (-1, 1), (-1, -1)][quadrant];
            let (x, y) = if horizontal_major { (major, minor) } else { (minor, major) };
            let sample = (x * sign_x, y * sign_y);
            let expected = match (horizontal_major, sign_x, sign_y) {
                (true, 1, _) => "right",
                (true, _, _) => "left",
                (false, _, -1) => "up",
                (false, _, _) => "down",
            };
            let mut sticks = StickActions::default();
            prop_assert_eq!(sticks.tick(&context, FRAME, sample, (0, 0)), vec![text(expected)]);
        }

        #[test]
        fn a_mapped_diagonal_wins_over_the_cardinals(index in 0..4_usize, depth in 600..=1000_i16) {
            let diagonal = [1, 3, 5, 7][index];
            let context = context(&format!(
                "left.up = {{ text = 'up' }}\nleft.right = {{ text = 'right' }}\nleft.down = {{ text = 'down' }}\nleft.left = {{ text = 'left' }}\nleft.{} = {{ text = 'diagonal' }}\n",
                DIRECTIONS[diagonal]
            ));
            let mut sticks = StickActions::default();
            let fired = sticks.tick(&context, FRAME, scaled(VECTORS[diagonal], depth), (0, 0));
            prop_assert_eq!(fired, vec![text("diagonal")]);
        }

        #[test]
        fn a_stick_inside_the_threshold_never_fires(
            samples in prop::collection::vec((-499..=499_i16, -499..=499_i16), 1..60),
        ) {
            let context = all_eight("left", "");
            let mut sticks = StickActions::default();
            for (x, y) in samples {
                // Keep the sample inside the fire circle.
                let inside = u32::from(x.unsigned_abs()).pow(2) + u32::from(y.unsigned_abs()).pow(2)
                    < FIRE_DEFLECTION.pow(2);
                if inside {
                    prop_assert!(sticks.tick(&context, FRAME, (x, y), (0, 0)).is_empty());
                }
            }
        }

        #[test]
        fn holding_repeats_and_a_deeper_push_repeats_at_least_as_often(
            index in 0..8_usize,
            shallow in 550..=800_i16,
            extra in 0..=200_i16,
            frames in 30..200_usize,
        ) {
            let context = all_eight("left", "");
            let direction = index - index % 2;
            let shallow_count = hold(&mut StickActions::default(), &context, scaled(VECTORS[direction], shallow), frames).len();
            let deep_count = hold(&mut StickActions::default(), &context, scaled(VECTORS[direction], shallow + extra), frames).len();
            prop_assert!(shallow_count >= 2, "a held stick repeats after half a second");
            prop_assert!(deep_count >= shallow_count);
        }

        #[test]
        fn repeat_false_fires_once_per_deflection(frames in 1..200_usize, pushes in 1..5_usize) {
            let context = all_eight("left", ", repeat = false");
            let mut sticks = StickActions::default();
            let mut fired = 0;
            for _ in 0..pushes {
                fired += hold(&mut sticks, &context, (0, -900), frames).len();
                fired += hold(&mut sticks, &context, (0, 0), 1).len();
            }
            prop_assert_eq!(fired, pushes);
        }

        #[test]
        fn the_held_modifier_selects_its_layer_and_no_modifier_the_plain_one(
            modifier in prop::sample::select(vec![ButtonName::South, ButtonName::East, ButtonName::West, ButtonName::North]),
            held in any::<bool>(),
        ) {
            let name = toml::Value::try_from(modifier).unwrap().as_str().unwrap().to_owned();
            let context = context(&format!(
                "left.up = [{{ text = 'plain' }}, {{ text = 'modified', with = '{name}' }}]\n"
            ));
            let mut sticks = StickActions::default();
            if held {
                sticks.press_modifier(modifier);
            }
            let expected = if held { "modified" } else { "plain" };
            prop_assert_eq!(sticks.tick(&context, FRAME, (0, -900), (0, 0)), vec![text(expected)]);
        }
    }

    #[test]
    fn a_modifier_without_its_own_layer_entry_blocks_the_plain_entry() {
        let context =
            context("left.up = { text = 'plain' }\nleft.down = { text = 'down', with = 'north' }\n");
        let mut sticks = StickActions::default();
        sticks.press_modifier(ButtonName::North);
        assert!(
            sticks.tick(&context, FRAME, (0, -900), (0, 0)).is_empty(),
            "north's layer has no up"
        );
    }

    #[test]
    fn a_direction_change_fires_the_new_direction_at_once() {
        let context = all_eight("left", "");
        let mut sticks = StickActions::default();
        assert_eq!(sticks.tick(&context, FRAME, (0, -900), (0, 0)), vec![text("up")]);
        assert_eq!(
            sticks.tick(&context, FRAME, (900, 0), (0, 0)),
            vec![text("right")]
        );
    }

    #[test]
    fn the_right_stick_maps_independently() {
        let context = all_eight("right", "");
        let mut sticks = StickActions::default();
        assert!(
            sticks.tick(&context, FRAME, (0, -900), (0, 0)).is_empty(),
            "left is unmapped"
        );
        assert_eq!(sticks.tick(&context, FRAME, (0, 0), (0, 900)), vec![text("down")]);
    }

    fn player() -> State {
        let context =
            context("left.up = { text = 'plain' }\nleft.down = { text = 'zoom', with = 'north' }\n");
        step(
            State::with_contexts(vec![context]),
            &Event::Focus {
                app_id: Some(APP.into()),
                title: String::new(),
            },
        )
        .0
    }

    fn button(state: State, name: ButtonName, pressed: bool) -> (State, Vec<Action>) {
        step(
            state,
            &Event::Button {
                name,
                state: if pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                },
            },
        )
    }

    #[test]
    fn tapping_a_modifier_alone_fires_its_own_action_on_release() {
        let (state, pressed) = button(player(), ButtonName::North, true);
        assert!(pressed.is_empty(), "the press waits for a stick move or release");
        let (_, released) = button(state, ButtonName::North, false);
        assert_eq!(released, vec![text("north")]);
    }

    #[test]
    fn a_modifier_held_through_a_stick_move_fires_only_the_stick() {
        let (mut state, _) = button(player(), ButtonName::North, true);
        let moved = state.stick_actions(FRAME, (0, 900), (0, 0));
        assert_eq!(moved, vec![text("zoom")]);
        let (mut state, released) = button(state, ButtonName::North, false);
        assert!(released.is_empty(), "the modifier was used");
        assert_eq!(
            state.stick_actions(FRAME, (0, -900), (0, 0)),
            vec![text("plain")],
            "released, the plain layer applies again"
        );
    }

    #[test]
    fn a_button_that_modifies_nothing_fires_on_press_as_before() {
        let (_, pressed) = button(player(), ButtonName::West, true);
        assert!(
            pressed.iter().all(|action| *action != text("north")),
            "west is not a modifier"
        );
        let state = State::with_contexts(vec![
            parse_context(&format!(
                "app_id = '{APP}'\n[buttons]\nnorth = {{ text = 'north' }}\n"
            ))
            .unwrap(),
        ]);
        let state = step(
            state,
            &Event::Focus {
                app_id: Some(APP.into()),
                title: String::new(),
            },
        )
        .0;
        assert_eq!(button(state, ButtonName::North, true).1, vec![text("north")]);
    }

    #[test]
    fn the_ticker_lists_the_layer_of_the_held_modifier() {
        let contexts = vec![context(
            "left.up = { text = 'plain' }\nleft.down = { text = 'zoom', with = 'north' }\n",
        )];
        let texts = |modifier| -> Vec<String> {
            ticker::entries_with_modifier(&contexts, Some(APP), PadFamily::Letters, Level::App, modifier)
                .iter()
                .map(|entry| entry.text(|_| true))
                .collect()
        };
        let plain = texts(None);
        assert!(
            plain.iter().any(|entry| entry.starts_with("L\u{2191} ->")),
            "{plain:?}"
        );
        assert!(
            !plain.iter().any(|entry| entry.contains("L\u{2193}")),
            "{plain:?}"
        );
        let held = texts(Some(ButtonName::North));
        assert!(
            held.iter().any(|entry| entry.starts_with("Y + L\u{2193} ->")),
            "{held:?}"
        );
        assert!(!held.iter().any(|entry| entry.contains("L\u{2191}")), "{held:?}");
    }
}
