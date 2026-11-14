//! Speech hotwords from contexts and plugins reach the sidecar with the focused window.

#[cfg(test)]
mod speech_hints {
    use core::time::Duration;
    use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
    use std::{os::unix::fs::PermissionsExt as _, path::Path, time::Instant};
    use tehuti::{plugins::install, speech::SpeechProcess, state::Compositor};
    use tehuti_protocol::Event;

    const PLUGIN: &str = r"#!/usr/bin/env python3
import json, select, sys, os
root = os.path.dirname(os.path.abspath(__file__))
json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc': '2.0', 'id': 1, 'result': {'protocol_version': 0, 'encoding': 'json', 'events': ['focus']}}), flush=True)
last = None
while True:
    if select.select([sys.stdin], [], [], 0.02)[0] and not sys.stdin.readline():
        break
    path = os.path.join(root, 'hints.json')
    if os.path.exists(path):
        text = open(path).read()
        if text != last:
            last = text
            print(json.dumps({'jsonrpc': '2.0', 'method': 'speech.hints', 'params': {'hotwords': json.loads(text)}}), flush=True)
";

    const SIDECAR: &str = r"import sys,json,pathlib
print(json.dumps({'ready':True}),flush=True)
for line in sys.stdin:
 c=json.loads(line)
 if 'vocabulary' in c:
  pathlib.Path(sys.argv[1]).write_text(','.join(w for w in c['vocabulary'] if w.startswith('hint')))
";

    struct Harness {
        directory: tempfile::TempDir,
        compositor: Compositor,
        event_loop: EventLoop<'static, Compositor>,
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
            compositor.set_speech(SpeechProcess::new(vec![
                "python3".into(),
                "-u".into(),
                "-c".into(),
                SIDECAR.into(),
                directory.path().join("words").to_str().unwrap().into(),
            ]));
            Self {
                directory,
                compositor,
                event_loop,
                _display: display,
            }
        }

        fn focus(&mut self, app_id: &str) {
            self.compositor.handle(&Event::Focus {
                app_id: Some(app_id.into()),
                title: String::new(),
            });
        }

        fn hints(&self, words: &[&str]) {
            std::fs::write(
                self.directory.path().join("hints.json"),
                serde_json::to_vec(words).unwrap(),
            )
            .unwrap();
        }

        /// Pumps until the sidecar last heard `expected`.
        fn await_words(&mut self, expected: &str) {
            let marker = self.directory.path().join("words");
            let deadline = Instant::now().checked_add(Duration::from_secs(5)).unwrap();
            while read(&marker) != expected {
                assert!(
                    Instant::now() < deadline,
                    "sidecar heard {:?}, expected {expected:?}",
                    read(&marker)
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

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|_| "pending".into())
    }

    #[test]
    fn a_plugins_hints_apply_only_while_its_context_is_focused() {
        let mut harness = Harness::new();
        harness.hints(&["hint chat with guild"]);
        harness.focus("chatty");
        harness.await_words("hint chat with guild");
        harness.focus("elsewhere");
        harness.await_words("");
    }

    #[test]
    fn a_second_notice_replaces_the_first_for_the_next_request() {
        let mut harness = Harness::new();
        harness.focus("chatty");
        harness.hints(&["hint alpha"]);
        harness.await_words("hint alpha");
        harness.hints(&["hint beta"]);
        harness.await_words("hint beta");
    }

    #[test]
    fn context_hotwords_merge_with_the_global_context_and_plugin_hints() {
        let mut harness = Harness::new();
        let contexts = [
            "app_id = \"chatty\"\n[buttons]\n[speech]\nhotwords = [\"hint own\"]\n",
            "app_id = \"*\"\n[buttons]\n[speech]\nhotwords = [\"hint global\"]\n",
        ]
        .map(|source| tehuti_protocol::parse_context(source).unwrap())
        .to_vec();
        harness.compositor.reload_contexts(Ok(contexts));
        harness.hints(&["hint plugin"]);
        harness.focus("chatty");
        harness.await_words("hint own,hint global,hint plugin");
    }

    #[test]
    fn over_the_cap_the_plugins_last_entries_go_first() {
        let mut harness = Harness::new();
        let owned: Vec<String> = (0..70).map(|index| format!("hint w{index}")).collect();
        let words: Vec<&str> = owned.iter().map(String::as_str).collect();
        harness.hints(&words);
        harness.focus("chatty");
        harness.await_words(&owned[..64].join(","));
    }
}
