//! Switching the active pad moves the one virtual pad without leaving buttons held.

#[cfg(test)]
mod tests {
    use core::cell::RefCell;
    use std::{collections::VecDeque, io, path::Path, rc::Rc};

    use evdev::{AbsoluteAxisCode, EventType, InputEvent, KeyCode, SynchronizationCode};
    use proptest::prelude::*;
    use tehuti::controller::{DpadHat, EvdevPad, MirrorSink, PadDevice, sync_mirrors};

    /// One frame as (type, code, value) triples, tagged with the pad that sent it.
    type Sent = (&'static str, Vec<(u16, u16, i32)>);

    /// Every frame each virtual pad sent, and how many existed at once.
    #[derive(Debug, Default)]
    struct Log {
        frames: Vec<Sent>,
        live: usize,
        most_live: usize,
        created: usize,
        refuse: bool,
        /// The physical pad is unplugged: reads fail with ENODEV.
        gone: bool,
        grabs: usize,
        ungrabs: usize,
    }

    struct Sink(&'static str, Rc<RefCell<Log>>);

    impl MirrorSink for Sink {
        fn emit(&mut self, frame: &[InputEvent]) -> io::Result<()> {
            let frame = frame
                .iter()
                .map(|event| (event.event_type().0, event.code(), event.value()))
                .collect();
            self.1.borrow_mut().frames.push((self.0, frame));
            Ok(())
        }
    }

    impl Drop for Sink {
        fn drop(&mut self) {
            let mut log = self.1.borrow_mut();
            log.live = log.live.saturating_sub(1);
        }
    }

    /// A synthetic pad: queued raw events and the keys its kernel state reports held.
    struct Synthetic {
        name: &'static str,
        raw: Rc<RefCell<VecDeque<InputEvent>>>,
        held: Rc<RefCell<Vec<u16>>>,
        hat: Rc<core::cell::Cell<i32>>,
        log: Rc<RefCell<Log>>,
    }

    impl PadDevice for Synthetic {
        fn grab(&mut self) -> io::Result<()> {
            let mut log = self.log.borrow_mut();
            log.grabs = log.grabs.saturating_add(1);
            Ok(())
        }
        fn get_key_state(&self) -> io::Result<Vec<u16>> {
            Ok(self.held.borrow().clone())
        }
        fn get_absinfo(&self) -> io::Result<Vec<(AbsoluteAxisCode, evdev::AbsInfo)>> {
            Ok(vec![
                (
                    AbsoluteAxisCode::ABS_HAT0X,
                    evdev::AbsInfo::new(self.hat.get(), -1, 1, 0, 0, 0),
                ),
                (
                    AbsoluteAxisCode::ABS_X,
                    evdev::AbsInfo::new(0, -100, 100, 0, 0, 0),
                ),
                (AbsoluteAxisCode::ABS_Z, evdev::AbsInfo::new(0, 0, 255, 0, 0, 0)),
            ])
        }
        fn drain(&mut self, _hat: &mut DpadHat) -> io::Result<Vec<tehuti_protocol::Event>> {
            Ok(Vec::new())
        }
        fn ungrab(&mut self) -> io::Result<()> {
            let mut log = self.log.borrow_mut();
            log.ungrabs = log.ungrabs.saturating_add(1);
            Ok(())
        }
        fn virtual_pad(&self) -> io::Result<Box<dyn MirrorSink>> {
            let mut log = self.log.borrow_mut();
            log.created = log.created.saturating_add(1);
            if log.refuse {
                return Err(io::Error::from_raw_os_error(22));
            }
            log.live = log.live.saturating_add(1);
            log.most_live = log.most_live.max(log.live);
            Ok(Box::new(Sink(self.name, Rc::clone(&self.log))))
        }
        fn fetch_raw(&mut self) -> io::Result<Vec<InputEvent>> {
            if self.log.borrow().gone {
                return Err(io::Error::from_raw_os_error(19));
            }
            Ok(self.raw.borrow_mut().drain(..).collect())
        }
    }

    struct Handles {
        raw: Rc<RefCell<VecDeque<InputEvent>>>,
        held: Rc<RefCell<Vec<u16>>>,
        hat: Rc<core::cell::Cell<i32>>,
    }

    fn pad(name: &'static str, log: &Rc<RefCell<Log>>) -> (EvdevPad<Synthetic>, Handles) {
        let raw = Rc::default();
        let held = Rc::default();
        let hat = Rc::default();
        let device = |queue: &Rc<_>, keys: &Rc<_>| Synthetic {
            name,
            raw: Rc::clone(queue),
            held: Rc::clone(keys),
            hat: Rc::clone(&hat),
            log: Rc::clone(log),
        };
        let pad = EvdevPad::new(device(&raw, &held), device(&raw, &held), DpadHat::default());
        (pad, Handles { raw, held, hat })
    }

    fn queue(handles: &Handles, code: KeyCode, value: i32) {
        handles.raw.borrow_mut().extend(press(code, value));
    }

    fn press(code: KeyCode, value: i32) -> [InputEvent; 2] {
        [
            InputEvent::new(EventType::KEY.0, code.code(), value),
            InputEvent::new(EventType::SYNCHRONIZATION.0, SynchronizationCode::SYN_REPORT.0, 0),
        ]
    }

    #[test]
    fn creation_failure_falls_back_once_per_source_pad() {
        let log = Rc::new(RefCell::new(Log {
            refuse: true,
            ..Log::default()
        }));
        let (mut first, _) = pad("first", &log);
        let (mut second, _) = pad("second", &log);
        let paths = [Path::new("event1"), Path::new("event2")];
        let mut failures = Vec::new();
        for active in [Path::new("event1"), Path::new("event2"), Path::new("event1")] {
            for mirrored in [true, true, false, true] {
                let result = sync_mirrors(
                    &mut [
                        (Path::new("event1"), &mut first),
                        (Path::new("event2"), &mut second),
                    ],
                    mirrored,
                    Some(active),
                    &mut None,
                    &|_| false,
                );
                for path in &result.failed {
                    if path == Path::new("event1") {
                        first.set_passthrough(true).unwrap();
                    } else {
                        second.set_passthrough(true).unwrap();
                    }
                }
                failures.extend(result.failed);
            }
        }
        assert_eq!(failures, paths.map(Path::to_path_buf));
        assert_eq!(log.borrow().created, 2);
        for pad in [&mut first, &mut second] {
            pad.set_passthrough(true).unwrap();
            for _ in 0..100 {
                pad.set_passthrough(true).unwrap();
            }
        }
        assert_eq!(log.borrow().grabs, 0);
        assert_eq!(log.borrow().ungrabs, 2);
        assert!(!first.mirrored() && !second.mirrored());
        for _ in 0..100 {
            let again = sync_mirrors(
                &mut [(paths[0], &mut first)],
                true,
                Some(paths[0]),
                &mut None,
                &|_| false,
            );
            assert!(again.failed.is_empty() && !again.started);
            first.pump_mirror(&mut |_, _| false).unwrap();
        }
        assert_eq!(log.borrow().created, 2, "a refused virtual pad is never retried");
    }

    /// Every key the application sees is up and every stick and trigger rests.
    fn neutral(log: &Log) -> bool {
        shown(log).iter().all(|(_, value)| *value == 0)
    }

    proptest! {
        /// Laws 1 to 3: one virtual pad for the session, neutral while no pad is present,
        /// and input flows again after each reconnect without a focus change.
        #[test]
        fn the_virtual_pad_outlives_any_number_of_read_failures(
            drops in 1_usize..6,
            reads_while_gone in 1_usize..20,
        ) {
            let log = Rc::new(RefCell::new(Log::default()));
            let mut spare = None;
            for round in 0..=drops {
                let path = Path::new("/dev/input/event1");
                let (mut current, handles) = pad("pad", &log);
                let sync = sync_mirrors(&mut [(path, &mut current)], true, Some(path), &mut spare, &|_| false);
                prop_assert!(sync.failed.is_empty() && current.mirrored() && spare.is_none());
                queue(&handles, KeyCode::BTN_EAST, 1);
                current.pump_mirror(&mut |_, _| false).unwrap();
                let east = shown(&log.borrow()).get(&(EventType::KEY.0, KeyCode::BTN_EAST.code())).copied();
                prop_assert_eq!(
                    east,
                    Some(1),
                    "input flows after reconnect {}", round
                );
                if round == drops {
                    prop_assert_eq!(log.borrow().live, 1, "one virtual pad is live");
                    break;
                }
                queue(&handles, KeyCode::BTN_SOUTH, 1);
                handles.raw.borrow_mut().extend(axis(AbsoluteAxisCode::ABS_X, 80));
                current.pump_mirror(&mut |_, _| false).unwrap();
                log.borrow_mut().gone = true;
                prop_assert!(current.pump_mirror(&mut |_, _| false).is_err());
                for _ in 0..reads_while_gone {
                    prop_assert!(current.pump_mirror(&mut |_, _| false).is_ok(), "the drop is reported once");
                }
                prop_assert!(!current.mirrored());
                prop_assert!(neutral(&log.borrow()), "{:?}", shown(&log.borrow()));
                spare = current.detach_virtual_pad();
                prop_assert!(spare.is_some());
                prop_assert!(neutral(&log.borrow()));
                log.borrow_mut().gone = false;
            }
            let log = log.borrow();
            prop_assert_eq!(log.created, 1, "the virtual pad is made once");
            prop_assert_eq!(log.most_live, 1, "never two at once");
        }
    }

    #[test]
    fn a_dropped_pad_releases_the_plugin_buttons_it_held() {
        let log = Rc::new(RefCell::new(Log::default()));
        let path = Path::new("/dev/input/event1");
        let (mut only, handles) = pad("pad", &log);
        let consumed = |button: tehuti_protocol::ButtonName| button == tehuti_protocol::ButtonName::Start;
        let _sync = sync_mirrors(&mut [(path, &mut only)], true, Some(path), &mut None, &consumed);
        let seen = RefCell::new(Vec::new());
        let mut step = |button, state| {
            seen.borrow_mut().push((button, state));
            consumed(button)
        };
        queue(&handles, KeyCode::BTN_START, 1);
        only.pump_mirror(&mut step).unwrap();
        log.borrow_mut().gone = true;
        assert!(only.pump_mirror(&mut step).is_err());
        assert!(only.pump_mirror(&mut step).is_ok());
        assert_eq!(
            seen.borrow().last(),
            Some(&(
                tehuti_protocol::ButtonName::Start,
                tehuti_protocol::ButtonState::Released
            ))
        );
        assert!(never_sent(&log.borrow(), KeyCode::BTN_START));
    }

    #[test]
    fn focus_leaving_and_returning_keeps_the_same_virtual_pad() {
        let log = Rc::new(RefCell::new(Log::default()));
        let (mut only, handles) = pad("only", &log);
        let path = Path::new("/dev/input/event1");
        let mut sync = |mirrored| {
            sync_mirrors(&mut [(path, &mut only)], mirrored, Some(path), &mut None, &|_| {
                false
            })
        };

        assert!(sync(true).started);
        assert!(!sync(false).started);
        assert_eq!(sync(true).failed, Vec::<std::path::PathBuf>::new());

        let log = log.borrow();
        assert_eq!(log.created, 1, "the application never sees the pad unplugged");
        assert_eq!(log.live, 1);
        drop(handles);
    }

    #[test]
    fn switching_releases_the_old_mirror_and_seeds_the_new_one() {
        let log = Rc::new(RefCell::new(Log::default()));
        let (mut first, first_handles) = pad("first", &log);
        let (mut second, second_handles) = pad("second", &log);
        let (first_path, second_path) = (Path::new("/dev/input/event1"), Path::new("/dev/input/event2"));

        let first_sync = sync_mirrors(
            &mut [(first_path, &mut first), (second_path, &mut second)],
            true,
            Some(first_path),
            &mut None,
            &|_| false,
        );
        assert!(first_sync.started && first_sync.failed.is_empty());
        queue(&first_handles, KeyCode::BTN_SOUTH, 1);
        first.pump_mirror(&mut |_, _| false).unwrap();

        // The second pad's press reached the core before its mirror existed; its kernel state still holds it.
        second_handles.held.borrow_mut().push(KeyCode::BTN_EAST.code());
        let second_sync = sync_mirrors(
            &mut [(first_path, &mut first), (second_path, &mut second)],
            true,
            Some(second_path),
            &mut None,
            &|_| false,
        );
        assert!(second_sync.started && second_sync.failed.is_empty());

        let log = log.borrow();
        assert_eq!(log.most_live, 1, "the application never sees two virtual pads");
        assert!(!first.mirrored() && second.mirrored());
        let key = |code: KeyCode, value| (EventType::KEY.0, code.code(), value);
        let sent = |name| -> Vec<_> {
            log.frames
                .iter()
                .filter(|(from, _)| *from == name)
                .map(|(_, frame)| frame.clone())
                .collect()
        };
        assert!(
            sent("first")
                .last()
                .is_some_and(|frame| frame.contains(&key(KeyCode::BTN_SOUTH, 0))),
            "the old mirror lets go of the held button: {:?}",
            log.frames
        );
        assert!(
            sent("second")
                .first()
                .is_some_and(|frame| frame.contains(&key(KeyCode::BTN_EAST, 1))),
            "the new mirror starts out holding what its pad holds: {:?}",
            log.frames
        );
    }

    #[test]
    fn a_switch_keeps_a_plugin_button_from_the_game_and_its_release_reaches_the_plugin() {
        use tehuti_core::vpad::Verdict;
        use tehuti_protocol::{ButtonName, ButtonState};

        let log = Rc::new(RefCell::new(Log::default()));
        let (mut first, first_handles) = pad("first", &log);
        let (mut second, second_handles) = pad("second", &log);
        let (first_path, second_path) = (Path::new("/dev/input/event1"), Path::new("/dev/input/event2"));
        let filter = RefCell::new(start_override());
        // What the plugin receives: one entry per input the filter hands it.
        let plugin = RefCell::new(Vec::new());
        let mut step = |button: ButtonName, state: ButtonState| {
            let verdict = filter.borrow_mut().step(button, state, &[0], &[]);
            if verdict == Verdict::Plugin(0) {
                plugin.borrow_mut().push((button, state));
            }
            verdict != Verdict::Pass
        };
        let consumed = |button: ButtonName| filter.borrow().consumed(button);

        let _started = sync_mirrors(
            &mut [(first_path, &mut first), (second_path, &mut second)],
            true,
            Some(first_path),
            &mut None,
            &consumed,
        );
        queue(&first_handles, KeyCode::BTN_START, 1);
        first.pump_mirror(&mut step).unwrap();
        let before = filter.borrow().clone();

        second_handles.held.borrow_mut().push(KeyCode::BTN_START.code());
        let switched = sync_mirrors(
            &mut [(first_path, &mut first), (second_path, &mut second)],
            true,
            Some(second_path),
            &mut None,
            &consumed,
        );
        assert!(switched.started && second.mirrored());
        assert_eq!(
            *filter.borrow(),
            before,
            "seeding the new mirror only reads the filter"
        );
        assert_eq!(
            *plugin.borrow(),
            [(ButtonName::Start, ButtonState::Pressed)],
            "no extra press reaches the plugin"
        );

        queue(&second_handles, KeyCode::BTN_START, 0);
        second.pump_mirror(&mut step).unwrap();
        assert_eq!(
            plugin.borrow().last(),
            Some(&(ButtonName::Start, ButtonState::Released)),
            "the release still reaches the plugin"
        );
        assert!(
            never_sent(&log.borrow(), KeyCode::BTN_START),
            "the game never sees the plugin's button: {:?}",
            log.borrow().frames
        );
    }

    #[test]
    fn releasing_a_plugin_button_on_the_old_pad_reaches_the_plugin_once() {
        use tehuti_core::vpad::Verdict;
        use tehuti_protocol::{ButtonName, ButtonState};

        let log = Rc::new(RefCell::new(Log::default()));
        let (mut first, first_handles) = pad("first", &log);
        let (mut second, second_handles) = pad("second", &log);
        let (first_path, second_path) = (Path::new("event1"), Path::new("event2"));
        let mut filter = start_override();
        let mut plugin = Vec::new();
        let _started = sync_mirrors(
            &mut [(first_path, &mut first), (second_path, &mut second)],
            true,
            Some(first_path),
            &mut None,
            &|button| filter.consumed(button),
        );
        queue(&first_handles, KeyCode::BTN_START, 1);
        first
            .pump_mirror(&mut |button, state| {
                let verdict = filter.step(button, state, &[0], &[]);
                if verdict == Verdict::Plugin(0) {
                    plugin.push((button, state));
                }
                verdict != Verdict::Pass
            })
            .unwrap();
        second_handles.held.borrow_mut().push(KeyCode::BTN_EAST.code());
        let _switched = sync_mirrors(
            &mut [(first_path, &mut first), (second_path, &mut second)],
            true,
            Some(second_path),
            &mut None,
            &|button| filter.consumed(button),
        );
        assert!(!first.mirrored() && second.mirrored());
        queue(&first_handles, KeyCode::BTN_START, 0);
        for _ in 0..2 {
            first
                .pump_mirror(&mut |button, state| {
                    let verdict = filter.step(button, state, &[], &[]);
                    if verdict == Verdict::Plugin(0) {
                        plugin.push((button, state));
                    }
                    verdict != Verdict::Pass
                })
                .unwrap();
        }
        assert_eq!(
            plugin,
            [
                (ButtonName::Start, ButtonState::Pressed),
                (ButtonName::Start, ButtonState::Released),
            ]
        );
        assert!(!filter.consumed(ButtonName::Start));
        filter.clear(0);
        assert_eq!(
            filter.step(ButtonName::Start, ButtonState::Pressed, &[], &[]),
            Verdict::Pass
        );
        assert!(never_sent(&log.borrow(), KeyCode::BTN_START));
    }

    /// A filter where plugin 0 takes Start.
    fn start_override() -> tehuti_core::vpad::Filter {
        let mut filter = tehuti_core::vpad::Filter::default();
        filter.set(
            0,
            vec![tehuti_protocol::PadOverride {
                button: tehuti_protocol::ButtonName::Start,
                held: Vec::new(),
            }],
        );
        filter
    }

    fn never_sent(log: &Log, key: KeyCode) -> bool {
        log.frames
            .iter()
            .all(|(_, frame)| frame.iter().all(|(_, code, _)| *code != key.code()))
    }

    #[test]
    fn a_held_dpad_is_seeded_and_released() {
        let log = Rc::new(RefCell::new(Log::default()));
        let (mut held, handles) = pad("dpad", &log);
        let path = Path::new("/dev/input/event1");
        handles.hat.set(-1);

        let sync = sync_mirrors(&mut [(path, &mut held)], true, Some(path), &mut None, &|_| false);
        assert!(sync.started);
        let _gone = sync_mirrors(&mut [(path, &mut held)], true, None, &mut None, &|_| false);

        let hat = (EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_HAT0X.0);
        let values: Vec<i32> = log
            .borrow()
            .frames
            .iter()
            .flat_map(|(_, frame)| frame.clone())
            .filter(|(kind, code, _)| (*kind, *code) == hat)
            .map(|(_, _, value)| value)
            .collect();
        assert_eq!(
            values,
            [-1, 0],
            "the virtual pad shows the held D-pad, then lets go"
        );
    }

    fn axis(code: AbsoluteAxisCode, value: i32) -> [InputEvent; 2] {
        [
            InputEvent::new(EventType::ABSOLUTE.0, code.0, value),
            InputEvent::new(EventType::SYNCHRONIZATION.0, SynchronizationCode::SYN_REPORT.0, 0),
        ]
    }

    /// What the application sees on the virtual pad after every frame so far.
    fn shown(log: &Log) -> std::collections::BTreeMap<(u16, u16), i32> {
        let mut state = std::collections::BTreeMap::new();
        for (_, frame) in &log.frames {
            for (kind, code, value) in frame {
                if *kind != EventType::SYNCHRONIZATION.0 {
                    let _previous = state.insert((*kind, *code), *value);
                }
            }
        }
        state
    }

    /// One pad mirrored with South held and the stick and trigger pushed, then unplugged.
    fn unplugged(log: &Rc<RefCell<Log>>) -> Option<tehuti::controller::VirtualPad> {
        let path = Path::new("/dev/input/event1");
        let (mut first, handles) = pad("first", log);
        let started = sync_mirrors(&mut [(path, &mut first)], true, Some(path), &mut None, &|_| false);
        assert!(started.started);
        queue(&handles, KeyCode::BTN_SOUTH, 1);
        handles.raw.borrow_mut().extend(axis(AbsoluteAxisCode::ABS_X, 80));
        handles
            .raw
            .borrow_mut()
            .extend(axis(AbsoluteAxisCode::ABS_Z, 200));
        first.pump_mirror(&mut |_, _| false).unwrap();
        first.detach_virtual_pad()
    }

    #[test]
    fn unplug_releases_buttons_centres_sticks_and_rests_triggers() {
        let log = Rc::new(RefCell::new(Log::default()));
        let spare = unplugged(&log);
        assert!(spare.is_some());
        let log = log.borrow();
        assert_eq!(log.live, 1, "the virtual pad outlives the unplugged pad");
        assert_eq!(
            log.frames.last(),
            Some(&(
                "first",
                vec![
                    (EventType::KEY.0, KeyCode::BTN_SOUTH.code(), 0),
                    (EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, 0),
                    (EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Z.0, 0),
                    (EventType::SYNCHRONIZATION.0, SynchronizationCode::SYN_REPORT.0, 0),
                ]
            )),
            "{:?}",
            log.frames
        );
    }

    #[test]
    fn a_reconnected_pad_takes_over_the_same_virtual_pad() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut spare = unplugged(&log);
        let path = Path::new("/dev/input/event7");
        let (mut second, _handles) = pad("second", &log);
        let sync = sync_mirrors(&mut [(path, &mut second)], true, Some(path), &mut spare, &|_| {
            false
        });
        assert!(sync.failed.is_empty() && second.mirrored() && spare.is_none());
        let log = log.borrow();
        assert_eq!(log.created, 1, "no second virtual pad is made");
        assert_eq!((log.live, log.most_live), (1, 1));
    }

    #[test]
    fn input_from_the_reconnected_pad_reaches_the_original_virtual_pad() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut spare = unplugged(&log);
        let path = Path::new("/dev/input/event7");
        let (mut second, handles) = pad("second", &log);
        let _sync = sync_mirrors(&mut [(path, &mut second)], true, Some(path), &mut spare, &|_| {
            false
        });
        queue(&handles, KeyCode::BTN_EAST, 1);
        second.pump_mirror(&mut |_, _| false).unwrap();
        assert_eq!(
            log.borrow().frames.last(),
            Some(&(
                "first",
                // `VirtualDevice::emit` appends the SYN_REPORT itself.
                vec![(EventType::KEY.0, KeyCode::BTN_EAST.code(), 1)]
            ))
        );
    }

