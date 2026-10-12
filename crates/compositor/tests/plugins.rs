//! Plugin isolation laws through public seams and real child processes.

#[cfg(test)]
mod plugins {
    use core::time::Duration;
    use proptest::prelude::*;
    use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
    use std::{
        io::Write,
        os::unix::fs::PermissionsExt,
        sync::{Arc, Mutex},
        time::Instant,
    };
    use tehuti::{
        input::{SyntheticPad, route},
        plugins::install,
        state::Compositor,
    };
    use tehuti_protocol::{ButtonName, ButtonState, Event, Manifest, Permission, StartPolicy, Subscription};

    struct Harness {
        event_loop: EventLoop<'static, Compositor>,
        compositor: Compositor,
        directory: tempfile::TempDir,
        _display: Display<Compositor>,
    }

    impl Harness {
        fn new(manifest: &Manifest, mode: &str, version: u64, encoding: &str) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let script = format!(
                r"#!/usr/bin/env python3
import json, sys, time
held = None
from pathlib import Path
root = Path(__file__).parent
(root / 'pid').write_text(str(__import__('os').getpid()))
for line in sys.stdin:
    message = json.loads(line)
    with (root / 'received').open('a') as log:
        log.write(line)
    if message['method'] == 'initialize':
        if {mode:?} == 'slow_initialize':
            time.sleep(2.1)
        result = {{'protocol_version': {version}, 'encoding': {encoding:?}, 'events': ['button', 'focus']}}
    elif message['method'] == 'button':
        if {mode:?} == 'out_of_order':
            if held is None:
                held = message['id']
                continue
            print(json.dumps({{'jsonrpc': '2.0', 'id': message['id'], 'result': []}}), flush=True)
            print(json.dumps({{'jsonrpc': '2.0', 'id': held, 'result': [{{'type': 'quit'}}]}}), flush=True)
            (root / 'replied').write_text('yes')
            continue
        if {mode:?} == 'slow':
            time.sleep(0.4)
        result = [{{'type': 'quit'}}] if {mode:?} not in ('empty', 'duplicate') else []
    else:
        continue
    if message['method'] == 'button' and {mode:?} == 'junk':
        sys.stdout.write('not json\n' + 'x' * 70000 + '\n')
    if message['method'] == 'button' and {mode:?} == 'notice':
        print(json.dumps({{'jsonrpc': '2.0', 'method': 'actions', 'params': {{'actions': [{{'type': 'quit'}}]}}}}), flush=True)
        result = []
    print(json.dumps({{'jsonrpc': '2.0', 'id': message['id'], 'result': result}}), flush=True)
    if message['method'] == 'button' and {mode:?} == 'duplicate':
        print(json.dumps({{'jsonrpc': '2.0', 'id': message['id'], 'result': [{{'type': 'quit'}}]}}), flush=True)
    (root / 'replied').write_text('yes')
"
            );
            let executable = directory.path().join("plugin.py");
            std::fs::write(&executable, script).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::write(
                directory.path().join("plugin.json"),
                serde_json::to_vec(manifest).unwrap(),
            )
            .unwrap();
            let display = Display::new().unwrap();
            let mut compositor = Compositor::new(&display.handle()).unwrap();
            let event_loop = EventLoop::try_new().unwrap();
            compositor.set_plugins(install(&event_loop, directory.path(), |state| state).unwrap());
            Self {
                event_loop,
                compositor,
                directory,
                _display: display,
            }
        }

        fn pump_until(&mut self, condition: impl Fn(&Self) -> bool) {
            let started = Instant::now();
            while !condition(self) {
                assert!(
                    started.elapsed() < Duration::from_secs(5),
                    "plugin did not progress"
                );
                self.event_loop
                    .dispatch(Duration::from_millis(5), &mut self.compositor)
                    .unwrap();
            }
        }

        fn ready(&mut self) {
            self.compositor.handle(&Event::Focus {
                app_id: Some("test".into()),
                title: String::new(),
            });
            self.pump_until(|harness| {
                harness.compositor.initialized_plugins() == 1
                    && harness.directory.path().join("replied").exists()
            });
        }

        fn send(&mut self, events: impl IntoIterator<Item = Event>) {
            let mut pad = SyntheticPad::default();
            for event in events {
                pad.push(event);
            }
            route(&mut self.compositor, &mut pad).unwrap();
        }

        fn received(&self) -> Vec<serde_json::Value> {
            std::fs::read_to_string(self.directory.path().join("received"))
                .unwrap_or_default()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
    }

