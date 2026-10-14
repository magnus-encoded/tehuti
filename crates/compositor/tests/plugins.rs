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
    use tehuti::{plugins::install, state::Compositor};
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
            Self::with_script(manifest, &plugin_script(mode, version, encoding))
        }

        fn with_script(manifest: &Manifest, script: &str) -> Self {
            let directory = tempfile::tempdir().unwrap();
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
            for event in events {
                self.compositor.handle(&event);
                if self.compositor.quit_requested() {
                    break;
                }
            }
        }

        fn received(&self) -> Vec<serde_json::Value> {
            std::fs::read_to_string(self.directory.path().join("received"))
                .unwrap_or_default()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
    }

    fn plugin_script(mode: &str, version: u64, encoding: &str) -> String {
        format!(
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
    if 'method' not in message:
        continue
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
    if message['method'] == 'button' and {mode:?} == 'invalid_action':
        print(json.dumps({{'jsonrpc':'2.0','id':42,'method':'submit_actions','params':{{'event_id':message['id'],'actions':[{{'type':'key','key':'Tab'}}]}}}}), flush=True)
        continue
    if message['method'] == 'button' and {mode:?} == 'junk':
        sys.stdout.write('not json\n' + 'x' * 70000 + '\n')
    if message['method'] == 'button' and {mode:?} == 'notice':
        print(json.dumps({{'jsonrpc': '2.0', 'method': 'actions', 'params': {{'actions': [{{'type': 'quit'}}]}}}}), flush=True)
        result = []
    if message['method'] == 'button' and {mode:?} == 'error':
        error = json.loads((root / 'error').read_text())
        print(json.dumps({{'jsonrpc': '2.0', 'id': message['id'], 'error': error}}), flush=True)
        print(json.dumps({{'jsonrpc': '2.0', 'id': message['id'], 'result': [{{'type': 'quit'}}]}}), flush=True)
    else:
        print(json.dumps({{'jsonrpc': '2.0', 'id': message['id'], 'result': result}}), flush=True)
    if message['method'] == 'button' and {mode:?} == 'duplicate':
        print(json.dumps({{'jsonrpc': '2.0', 'id': message['id'], 'result': [{{'type': 'quit'}}]}}), flush=True)
    (root / 'replied').write_text('yes')
"
        )
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

    /// Holding Guide opens the system menu; Log out is the third row down and South chooses it.
    fn escape() -> [Event; 6] {
        [
            button(ButtonName::Guide),
            Event::Tick {
                elapsed: Duration::from_secs(3),
            },
            button(ButtonName::DpadDown),
            button(ButtonName::DpadDown),
            button(ButtonName::DpadDown),
            button(ButtonName::South),
        ]
    }

    fn button(name: ButtonName) -> Event {
        Event::Button {
            name,
            state: ButtonState::Pressed,
        }
    }

    fn dispatched_plugin_events(
        contexts: Vec<tehuti_protocol::Context>,
        events: impl IntoIterator<Item = Event>,
    ) -> Vec<Event> {
        use tehuti_core::dispatch::{Effect, Input, State, step};
        let mut state = State::new(tehuti_core::State::with_contexts(contexts));
        let mut routed = Vec::new();
        for event in events {
            let mut input = Input::Event {
                event,
                elapsed: Duration::ZERO,
            };
            loop {
                let (next, effects) = step(state, input);
                state = next;
                input = Input::Resume;
                for effect in effects {
                    match effect {
                        Effect::QueryPlugin(_) => input = Input::PluginActive(true),
                        Effect::DispatchPlugin {
                            event: dispatched,
                            route,
                        } => {
                            if route {
                                routed.push(dispatched);
                            }
                            input = Input::PluginRequest(None);
                        }
                        Effect::ReadClock => input = Input::ClockRead(Duration::ZERO),
                        Effect::Supervise => input = Input::Supervised(vec![]),
                        Effect::ConfirmSecret(_) => input = Input::SecretConfirmation(false),
                        Effect::SpeechCommand(_) => input = Input::SpeechSent(true),
                        Effect::PollSpeech(_) => input = Input::SpeechPolled(vec![]),
                        Effect::InvalidReply => panic!("valid dispatch replies"),
                        Effect::SettingsMode(_)
                        | Effect::PersistMode(_)
                        | Effect::MovePointer { .. }
                        | Effect::Scroll { .. }
                        | Effect::Click { .. }
                        | Effect::MouseMode(_)
                        | Effect::System(_)
                        | Effect::PollSources
                        | Effect::SouthPressed(_)
                        | Effect::ResetSecretFocus
                        | Effect::Apply(_)
                        | Effect::PluginAction(_)
                        | Effect::PluginRejected(_)
                        | Effect::Handle(_)
                        | Effect::SpeechFailed { .. }
                        | Effect::TranscriptReceived { .. }
                        | Effect::SpeechRouted { .. } => {}
                    }
                }
                if state.busy() == 0 {
                    break;
                }
            }
        }
        routed
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
                (any::<ButtonName>().prop_filter("Guide would open the menu early", |name| *name != ButtonName::Guide), any::<ButtonState>()).prop_map(|(name, state)| Event::Button { name, state }),
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
            // The menu escape, then Guide and Start, which still quits inside mouse mode.
            let chord: Vec<Event> = escape().into_iter().chain([button(ButtonName::Guide), button(ButtonName::Start), Event::Tick { elapsed: Duration::from_secs(3) }]).collect();
            let routable = dispatched_plugin_events(contexts, core::iter::once(focus("test")).chain(events.iter().cloned()).chain(chord.iter().cloned()));
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
            let routed = dispatched_plugin_events(contexts.clone(), [focus("test"), event.clone()]);
            prop_assume!(routed.contains(&event), "only a dispatched button gets a reply");
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
    fn invalid_action_names_the_rejected_action_and_falls_through_once() {
        let output = text_actions(|| {
            let mut configuration = manifest();
            configuration.permissions.push(Permission::Key);
            let mut harness = Harness::new(&configuration, "invalid_action", 0, "json");
            let mappings = core::iter::once((ButtonName::East, String::from("fallback"))).collect();
            harness.compositor.set_contexts(vec![text_context(&mappings)]);
            harness.ready();
            harness.send([button(ButtonName::East)]);
            harness.pump_until(|harness| {
                harness
                    .received()
                    .iter()
                    .any(|message| message.get("error").is_some())
            });
            let received = harness.received();
            let rejection = received
                .iter()
                .find(|message| message.get("error").is_some())
                .unwrap();
            assert_eq!(rejection["jsonrpc"], "2.0");
            assert_eq!(rejection["id"], 42);
            assert_eq!(rejection["error"]["code"], -32602);
            let reason = rejection["error"]["message"].as_str().unwrap();
            assert!(reason.contains("action 0 (key)"));
            assert!(reason.contains("Tab"));
            harness.send([Event::Tick {
                elapsed: Duration::from_secs(3),
            }]);
        });
        assert_eq!(output, 1, "rejection releases the captured fallback exactly once");
    }

    #[test]
    fn a_late_initialize_reply_is_refused() {
        let mut harness = Harness::new(&manifest(), "slow_initialize", 0, "json");
        harness.pump_until(|harness| {
            harness
                .received()
                .iter()
                .filter(|message| message["method"] == "initialize")
                .count()
                >= 2
        });
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
        harness.send(escape());
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
        let started = Instant::now();
        while std::path::Path::new(&format!("/proc/{process}")).exists() {
            assert!(started.elapsed() < Duration::from_secs(2), "child must be reaped");
            std::thread::sleep(Duration::from_millis(1));
        }
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
                for event in &events { compositor.handle(event); }
            });
            prop_assert_eq!(with_plugin, without_plugin, "context output is the same as with no plugin installed");
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]
        #[test]
        fn generated_error_replies_route_like_empty_results_and_keep_the_plugin_active(
            code in any::<i64>(), message in "[a-z]{1,30}",
            mappings in prop::collection::hash_map(any::<ButtonName>(), "[a-z]{1,8}", 0..=8),
            names in prop::collection::vec(
                prop::sample::select(vec![ButtonName::East, ButtonName::West, ButtonName::North]), 1..8),
        ) {
            let configuration = manifest();
            let context = text_context(&mappings);
            let mut outputs = Vec::new();
            for mode in ["empty", "error"] {
                let output = text_actions(|| {
                    let mut harness = Harness::new(&configuration, mode, 0, "json");
                    harness.compositor.set_contexts(vec![context.clone()]);
                    harness.ready();
                    std::fs::write(harness.directory.path().join("error"),
                        serde_json::to_vec(&serde_json::json!({"code": code, "message": message})).unwrap()).unwrap();
                    for name in &names {
                        std::fs::remove_file(harness.directory.path().join("replied")).unwrap();
                        harness.send([button(*name)]);
                        harness.pump_until(|harness| harness.directory.path().join("replied").exists());
                        for _attempt in 0..3 {
                            harness.event_loop.dispatch(Duration::from_millis(5), &mut harness.compositor).unwrap();
                        }
                        assert!(!harness.compositor.quit_requested(), "declines complete requests and duplicate results are ignored");
                        assert_eq!(harness.compositor.initialized_plugins(), 1, "errors keep the plugin active");
                    }
                    assert_eq!(harness.received().iter().filter(|received| received["method"] == "button").count(), names.len());
                });
                outputs.push(output);
            }
            prop_assert_eq!(outputs.first(), outputs.get(1), "error and empty replies produce the same context fallback");
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

    fn launches(harness: &Harness) -> Vec<u64> {
        std::fs::read_to_string(harness.directory.path().join("launches"))
            .unwrap_or_default()
            .lines()
            .map(|line| line.parse().unwrap())
            .collect()
    }

    fn supervision_logs(run: impl FnOnce()) -> String {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let writer = LogWriter(logs.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, run);
        String::from_utf8(logs.lock().unwrap().clone()).unwrap()
    }

    #[test]
    fn crashing_processes_restart_with_exponential_backoff_without_input() {
        let output = supervision_logs(|| {
            let mut harness = Harness::with_script(&manifest(), include_str!("fixtures/plugins/crash.py"));
            harness.pump_until(|harness| launches(harness).len() >= 4);
            let timestamps = launches(&harness);
            for (pair, minimum) in timestamps.windows(2).zip([100, 200, 400]) {
                let elapsed = Duration::from_nanos(pair[1].saturating_sub(pair[0]));
                assert!(
                    elapsed >= Duration::from_millis(minimum),
                    "each restart waits for its increasing backoff"
                );
            }
            assert_eq!(
                harness.compositor.initialized_plugins(),
                0,
                "a crashing plugin cannot intercept input"
            );
        });
        assert!(output.contains("plugin restart scheduled"), "crashes are logged");
        for delay in ["delay_ms=100", "delay_ms=200", "delay_ms=400"] {
            assert!(output.contains(delay), "restart delay is logged: {delay}");
        }
    }

    #[test]
    fn synthetic_input_keeps_working_during_repeated_crashes_and_restarts() {
        let output = text_actions(|| {
            let mut harness = Harness::with_script(&manifest(), include_str!("fixtures/plugins/crash.py"));
            let mappings = core::iter::once((ButtonName::East, String::from("input"))).collect();
            harness.compositor.set_contexts(vec![text_context(&mappings)]);
            harness.send([focus("test")]);
            for count in 1..=4 {
                harness.pump_until(|harness| launches(harness).len() >= count);
                let started = Instant::now();
                harness.send([button(ButtonName::East)]);
                assert!(
                    started.elapsed() < Duration::from_millis(250),
                    "controller routing cannot wait for a plugin restart"
                );
            }
            harness.send(escape());
            assert!(harness.compositor.quit_requested(), "escape survives restarts");
        });
        assert_eq!(output, 4, "every controller event reaches its context");
    }

    #[test]
    fn hanging_decisions_expire_and_the_process_restarts() {
        let output = text_actions(|| {
            let mut harness = Harness::with_script(&manifest(), include_str!("fixtures/plugins/hang.py"));
            let mappings = core::iter::once((ButtonName::East, String::from("fallback"))).collect();
            harness.compositor.set_contexts(vec![text_context(&mappings)]);
            harness.ready();
            // Misses count per expiry instant, so each decision must expire on its own tick.
            for _ in 0..3 {
                harness.send([button(ButtonName::East)]);
                std::thread::sleep(tehuti_core::plugin::BUTTON_LATENCY_BUDGET);
                harness.compositor.handle(&Event::Tick {
                    elapsed: Duration::from_secs(3),
                });
            }
            harness.pump_until(|harness| launches(harness).len() >= 2);
            harness.send(escape());
            assert!(
                harness.compositor.quit_requested(),
                "escape survives a hung plugin"
            );
        });
        assert_eq!(
            output, 3,
            "each unanswered decision releases its fallback exactly once"
        );
    }

    #[test]
    fn repeated_garbage_and_invalid_actions_are_logged_contained_and_restarted() {
        for script in [
            include_str!("fixtures/plugins/garbage.py"),
            include_str!("fixtures/plugins/invalid-actions.py"),
        ] {
            let output = supervision_logs(|| {
                let mut configuration = manifest();
                configuration.permissions.clear();
                let mut harness = Harness::with_script(&configuration, script);
                harness.ready();
                harness.send([button(ButtonName::East)]);
                harness.pump_until(|harness| launches(harness).len() >= 2);
                assert!(
                    !harness.compositor.quit_requested(),
                    "invalid messages cannot execute actions"
                );
            });
            assert!(
                output.contains("repeated invalid messages"),
                "repeated invalid messages are logged before restarting"
            );
            assert!(
                output.contains("delay_ms=100"),
                "invalid messages use restart backoff"
            );
        }
    }

    #[test]
    fn stdout_flooding_is_bounded_and_input_keeps_working() {
        for wire in [
            "x".repeat(4096),
            String::from("{\"jsonrpc\":\"2.0\",\"method\":\"actions\",\"params\":{\"actions\":[]}}\n"),
        ] {
            let output = supervision_logs(|| {
                let mut harness =
                    Harness::with_script(&manifest(), include_str!("fixtures/plugins/flood.py"));
                std::fs::write(harness.directory.path().join("wire"), &wire).unwrap();
                harness.ready();
                harness.send([button(ButtonName::East)]);
                let started = Instant::now();
                while launches(&harness).len() < 2 {
                    assert!(
                        started.elapsed() < Duration::from_secs(2),
                        "a continuous flood is terminated rather than drained forever"
                    );
                    let dispatched = Instant::now();
                    harness
                        .event_loop
                        .dispatch(Duration::from_millis(5), &mut harness.compositor)
                        .unwrap();
                    harness.send([button(ButtonName::South)]);
                    assert!(
                        dispatched.elapsed() < Duration::from_millis(250),
                        "flood callbacks leave time for controller input"
                    );
                }
                harness.send(escape());
                assert!(harness.compositor.quit_requested(), "input survives a flood");
            });
            assert!(
                output.contains("plugin stdout flood"),
                "the byte limit logs the flood"
            );
            assert!(
                output.matches("overlong plugin line dropped").count() <= 4,
                "only a bounded amount of flood data is parsed before termination"
            );
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
                harness.send(escape());
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
