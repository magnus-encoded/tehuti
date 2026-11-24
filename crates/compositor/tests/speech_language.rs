//! A plugin's standing dictation language reaches the sidecar's start command.

#[cfg(test)]
mod speech_language {
    use core::time::Duration;
    use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
    use std::{os::unix::fs::PermissionsExt as _, time::Instant};
    use tehuti::{plugins::install, speech::SpeechProcess, state::Compositor};
    use tehuti_core::speech::SpeechInput;
    use tehuti_protocol::Event;

    /// Sends `speech.language` for each new line in `lang`, announces it in `sent`, and exits
    /// when `quit` exists.
    const PLUGIN: &str = r"#!/usr/bin/env python3
import json, select, sys, os
root = os.path.dirname(os.path.abspath(__file__))
json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc': '2.0', 'id': 0, 'result': {'protocol_version': 0, 'encoding': 'json', 'events': ['focus']}}), flush=True)
last = None
while True:
    if select.select([sys.stdin], [], [], 0.02)[0] and not sys.stdin.readline():
        break
    if os.path.exists(os.path.join(root, 'quit')):
        sys.exit(0)
    path = os.path.join(root, 'lang')
    if os.path.exists(path):
        text = open(path).read()
        if text != last:
            last = text
            print(json.dumps({'jsonrpc': '2.0', 'method': 'speech.language', 'params': {'language': text}}), flush=True)
            open(os.path.join(root, 'sent'), 'w').write(text)
";

    const SIDECAR: &str = r"import sys,json
print(json.dumps({'ready':True}),flush=True)
for line in sys.stdin:
 if 'start' in json.loads(line):
  open(sys.argv[1],'a').write(line)
";

    struct Harness {
        directory: tempfile::TempDir,
        compositor: Compositor,
        event_loop: EventLoop<'static, Compositor>,
        _display: Display<Compositor>,
    }

    impl Harness {
        fn new() -> Self {
            Self::with_plugin(
                &serde_json::json!({
                    "executable":"plugin.py", "app_ids":["chatty"], "events":["focus"],
                    "start":"always", "permissions":[]
                }),
                PLUGIN,
            )
        }

        fn with_plugin(manifest: &serde_json::Value, source: &str) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let executable = directory.path().join("plugin.py");
            std::fs::write(&executable, source).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::write(
                directory.path().join("plugin.json"),
                serde_json::to_vec(manifest).unwrap(),
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
                directory.path().join("starts").to_str().unwrap().into(),
            ]));
            let mut harness = Self {
                directory,
                compositor,
                event_loop,
                _display: display,
            };
            harness.focus("chatty");
            harness.pump_until("the sidecar is ready", |harness| {
                harness.compositor.speech_ready()
            });
            harness
        }

        fn focus(&mut self, app_id: &str) {
            self.compositor.handle(&Event::Focus {
                app_id: Some(app_id.into()),
                title: String::new(),
            });
        }

        fn pump_until(&mut self, what: &str, done: impl Fn(&Self) -> bool) {
            let deadline = Instant::now().checked_add(Duration::from_secs(5)).unwrap();
            while !done(self) {
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                self.pump(1);
            }
        }

        fn pump(&mut self, rounds: u32) {
            for _ in 0..rounds {
                self.event_loop
                    .dispatch(Duration::from_millis(5), &mut self.compositor)
                    .unwrap();
                self.compositor.handle(&Event::Tick {
                    elapsed: Duration::from_millis(5),
                });
            }
        }

        fn file(&self, name: &str) -> String {
            std::fs::read_to_string(self.directory.path().join(name)).unwrap_or_default()
        }

        /// The plugin has sent `language` and the compositor has had time to read it.
        fn set_language(&mut self, language: &str) {
            std::fs::write(self.directory.path().join("lang"), language).unwrap();
            self.pump_until("the plugin's notice", |harness| harness.file("sent") == language);
            self.pump(40);
        }

        /// Holds and releases push-to-talk once; returns the start command it sent.
        fn session(&mut self) -> serde_json::Value {
            let before = self.file("starts").lines().count();
            self.compositor.speech_input(SpeechInput::Press);
            self.pump_until("a start command", |harness| {
                harness.file("starts").lines().count() > before
            });
            self.compositor.speech_input(SpeechInput::Release);
            self.pump(5);
            let starts = self.file("starts");
            serde_json::from_str(starts.lines().last().unwrap()).unwrap()
        }
    }

    #[test]
    fn ready_verb_definitions_reach_captures_in_every_context_and_follow_notices() {
        let verbs = serde_json::json!(["ada", {"word":"orion","locale":"en","hints":["chat"]}]);
        let source = PLUGIN
            .replace("['focus']", "['speech']")
            .replace("'speech.language'", "'verbs'")
            .replace("'language': text", "'verbs': json.loads(text)");
        let mut harness = Harness::with_plugin(
            &serde_json::json!({
                "kind":"verb", "verbs":verbs, "executable":"plugin.py", "app_ids":[],
                "events":["speech"], "start":"always", "permissions":["text"]
            }),
            &source,
        );
        harness.pump(40);
        assert_eq!(harness.session()["verbs"], verbs);
        harness.focus("elsewhere");
        assert_eq!(harness.session()["verbs"], verbs);
        let replacement = serde_json::json!([{"word":"orion","locale":"fr","hints":["canal"]}]);
        harness.set_language(&replacement.to_string());
        assert_eq!(harness.session()["verbs"], replacement);
        std::fs::remove_file(harness.directory.path().join("lang")).unwrap();
        std::fs::write(harness.directory.path().join("quit"), "").unwrap();
        harness.pump(100);
        assert_eq!(harness.session().get("verbs"), None);
    }

    #[test]
    fn a_standing_language_applies_to_every_following_session() {
        let mut harness = Harness::new();
        assert_eq!(harness.session().get("language"), None, "default: no language");
        harness.set_language("no");
        assert_eq!(harness.session()["language"], "no");
        assert_eq!(harness.session()["language"], "no", "it stays");
    }

    #[test]
    fn leaving_the_plugins_context_brings_the_default_back() {
        let mut harness = Harness::new();
        harness.set_language("no");
        harness.focus("elsewhere");
        assert_eq!(harness.session().get("language"), None);
        harness.focus("chatty");
        assert_eq!(harness.session()["language"], "no");
    }

    #[test]
    fn a_stopped_plugin_loses_its_language() {
        let mut harness = Harness::new();
        harness.set_language("no");
        std::fs::remove_file(harness.directory.path().join("lang")).unwrap();
        std::fs::write(harness.directory.path().join("quit"), "").unwrap();
        harness.pump(100);
        assert_eq!(harness.session().get("language"), None);
    }
}