    #[test]
    fn switching_mirroring_off_drops_the_spare_virtual_pad() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut spare = unplugged(&log);
        let path = Path::new("/dev/input/event7");
        let (mut second, _handles) = pad("second", &log);
        let _sync = sync_mirrors(&mut [(path, &mut second)], false, Some(path), &mut spare, &|_| {
            false
        });
        assert!(spare.is_none() && !second.mirrored());
        assert_eq!(log.borrow().live, 0, "the application sees the pad unplugged");
    }

    proptest! {
        #[test]
        fn unplugging_leaves_nothing_held(
            series in proptest::collection::vec((0_usize..4, -100_i32..=100), 0..24),
            parked in proptest::bool::ANY,
        ) {
            let codes = [KeyCode::BTN_SOUTH, KeyCode::BTN_EAST, KeyCode::BTN_TL];
            let log = Rc::new(RefCell::new(Log::default()));
            let path = Path::new("/dev/input/event1");
            let (mut only, handles) = pad("only", &log);
            let _started = sync_mirrors(&mut [(path, &mut only)], true, Some(path), &mut None, &|_| false);
            for (input, value) in series {
                let events = codes
                    .get(input)
                    .map_or_else(|| axis(AbsoluteAxisCode::ABS_X, value), |code| press(*code, i32::from(value > 0)));
                handles.raw.borrow_mut().extend(events);
            }
            only.pump_mirror(&mut |_, _| false).unwrap();
            if parked {
                let _left = sync_mirrors(&mut [(path, &mut only)], false, Some(path), &mut None, &|_| false);
            }
            prop_assert!(only.detach_virtual_pad().is_some());
            let log = log.borrow();
            let state = shown(&log);
            prop_assert!(state.values().all(|value| *value == 0), "{state:?}");
            prop_assert_eq!(log.created, 1);
        }
    }
}
