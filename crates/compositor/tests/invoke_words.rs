//! A plugin's invoke words reach the core with the focused window and leave with its plugin.

#[cfg(test)]
mod invoke_words {
    use core::time::Duration;
    use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
    use std::{os::unix::fs::PermissionsExt as _, time::Instant};
    use tehuti::{plugins::install, state::Compositor};
    use tehuti_protocol::Event;

    const PLUGIN: &str = r"#!/usr/bin/env python3
import json, select, sys, os
root = os.path.dirname(os.path.abspath(__file__))
json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc': '2.0', 'id': 0, 'result': {'protocol_version': 0, 'encoding': 'json', 'events': ['focus']}}), flush=True)
print(json.dumps({'jsonrpc': '2.0', 'method': 'invoke.words', 'params': {'words': ['changelog']}}), flush=True)
while True:
    if select.select([sys.stdin], [], [], 0.02)[0] and not sys.stdin.readline():
        break
    if os.path.exists(os.path.join(root, 'die')):
        os._exit(1)
";

    struct Harness {
        compositor: Compositor,
        event_loop: EventLoop<'static, Compositor>,
        directory: tempfile::TempDir,
        _display: Display<Compositor>,
    }

    impl Harness {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let executable = directory.path().join("plugin.py");
            std::fs::write(&executable, PLUGIN).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::write(
                directory.path().join("plugin.json"),
                serde_json::to_vec(&serde_json::json!({
                    "executable":"plugin.py", "app_ids":["chatty"], "events":["focus"],
                    "start":"always", "permissions":[]
                }))
                .unwrap(),
            )
            .unwrap();
            let display: Display<Compositor> = Display::new().unwrap();
            let mut compositor = Compositor::new(&display.handle()).unwrap();
            let event_loop = EventLoop::try_new().unwrap();
            compositor.set_plugins(install(&event_loop, directory.path(), |state| state).unwrap());
            Self {
                compositor,
                event_loop,
                directory,
                _display: display,
            }
        }

        fn focus(&mut self, app_id: Option<&str>) {
            self.compositor.handle(&Event::Focus {
                app_id: app_id.map(Into::into),
                title: String::new(),
            });
        }

        fn await_words(&mut self, expected: &[&str]) {
            let deadline = Instant::now().checked_add(Duration::from_secs(5)).unwrap();
            while self.compositor.plugin_invoke_words() != expected {
                assert!(
                    Instant::now() < deadline,
                    "core holds {:?}, expected {expected:?}",
                    self.compositor.plugin_invoke_words()
                );
                self.event_loop
                    .dispatch(Duration::from_millis(5), &mut self.compositor)
                    .unwrap();
                self.compositor.handle(&Event::Tick {
                    elapsed: Duration::from_millis(5),
                });
            }
        }
    }

    #[test]
    fn words_apply_while_a_covered_window_is_focused() {
        let mut harness = Harness::new();
        harness.focus(Some("chatty"));
        harness.await_words(&["changelog"]);
    }

    #[test]
    fn focus_moving_to_an_uncovered_app_or_none_empties_them() {
        let mut harness = Harness::new();
        harness.focus(Some("chatty"));
        harness.await_words(&["changelog"]);
        harness.focus(Some("elsewhere"));
        harness.await_words(&[]);
        harness.focus(Some("chatty"));
        harness.await_words(&["changelog"]);
        harness.focus(None);
        harness.await_words(&[]);
    }

    #[test]
    fn a_plugin_exit_clears_them_without_a_focus_change() {
        let mut harness = Harness::new();
        harness.focus(Some("chatty"));
        harness.await_words(&["changelog"]);
        std::fs::write(harness.directory.path().join("die"), "").unwrap();
        harness.await_words(&[]);
    }
}
