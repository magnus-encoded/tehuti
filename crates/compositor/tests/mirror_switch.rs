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
            Ok(Vec::new())
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
    }

    fn pad(name: &'static str, log: &Rc<RefCell<Log>>) -> (EvdevPad<Synthetic>, Handles) {
        let raw = Rc::default();
        let held = Rc::default();
        let device = |queue: &Rc<_>, keys: &Rc<_>| Synthetic {
            name,
            raw: Rc::clone(queue),
            held: Rc::clone(keys),
            log: Rc::clone(log),
        };
        let pad = EvdevPad::new(device(&raw, &held), device(&raw, &held), DpadHat::default());
        (pad, Handles { raw, held })
    }

    fn press(code: KeyCode, value: i32) -> [InputEvent; 2] {
        [
            InputEvent::new(EventType::KEY.0, code.code(), value),
            InputEvent::new(EventType::SYNCHRONIZATION.0, SynchronizationCode::SYN_REPORT.0, 0),
        ]
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
        );
        assert!(first_sync.started && first_sync.failed.is_empty());
        first_handles
            .raw
            .borrow_mut()
            .extend(press(KeyCode::BTN_SOUTH, 1));
        first.pump_mirror(&mut |_, _| false).unwrap();

        // The second pad's press reached the core before its mirror existed; its kernel state still holds it.
        second_handles.held.borrow_mut().push(KeyCode::BTN_EAST.code());
        let second_sync = sync_mirrors(
            &mut [(first_path, &mut first), (second_path, &mut second)],
            true,
            Some(second_path),
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
}
