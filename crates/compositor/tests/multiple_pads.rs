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
}
