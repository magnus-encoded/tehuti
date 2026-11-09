//! Switching the active pad moves the one virtual pad without leaving buttons held.

#[cfg(test)]
mod tests {
    use core::cell::RefCell;
    use std::{collections::VecDeque, io, path::Path, rc::Rc};

    use evdev::{AbsoluteAxisCode, EventType, InputEvent, KeyCode, SynchronizationCode};
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
            Ok(())
        }
        fn get_key_state(&self) -> io::Result<Vec<u16>> {
            Ok(self.held.borrow().clone())
        }
        fn get_absinfo(&self) -> io::Result<Vec<(AbsoluteAxisCode, evdev::AbsInfo)>> {
            Ok(vec![(
                AbsoluteAxisCode::ABS_HAT0X,
                evdev::AbsInfo::new(self.hat.get(), -1, 1, 0, 0, 0),
            )])
        }
        fn drain(&mut self, _hat: &mut DpadHat) -> io::Result<Vec<tehuti_protocol::Event>> {
            Ok(Vec::new())
        }
        fn ungrab(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn virtual_pad(&self) -> io::Result<Box<dyn MirrorSink>> {
            let mut log = self.log.borrow_mut();
            log.live = log.live.saturating_add(1);
            log.created = log.created.saturating_add(1);
            log.most_live = log.most_live.max(log.live);
            Ok(Box::new(Sink(self.name, Rc::clone(&self.log))))
        }
        fn fetch_raw(&mut self) -> io::Result<Vec<InputEvent>> {
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
}
