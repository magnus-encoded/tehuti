//! Plugin session rules through child processes and the public compositor seam.

#[cfg(test)]
mod contract {

    use core::time::Duration;
    use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
    use std::{
        io::Write,
        os::unix::fs::PermissionsExt,
        sync::{Arc, Mutex},
        time::Instant,
    };
    use tehuti::{plugins::install, state::Compositor};
    use tehuti_protocol::Event;

    #[derive(Clone)]
    struct LogWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for LogWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn session(body: &str, permissions: &[&str], focus_title: &str) -> (bool, String) {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let writer = LogWriter(logs.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let quit = tracing::subscriber::with_default(subscriber, || {
            let directory = tempfile::tempdir().unwrap();
            let executable = directory.path().join("plugin.py");
            let script = format!(
                "#!/usr/bin/env python3\nimport json, sys\nfrom pathlib import Path\nroot = Path(__file__).parent\ninitialize = json.loads(sys.stdin.readline())\n{body}\n(root / 'sent').write_text('yes')\nfor line in sys.stdin:\n    with (root / 'received').open('a') as log:\n        log.write(line)\n"
            );
            std::fs::write(&executable, script).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::write(
                directory.path().join("plugin.json"),
                serde_json::to_vec(&serde_json::json!({
                    "executable":"plugin.py", "app_ids":["test"], "events":["focus"],
                    "start":"always", "permissions":permissions
                }))
                .unwrap(),
            )
            .unwrap();
            let display: Display<Compositor> = Display::new().unwrap();
            let mut compositor = Compositor::new(&display.handle()).unwrap();
            let mut event_loop = EventLoop::try_new().unwrap();
            compositor.set_plugins(install(&event_loop, directory.path(), |state| state).unwrap());
            compositor.handle(&Event::Focus {
                app_id: Some("test".into()),
                title: focus_title.into(),
            });
            let started = Instant::now();
            while !directory.path().join("sent").exists() || compositor.initialized_plugins() == 0 {
                assert!(started.elapsed() < Duration::from_secs(5));
                event_loop
                    .dispatch(Duration::from_millis(5), &mut compositor)
                    .unwrap();
            }
            for _attempt in 0..10 {
                event_loop
                    .dispatch(Duration::from_millis(5), &mut compositor)
                    .unwrap();
            }
            compositor.quit_requested()
        });
        let output = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        (quit, output)
    }

    const INITIALIZE: &str = "print(json.dumps({'jsonrpc':'2.0','id':0,'result':{'protocol_version':0,'encoding':'json','events':['focus']}}), flush=True)";

    #[test]
    fn quit_stops_later_actions_and_notices_but_unpermitted_quit_does_not() {
        let body = format!(
            "{INITIALIZE}\nprint(json.dumps({{'jsonrpc':'2.0','method':'actions','params':{{'actions':[{{'type':'text','text':'before'}},{{'type':'quit'}},{{'type':'text','text':'after'}}]}}}}), flush=True)\nprint(json.dumps({{'jsonrpc':'2.0','method':'actions','params':{{'actions':[{{'type':'text','text':'later'}}]}}}}), flush=True)"
        );
        for (permissions, expected_quit, expected_texts) in
            [(vec!["text", "quit"], true, 1), (vec!["text"], false, 3)]
        {
            let (quit, logs) = session(&body, &permissions, "");
            assert_eq!(quit, expected_quit);
            assert_eq!(logs.matches("text action skipped").count(), expected_texts);
        }
    }

    #[test]
    fn invalid_action_discards_the_whole_line_and_valid_next_line_survives() {
        let body = format!(
            "{INITIALIZE}\nprint(json.dumps({{'jsonrpc':'2.0','method':'actions','params':{{'actions':[{{'type':'quit'}},{{'type':'key','key':'Tab'}}]}}}}), flush=True)\nprint(json.dumps({{'jsonrpc':'2.0','method':'actions','params':{{'actions':[{{'type':'text','text':'next'}}]}}}}), flush=True)"
        );
        let (quit, logs) = session(&body, &["quit", "text"], "");
        assert!(!quit);
        assert!(logs.contains("plugin message dropped"));
        assert_eq!(logs.matches("text action skipped").count(), 1);
    }

    #[test]
    fn preinitialize_actions_are_ignored_and_invalid_handshake_can_be_retried() {
        let body = format!(
            "print(json.dumps({{'jsonrpc':'2.0','method':'actions','params':{{'actions':[{{'type':'quit'}}]}}}}), flush=True)\nprint(json.dumps({{'jsonrpc':'2.0','id':0,'result':{{'protocol_version':0,'encoding':'json','events':[],'extra':True}}}}), flush=True)\n{INITIALIZE}\nprint(json.dumps({{'jsonrpc':'2.0','method':'actions','params':{{'actions':[{{'type':'text','text':'ready'}}]}}}}), flush=True)"
        );
        let (quit, logs) = session(&body, &["quit", "text"], "");
        assert!(!quit);
        assert!(logs.contains("extra"));
        assert_eq!(logs.matches("text action skipped").count(), 1);
    }

    #[test]
    fn initialize_errors_warn_and_allow_a_valid_reply_and_later_actions() {
        let body = format!(
            "print(json.dumps({{'jsonrpc':'2.0','id':0,'error':{{'code':-32601,'message':'Method not found','data':{{'detail':'optional'}}}}}}), flush=True)\n{INITIALIZE}\nprint(json.dumps({{'jsonrpc':'2.0','method':'actions','params':{{'actions':[{{'type':'text','text':'ready'}}]}}}}), flush=True)"
        );
        let (quit, logs) = session(&body, &["text"], "");
        assert!(!quit);
        let warning = logs
            .lines()
            .find(|line| line.contains("plugin request declined"))
            .unwrap();
        assert!(warning.contains("WARN"));
        assert!(warning.contains("-32601"));
        assert!(warning.contains("Method not found"));
        assert_eq!(logs.matches("text action skipped").count(), 1);
    }

    #[test]
    fn output_queue_limit_counts_the_final_newline() {
        let focus = tehuti_protocol::Call {
            jsonrpc: tehuti_protocol::RpcVersion::Two,
            request_id: None,
            method: tehuti_protocol::Method::Focus {
                app_id: Some("test".into()),
                title: String::new(),
            },
        };
        let overhead = serde_json::to_vec(&focus).unwrap().len();
        for (bytes, dropped) in [(65_535, false), (65_536, true)] {
            let (_, logs) = session(INITIALIZE, &[], &"a".repeat(bytes - overhead));
            assert_eq!(logs.contains("plugin focus dropped"), dropped);
        }
    }
}
