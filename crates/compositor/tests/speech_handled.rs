//! A plugin can mark speech as handled; an empty reply still declines.

#[cfg(test)]
mod speech_handled {
    use core::time::Duration;
    use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
    use std::{os::unix::fs::PermissionsExt as _, time::Instant};
    use tehuti::{plugins::install, state::Compositor};
    use tehuti_protocol::{Event, Text};

    const PLUGIN: &str = r"#!/usr/bin/env python3
import json, sys, os
root = os.path.dirname(os.path.abspath(__file__))
for line in sys.stdin:
    message = json.loads(line)
    if message.get('method') == 'initialize':
        result = {'protocol_version': 0, 'encoding': 'json', 'events': ['focus', 'speech']}
        print(json.dumps({'jsonrpc': '2.0', 'id': message['id'], 'result': result}), flush=True)
    elif message.get('method') == 'speech':
        handled = open(os.path.join(root, 'mode')).read().strip() == 'handled'
        result = {'handled': True} if handled else []
        print(json.dumps({'jsonrpc': '2.0', 'id': message['id'], 'result': result}), flush=True)
        open(os.path.join(root, 'replied'), 'w').write('yes')
";

    /// Says one phrase to a speech plugin that replies as `mode` and reports whether the
    /// "no speech mapping" notice appeared while the request could have expired.
    fn notice_seen(mode: &str) -> bool {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("plugin.py");
        std::fs::write(&executable, PLUGIN).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(directory.path().join("mode"), mode).unwrap();
        std::fs::write(
            directory.path().join("plugin.json"),
            serde_json::to_vec(&serde_json::json!({
                "executable": "plugin.py", "app_ids": ["chatty"], "events": ["focus", "speech"],
                "start": "always", "permissions": []
            }))
            .unwrap(),
        )
        .unwrap();
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let mut event_loop = EventLoop::try_new().unwrap();
        compositor.set_plugins(install(&event_loop, directory.path(), |state| state).unwrap());
        compositor.handle(&Event::Focus {
            app_id: Some("chatty".into()),
            title: String::new(),
        });
        let started = Instant::now();
        while compositor.initialized_plugins() != 1 {
            assert!(started.elapsed() < Duration::from_secs(5), "plugin did not start");
            event_loop
                .dispatch(Duration::from_millis(5), &mut compositor)
                .unwrap();
        }
        compositor.handle(&Event::Speech {
            text: Text::new("switch channel".into()).unwrap(),
        });
        let mut seen = false;
        for _ in 0..700 {
            event_loop
                .dispatch(Duration::from_millis(5), &mut compositor)
                .unwrap();
            compositor.handle(&Event::Tick {
                elapsed: Duration::from_millis(5),
            });
            seen |= compositor.visible_notice_text().as_deref() == Some("NO SPEECH MAPPING");
        }
        assert!(
            directory.path().join("replied").exists(),
            "plugin heard the speech"
        );
        seen
    }

    #[test]
    fn an_empty_reply_still_declines_to_the_notice() {
        assert!(notice_seen("decline"));
    }

    #[test]
    fn a_handled_reply_shows_no_notice() {
        assert!(!notice_seen("handled"));
    }
}
