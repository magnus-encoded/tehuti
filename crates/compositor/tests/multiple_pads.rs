//! Connected pads share event routing and retain independent lifetimes.

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, io, path::PathBuf};

    use proptest::prelude::*;
    use tehuti::input::{InputSource, SyntheticPad, drain_pads};
    use tehuti_protocol::{ButtonName, ButtonState, Event};

    struct Pad {
        source: SyntheticPad,
        disconnected: bool,
    }

    impl InputSource for Pad {
        fn drain(&mut self) -> io::Result<Vec<Event>> {
            if self.disconnected {
                Err(io::ErrorKind::NotConnected.into())
            } else {
                self.source.drain()
            }
        }

        fn release(&mut self) -> io::Result<()> {
            self.source.release()
        }
    }

    const fn button(name: ButtonName, pressed: bool) -> Event {
        Event::Button {
            name,
            state: if pressed {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            },
        }
    }

    fn pad(events: impl IntoIterator<Item = Event>) -> Pad {
        let mut source = SyntheticPad::default();
        for event in events {
            source.push(event);
        }
        Pad {
            source,
            disconnected: false,
        }
    }

    fn drain_gated(
        pads: &mut BTreeMap<PathBuf, Pad>,
        gate: &mut tehuti::input::PadGate,
        chosen: &PathBuf,
    ) -> Vec<Event> {
        tehuti::input::drain_pads_admitted(pads, |path, _, event| gate.admit(path, path == chosen, event)).0
    }

    proptest! {
        #[test]
        fn every_pad_delivers_buttons_and_losing_one_preserves_the_others(
            first in prop::collection::vec(any::<bool>(), 1..20),
            second in prop::collection::vec(any::<bool>(), 1..20),
            lost_first in any::<bool>(),
        ) {
            let first_events: Vec<_> = first.into_iter().map(|pressed| button(ButtonName::South, pressed)).collect();
            let second_events: Vec<_> = second.into_iter().map(|pressed| button(ButtonName::East, pressed)).collect();
            let paths = [PathBuf::from("event0"), PathBuf::from("event1")];
            let mut pads = BTreeMap::from([
                (paths[0].clone(), pad(first_events.clone())),
                (paths[1].clone(), pad(second_events.clone())),
            ]);
            let (events, removed) = drain_pads(&mut pads);
            prop_assert_eq!(events, [first_events, second_events].concat());
            prop_assert!(removed.is_empty());
            let lost = usize::from(!lost_first);
            let survivor = usize::from(lost_first);
            pads.get_mut(&paths[lost]).expect("connected pad").disconnected = true;
            let next = button(ButtonName::Start, true);
            pads.get_mut(&paths[survivor]).expect("connected pad").source.push(next.clone());
            let (surviving_events, disconnected) = drain_pads(&mut pads);
            prop_assert_eq!(surviving_events, [next.clone()]);
            prop_assert_eq!(disconnected, [paths[lost].clone()]);
            prop_assert_eq!(pads.len(), 1);
            pads.get_mut(&paths[survivor]).expect("survivor retained").source.push(next.clone());
            prop_assert_eq!(drain_pads(&mut pads).0, [next]);
            let _previous = pads.insert(paths[lost].clone(), pad([button(ButtonName::Guide, true)]));
            prop_assert_eq!(drain_pads(&mut pads).0, [button(ButtonName::Guide, true)]);
            prop_assert_eq!(pads.len(), 2);
        }
    }

    #[test]
    fn a_chosen_pad_drives_alone_while_connected() {
        use tehuti::input::chosen;
        use tehuti_core::dispatch::ControllerChoice;
        let connected = ["Wireless Controller".to_owned(), "Xbox Controller".to_owned()];
        let wireless = ControllerChoice::Named("Wireless Controller".into());
        assert!(chosen(&ControllerChoice::Auto, &connected, "Xbox Controller"));
        assert!(chosen(&wireless, &connected, "Wireless Controller"));
        assert!(!chosen(&wireless, &connected, "Xbox Controller"));
        assert!(
            chosen(&wireless, &connected[1..], "Xbox Controller"),
            "a missing chosen pad never locks every pad out"
        );
    }

    #[test]
    fn the_gate_lets_go_of_buttons_it_let_through_and_drops_the_rest() {
        use tehuti::input::PadGate;
        let paths = [PathBuf::from("event0"), PathBuf::from("event1")];
        let mut pads = BTreeMap::from([
            (paths[0].clone(), pad([button(ButtonName::DpadDown, true)])),
            (paths[1].clone(), pad([button(ButtonName::South, true)])),
        ]);
        let mut gate = PadGate::default();
        let mut chosen_path = paths[0].clone();
        assert_eq!(
            drain_gated(&mut pads, &mut gate, &chosen_path),
            [button(ButtonName::DpadDown, true)]
        );
        chosen_path.clone_from(&paths[1]);
        for (path, event) in [
            (&paths[0], button(ButtonName::DpadDown, false)),
            (&paths[0], button(ButtonName::East, true)),
            (&paths[0], button(ButtonName::East, false)),
            (&paths[1], button(ButtonName::South, false)),
        ] {
            pads.get_mut(path).expect("connected pad").source.push(event);
        }
        assert_eq!(
            drain_gated(&mut pads, &mut gate, &chosen_path),
            [
                button(ButtonName::DpadDown, false),
                button(ButtonName::South, false)
            ],
            "the old pad lets go of its held button but presses nothing new"
        );
    }
}
