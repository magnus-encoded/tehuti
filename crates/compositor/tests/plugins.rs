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
    use tehuti_protocol::{
        ButtonName, ButtonState, Event, KeyName, Manifest, Permission, StartPolicy, Subscription,
    };

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
            plugin_id: None,
            author: None,
            security_contact: None,
            executable: "plugin.py".into(),
            app_ids: vec!["test".into()],
            events: vec![Subscription::Button, Subscription::Focus],
            start: StartPolicy::Always,
            permissions: vec![Permission::Quit],
        }
    }

    fn focus(app_id: &str) -> Event {
        Event::Focus {
            app_id: Some(app_id.into()),
            title: String::new(),
        }
    }

    fn context(
        app_id: &str,
        mappings: &std::collections::HashMap<ButtonName, KeyName>,
    ) -> tehuti_protocol::Context {
        let mut source = format!("app_id = '{app_id}'\n[buttons]\n");
        for (name, key) in mappings {
            let name = serde_json::to_value(name).unwrap();
            let key = serde_json::to_value(key).unwrap();
            source =
                source + "[buttons." + name.as_str().unwrap() + "]\nkey = '" + key.as_str().unwrap() + "'\n";
        }
        tehuti_protocol::parse_context(&source).unwrap()
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
        fn only_buttons_the_core_routes_cross_the_wire_and_escape_always_quits(
            focus_subscription in any::<bool>(), button_subscription in any::<bool>(), on_focus in any::<bool>(),
            plugin_apps in prop::collection::vec(prop::sample::select(vec!["test", "other"]), 0..3),
            permissions in prop::collection::vec(prop::sample::select(vec![Permission::Key, Permission::Text, Permission::Quit]), 0..10),
            consumes in any::<bool>(),
            configurations in prop::collection::vec((prop::sample::select(vec!["test", "other"]), prop::collection::hash_map(any::<ButtonName>(), any::<KeyName>(), 0..=8)), 0..3),
            events in prop::collection::vec(prop_oneof![
                (any::<ButtonName>(), any::<ButtonState>()).prop_map(|(name, state)| Event::Button { name, state }),
                (0u64..500).prop_map(|millis| Event::Tick { elapsed: Duration::from_millis(millis) }),
            ], 0..20),
        ) {
            let mut configuration = manifest();
            configuration.events = [focus_subscription.then_some(Subscription::Focus), button_subscription.then_some(Subscription::Button)].into_iter().flatten().collect();
            configuration.start = if on_focus { StartPolicy::OnFocus } else { StartPolicy::Always };
            configuration.app_ids = plugin_apps.into_iter().map(str::to_owned).collect();
            configuration.permissions = permissions;
            let starts = !on_focus || configuration.app_ids.iter().any(|app_id| app_id == "test");
            let contexts: Vec<_> = configurations.iter().map(|(app_id, mappings)| context(app_id, mappings)).collect();
            let mut harness = Harness::new(&configuration, if consumes { "quit" } else { "empty" }, 0, "json");
            harness.compositor.set_contexts(contexts.clone());
            harness.compositor.handle(&focus("test"));
            if starts {
                harness.pump_until(|harness| harness.compositor.initialized_plugins() == 1 && harness.directory.path().join("replied").exists());
            }
            // The most permissive core: every button it would route to any live plugin.
            let mut core = tehuti_core::State::with_contexts(contexts);
            let mut routable = Vec::new();
            let chord = [button(ButtonName::Guide), Event::Tick { elapsed: Duration::from_secs(3) }];
            for event in core::iter::once(focus("test")).chain(events.iter().cloned()).chain(chord.iter().cloned()) {
                let (next, _, sent) = tehuti_core::step_with_plugin(core, &event, true);
                core = next;
                if sent { routable.push(event); }
            }
            prop_assert!(!harness.compositor.quit_requested(), "no reply is read before the loop dispatches");
            harness.send(events);
            harness.send(chord);
            prop_assert!(harness.compositor.quit_requested(), "no plugin configuration can suppress escape");
            for _attempt in 0..10 { harness.event_loop.dispatch(Duration::from_millis(1), &mut harness.compositor).unwrap(); }
            let mut remaining = routable.iter();
            for message in harness.received() {
                match message["method"].as_str() {
                    Some("initialize") => {}
                    Some("focus") => prop_assert_eq!(&message["params"]["app_id"], "test", "only the generated focus crosses the wire"),
                    Some("button") => {
                        let wire: Event = serde_json::from_value(serde_json::json!({"type": "button", "name": message["params"]["name"], "state": message["params"]["state"]})).unwrap();
                        prop_assert!(remaining.any(|event| *event == wire), "every button on the wire is one the core routed, in order: {message}");
                    }
                    _ => prop_assert!(false, "only initialize, focus and routed buttons cross the wire (Tick never does): {message}"),
                }
            }
        }

        #[test]
        fn a_late_reply_cannot_execute_under_any_configuration(
            permissions in prop::collection::vec(prop::sample::select(vec![Permission::Key, Permission::Text, Permission::Quit]), 0..10),
            mappings in prop::collection::hash_map(any::<ButtonName>(), any::<KeyName>(), 0..=8),
            name in any::<ButtonName>(),
            button_state in any::<ButtonState>(),
        ) {
            let event = Event::Button { name, state: button_state };
            let contexts = vec![context("test", &mappings)];
            let (core, _, _) = tehuti_core::step_with_plugin(tehuti_core::State::with_contexts(contexts.clone()), &focus("test"), true);
            let (_, _, routed) = tehuti_core::step_with_plugin(core, &event, true);
            prop_assume!(routed, "only a routed button gets a reply");
            let mut configuration = manifest();
            configuration.permissions = permissions;
            let mut harness = Harness::new(&configuration, "slow", 0, "json");
            harness.compositor.set_contexts(contexts);
            harness.ready();
            std::fs::remove_file(harness.directory.path().join("replied")).unwrap();
            harness.send([event]);
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

    /// Counts the text actions the compositor carried out, observed as the
    /// warning each one logs when no client is focused.
    fn text_actions(run: impl FnOnce()) -> usize {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let writer = LogWriter(logs.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, run);
        let output = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        output.matches("text action skipped").count()
    }

    fn text_context(mappings: &std::collections::HashMap<ButtonName, String>) -> tehuti_protocol::Context {
        let mut source = String::from("app_id = 'test'\n[buttons]\n");
        for (name, text) in mappings {
            let name = serde_json::to_value(name).unwrap();
            source = source
                + "[buttons."
                + name.as_str().unwrap()
                + "]\ntext = "
                + &toml::Value::String(text.clone()).to_string()
                + "\n";
        }
        tehuti_protocol::parse_context(&source).unwrap()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]
        #[test]
        fn a_plugin_that_does_not_subscribe_to_buttons_never_changes_context_output(
            focus_subscription in any::<bool>(), on_focus in any::<bool>(), consumes in any::<bool>(),
            other_apps in prop::collection::vec(prop::sample::select(vec!["other", "third"]), 0..3),
            permissions in prop::collection::vec(prop::sample::select(vec![Permission::Key, Permission::Text, Permission::Quit]), 0..10),
            mappings in prop::collection::hash_map(any::<ButtonName>(), "[a-z]{1,8}", 0..=8),
            events in prop::collection::vec((any::<ButtonName>(), any::<ButtonState>()).prop_map(|(name, state)| Event::Button { name, state }), 0..20),
        ) {
            let mut configuration = manifest();
            configuration.events = focus_subscription.then_some(Subscription::Focus).into_iter().collect();
            configuration.start = if on_focus { StartPolicy::OnFocus } else { StartPolicy::Always };
            configuration.app_ids = core::iter::once("test").chain(other_apps).map(str::to_owned).collect();
            configuration.permissions = permissions;
            let context = text_context(&mappings);
            let with_plugin = text_actions(|| {
                let mut harness = Harness::new(&configuration, if consumes { "quit" } else { "empty" }, 0, "json");
                harness.compositor.set_contexts(vec![context.clone()]);
                harness.ready();
                harness.send(events.iter().cloned());
            });
            let without_plugin = text_actions(|| {
                let display: Display<Compositor> = Display::new().unwrap();
                let mut compositor = Compositor::new(&display.handle()).unwrap();
                compositor.set_contexts(vec![context.clone()]);
                compositor.handle(&focus("test"));
                let mut pad = SyntheticPad::default();
                for event in events.iter().cloned() { pad.push(event); }
                route(&mut compositor, &mut pad).unwrap();
            });
            prop_assert_eq!(with_plugin, without_plugin, "context output is the same as with no plugin installed");
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