    fn manifest() -> Manifest {
        Manifest {
            executable: "plugin.py".into(),
            app_ids: vec!["test".into()],
            events: vec![Subscription::Button, Subscription::Focus],
            start: StartPolicy::Always,
            permissions: vec![Permission::Quit],
        }
    }

    fn button(name: ButtonName) -> Event {
        Event::Button {
            name,
            state: ButtonState::Pressed,
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]
        #[test]
        fn tick_and_consumed_buttons_never_cross_any_subscribed_plugin_wire(
            focus_subscription in any::<bool>(), button_subscription in any::<bool>(), on_focus in any::<bool>(),
            permissions in prop::collection::vec(any::<bool>(), 0..10),
            ticks in prop::collection::vec(0u64..500, 0..20),
            events in prop::collection::vec(prop::sample::select(vec![ButtonName::South, ButtonName::Guide]), 0..20),
        ) {
            let mut configuration = manifest();
            configuration.events = [focus_subscription.then_some(Subscription::Focus), button_subscription.then_some(Subscription::Button)].into_iter().flatten().collect();
            configuration.start = if on_focus { StartPolicy::OnFocus } else { StartPolicy::Always };
            configuration.permissions = permissions.into_iter().map(|quit| if quit { Permission::Quit } else { Permission::Key }).collect();
            let mut harness = Harness::new(&configuration, "empty", 0, "json");
            harness.ready();
            harness.send(events.into_iter().map(button).chain(ticks.into_iter().map(|millis| Event::Tick { elapsed: Duration::from_millis(millis) })));
            harness.send([button(ButtonName::Guide), Event::Tick { elapsed: Duration::from_secs(3) }]);
            prop_assert!(harness.compositor.quit_requested(), "no plugin can suppress escape");
            for _attempt in 0..10 { harness.event_loop.dispatch(Duration::from_millis(1), &mut harness.compositor).unwrap(); }
            for message in harness.received() {
                prop_assert!(matches!(message["method"].as_str(), Some("initialize" | "focus")), "consumed events never cross the wire: {message}");
            }
        }

        #[test]
        fn a_late_reply_cannot_execute_under_any_permission_configuration(
            permissions in prop::collection::vec(prop::sample::select(vec![Permission::Key, Permission::Text, Permission::Quit]), 0..10),
            name in prop::sample::select(vec![ButtonName::East, ButtonName::North, ButtonName::West]),
        ) {
            let mut configuration = manifest();
            configuration.permissions = permissions;
            let mut harness = Harness::new(&configuration, "slow", 0, "json");
            harness.ready();
            std::fs::remove_file(harness.directory.path().join("replied")).unwrap();
            harness.send([button(name)]);
            harness.pump_until(|harness| harness.directory.path().join("replied").exists());
            for _attempt in 0..3 { harness.event_loop.dispatch(Duration::from_millis(5), &mut harness.compositor).unwrap(); }
            prop_assert!(!harness.compositor.quit_requested(), "a late Quit reply is dropped even with permission");
        }
    }

    #[test]
    fn a_late_initialize_reply_is_refused() {
        let mut harness = Harness::new(&manifest(), "slow_initialize", 0, "json");
        harness.pump_until(|harness| harness.directory.path().join("replied").exists());
        for _attempt in 0..3 {
            harness
                .event_loop
                .dispatch(Duration::from_millis(5), &mut harness.compositor)
                .unwrap();
        }
        assert_eq!(
            harness.compositor.initialized_plugins(),
            0,
            "late initialize cannot activate a plugin"
        );
        harness.send([
            button(ButtonName::Guide),
            Event::Tick {
                elapsed: Duration::from_secs(3),
            },
        ]);
        assert!(
            harness.compositor.quit_requested(),
            "input survives initialize timeout"
        );
    }

    #[test]
    fn dropping_the_loop_terminates_and_reaps_the_plugin() {
        let mut harness = Harness::new(&manifest(), "empty", 0, "json");
        harness.ready();
        let process = std::fs::read_to_string(harness.directory.path().join("pid")).unwrap();
        drop(harness);
        assert!(
            !std::path::Path::new(&format!("/proc/{process}")).exists(),
            "the plugin is terminated and reaped with its loop"
        );
    }

    #[test]
    fn replies_can_arrive_out_of_order() {
        let mut harness = Harness::new(&manifest(), "out_of_order", 0, "json");
        harness.ready();
        harness.send([button(ButtonName::East), button(ButtonName::North)]);
        harness.pump_until(|harness| harness.compositor.quit_requested());
    }

    #[test]
    fn bad_and_overlong_lines_do_not_discard_the_next_valid_reply() {
        let mut harness = Harness::new(&manifest(), "junk", 0, "json");
        harness.ready();
        harness.send([button(ButtonName::East)]);
        harness.pump_until(|harness| harness.compositor.quit_requested());
    }

