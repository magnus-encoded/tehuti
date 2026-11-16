//! A controller button press and release reach the session log at debug level, and not at info.

#[cfg(test)]
mod button_log {
    use smithay::reexports::wayland_server::Display;
    use std::sync::{Arc, Mutex};
    use tehuti::state::Compositor;
    use tracing::Level;
    use tehuti_protocol::{ButtonName, ButtonState, Event};

    #[derive(Debug, Clone, Default)]
    struct Logs(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Logs {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::Error::other("log lock poisoned"))?
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn press_and_release(level: Level) -> Vec<String> {
        let logs = Logs::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(level)
            .with_writer(move || writer.clone())
            .finish();
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tracing::subscriber::with_default(subscriber, || {
            for state in [ButtonState::Pressed, ButtonState::Released] {
                compositor.handle(&Event::Button {
                    name: ButtonName::South,
                    state,
                });
            }
        });
        String::from_utf8_lossy(&logs.0.lock().unwrap())
            .lines()
            .filter(|line| line.contains("controller button"))
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn debug_logs_one_press_and_one_release_naming_the_button_and_who_took_it() {
        let lines = press_and_release(Level::DEBUG);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("Pressed") && lines[1].contains("Released"), "{lines:?}");
        assert!(lines.iter().all(|line| line.contains("South")), "{lines:?}");
        assert!(lines.iter().all(|line| line.contains("taken_by=")), "{lines:?}");
        assert!(lines.iter().all(|line| line.contains("controller=")), "{lines:?}");
    }

    #[test]
    fn info_logs_no_button_lines() {
        assert!(press_and_release(Level::INFO).is_empty());
    }
}
