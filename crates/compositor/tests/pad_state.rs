//! State queries preserve queued buttons and watchdog recovery follows routed input.

#[cfg(test)]
mod tests {
    use core::{cell::RefCell, time::Duration};
    use std::{collections::VecDeque, io, rc::Rc};

    use evdev::{AbsoluteAxisCode, KeyCode};
    use proptest::prelude::*;
    use smithay::reexports::wayland_server::Display;
    use tehuti::controller::{
        DpadHat, EvdevPad, PadDevice, WatchInput, WatchVerdict, Watchdog, fingerprint, watch,
    };
    use tehuti::input::InputSource;
    use tehuti::state::Compositor;
    use tehuti_protocol::ButtonName;

    #[derive(Default)]
    struct Client(Rc<RefCell<VecDeque<ButtonName>>>);

    impl PadDevice for Client {
        fn get_key_state(&self) -> io::Result<Vec<u16>> {
            self.0.borrow_mut().clear();
            Ok(Vec::new())
        }

        fn get_absinfo(&self) -> io::Result<Vec<(AbsoluteAxisCode, evdev::AbsInfo)>> {
            self.0.borrow_mut().clear();
            Ok(Vec::new())
        }

        fn drain(&mut self, _hat: &mut DpadHat) -> io::Result<Vec<tehuti_protocol::Event>> {
            Ok(self
                .0
                .borrow_mut()
                .drain(..)
                .map(|name| tehuti_protocol::Event::Button {
                    name,
                    state: tehuti_protocol::ButtonState::Pressed,
                })
                .collect())
        }

        fn ungrab(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn querying_state_preserves_a_queued_button_and_escape_chord() -> Result<(), Box<dyn core::error::Error>>
    {
        let events = Client::default();
        let queued = Rc::clone(&events.0);
        let mut controller = EvdevPad::new(events, Client::default(), DpadHat::default());
        queued
            .borrow_mut()
            .extend([ButtonName::South, ButtonName::Guide, ButtonName::Start]);
        let _activity = controller.activity()?;
        let _sticks = controller.sticks()?;
        let buttons = controller.drain()?;
        assert_eq!(
            buttons,
            [ButtonName::South, ButtonName::Guide, ButtonName::Start].map(|name| {
                tehuti_protocol::Event::Button {
                    name,
                    state: tehuti_protocol::ButtonState::Pressed,
                }
            })
        );
        let display: Display<Compositor> = Display::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        let mut pad = tehuti::input::SyntheticPad::default();
        for event in buttons {
            pad.push(event);
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
            fingerprint(
                &[KeyCode::BTN_SOUTH.code()],
                &[(AbsoluteAxisCode::ABS_HAT0X.0, 0)]
            )
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
                let (next, sampled_verdict) = watch(state, sample(now, fingerprint));
                prop_assert_eq!(sampled_verdict, WatchVerdict::Healthy);
                now = now.saturating_add(2000);
                let (idle_state, idle_verdict) = watch(next, sample(now, fingerprint));
                prop_assert_eq!(idle_verdict, WatchVerdict::Healthy);
                state = idle_state;
            }
        }
    }
}