    #[test]
    fn action_notices_obey_permissions_and_duplicate_replies_are_dropped() {
        for (mode, permitted, expected) in [
            ("notice", true, true),
            ("notice", false, false),
            ("duplicate", true, false),
        ] {
            let mut configuration = manifest();
            if !permitted {
                configuration.permissions.clear();
            }
            let mut harness = Harness::new(&configuration, mode, 0, "json");
            harness.ready();
            std::fs::remove_file(harness.directory.path().join("replied")).unwrap();
            harness.send([button(ButtonName::East)]);
            harness.pump_until(|harness| harness.directory.path().join("replied").exists());
            for _attempt in 0..3 {
                harness
                    .event_loop
                    .dispatch(Duration::from_millis(5), &mut harness.compositor)
                    .unwrap();
            }
            assert_eq!(
                harness.compositor.quit_requested(),
                expected,
                "notice permissions and request ownership gate actions"
            );
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]
        #[test]
        fn subscriptions_bound_the_events_sent_to_every_generated_manifest(
            subscribe_button in any::<bool>(), subscribe_focus in any::<bool>(),
            names in prop::collection::vec(prop::sample::select(vec![ButtonName::East, ButtonName::West, ButtonName::North]), 1..10),
            states in prop::collection::vec(any::<bool>(), 1..10),
            app_id in prop::sample::select(vec!["test", "other"]),
        ) {
            let mut configuration = manifest();
            configuration.events = [subscribe_button.then_some(Subscription::Button), subscribe_focus.then_some(Subscription::Focus)].into_iter().flatten().collect();
            let mut harness = Harness::new(&configuration, "empty", 0, "json");
            harness.ready();
            harness.send([Event::Focus { app_id: Some(app_id.into()), title: String::new() }]);
            for (name, pressed) in names.into_iter().zip(states) {
                harness.send([Event::Button { name, state: if pressed { ButtonState::Pressed } else { ButtonState::Released } }]);
            }
            for _attempt in 0..10 { harness.event_loop.dispatch(Duration::from_millis(5), &mut harness.compositor).unwrap(); }
            let messages = harness.received();
            for message in messages {
                match message["method"].as_str() {
                    Some("initialize") => {},
                    Some("focus") => prop_assert!(subscribe_focus),
                    Some("button") => prop_assert!(subscribe_button && app_id == "test"),
                    _ => prop_assert!(false, "unsubscribed event on wire"),
                }
            }
        }
    }

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

    #[test]
    fn unsupported_handshakes_are_logged_and_input_keeps_working() {
        for (version, encoding) in [(1, "json"), (0, "protobuf")] {
            let logs = Arc::new(Mutex::new(Vec::new()));
            let writer = LogWriter(logs.clone());
            let subscriber = tracing_subscriber::fmt()
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .finish();
            tracing::subscriber::with_default(subscriber, || {
                let mut harness = Harness::new(&manifest(), "quit", version, encoding);
                harness.pump_until(|harness| harness.directory.path().join("replied").exists());
                for _attempt in 0..5 {
                    harness
                        .event_loop
                        .dispatch(Duration::from_millis(5), &mut harness.compositor)
                        .unwrap();
                }
                assert_eq!(
                    harness.compositor.initialized_plugins(),
                    0,
                    "unsupported handshake is refused"
                );
                harness.send([
                    button(ButtonName::Guide),
                    Event::Tick {
                        elapsed: Duration::from_secs(3),
                    },
                ]);
                assert!(harness.compositor.quit_requested(), "input survives refusal");
            });
            assert!(
                String::from_utf8(logs.lock().unwrap().clone())
                    .unwrap()
                    .contains("plugin handshake refused"),
                "refusal is logged"
            );
        }
    }

    #[test]
    fn permitted_replies_execute_and_unpermitted_replies_do_not() {
        for permitted in [true, false] {
            let mut configuration = manifest();
            if !permitted {
                configuration.permissions.clear();
            }
            let mut harness = Harness::new(&configuration, "quit", 0, "json");
            harness.ready();
            std::fs::remove_file(harness.directory.path().join("replied")).unwrap();
            harness.send([button(ButtonName::East)]);
            harness.pump_until(|harness| harness.directory.path().join("replied").exists());
            for _attempt in 0..3 {
                harness
                    .event_loop
                    .dispatch(Duration::from_millis(5), &mut harness.compositor)
                    .unwrap();
            }
            assert_eq!(
                harness.compositor.quit_requested(),
                permitted,
                "permissions gate plugin actions"
            );
        }
    }
}
