//! State queries preserve queued buttons and watchdog recovery follows routed input.

#[cfg(test)]
mod tests {
    use core::{cell::RefCell, time::Duration};
    use std::{collections::VecDeque, rc::Rc};

    use evdev::{AbsoluteAxisCode, KeyCode};
    use proptest::prelude::*;
    use tehuti::controller::{PadHandles, WatchInput, WatchVerdict, Watchdog, fingerprint, watch};
    use tehuti_protocol::ButtonName;

    #[derive(Default)]
    struct Client(Rc<RefCell<VecDeque<ButtonName>>>);

    impl Client {
        fn query_keys(&self, during: impl FnOnce()) {
            during();
            self.0.borrow_mut().clear();
        }
    }

    #[test]
    fn querying_state_preserves_a_queued_button_and_escape_chord()
    -> Result<(), Box<dyn core::error::Error>> {
        let mut handles = PadHandles::new(Client::default(), Client::default());
        let queued = Rc::clone(&handles.events().0);
        handles.probe().query_keys(|| {
            queued
                .borrow_mut()
                .extend([ButtonName::South, ButtonName::Guide, ButtonName::Start]);
        });
        let buttons = handles.events().0.borrow_mut().drain(..).collect::<Vec<_>>();
        assert_eq!(
            buttons,
            [ButtonName::South, ButtonName::Guide, ButtonName::Start]
        );
        let display = smithay::reexports::wayland_server::Display::new()?;
        let mut compositor = tehuti::state::Compositor::new(&display.handle())?;
        let mut pad = tehuti::input::SyntheticPad::default();
        for name in buttons {
            pad.push(tehuti_protocol::Event::Button {
                name,
                state: tehuti_protocol::ButtonState::Pressed,
            });
        }
        tehuti::input::route(&mut compositor, &mut pad)?;
        pad.push(tehuti_protocol::Event::Tick {
            elapsed: Duration::from_secs(3),
        });
        tehuti::input::route(&mut compositor, &mut pad)?;
        assert!(compositor.quit_requested());
        Ok(())
    }

    #[test]
    fn button_and_hat_changes_change_the_fingerprint() {
        let idle = fingerprint(&[], &[(AbsoluteAxisCode::ABS_HAT0X.0, 0)]);
        assert_ne!(
            idle,
            fingerprint(&[KeyCode::BTN_SOUTH.code()], &[(AbsoluteAxisCode::ABS_HAT0X.0, 0)])
        );
        assert_ne!(idle, fingerprint(&[], &[(AbsoluteAxisCode::ABS_HAT0X.0, 1)]));
    }

    #[test]
    fn unanswered_activity_requests_recovery() {
        let (state, _) = watch(Watchdog::default(), sample(0, 0));
        let (state, _) = watch(state, sample(1, 1));
        let (_, verdict) = watch(state, sample(1001, 1));
        assert_eq!(verdict, WatchVerdict::Reopen);
    }

    fn sample(millis: u64, fingerprint: u64) -> WatchInput {
        WatchInput::Sample {
            now: Duration::from_millis(millis),
            fingerprint,
        }
    }

    proptest! {
        #[test]
        fn routed_input_never_requests_a_regrab(
            changes in prop::collection::vec(any::<u64>(), 1..64),
        ) {
            let mut state = Watchdog::default();
            let mut now = 0;
            for fingerprint in changes {
                state = watch(state, WatchInput::Events).0;
                let (next, verdict) = watch(state, sample(now, fingerprint));
                prop_assert_eq!(verdict, WatchVerdict::Healthy);
                now = now.saturating_add(2000);
                let (next, verdict) = watch(next, sample(now, fingerprint));
                prop_assert_eq!(verdict, WatchVerdict::Healthy);
                state = next;
            }
        }

        #[test]
        fn drifting_sticks_and_triggers_do_not_change_activity(
            values in prop::array::uniform6(any::<i32>()),
        ) {
            let axes: Vec<_> = (0_u16..6).zip(values).collect();
            prop_assert_eq!(fingerprint(&[], &axes), fingerprint(&[], &[]));
        }
    }
}
