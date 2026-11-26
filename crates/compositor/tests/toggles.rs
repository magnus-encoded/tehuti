//! End-to-end tests for built-in modes, plugin toggles, Toggles settings page, and bar chips.

#[cfg(test)]
mod toggles {
    use core::time::Duration;
    use smithay::{
        output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
        reexports::{calloop::EventLoop, wayland_server::Display},
        utils::Transform,
    };
    use std::{os::unix::fs::PermissionsExt, path::Path, sync::mpsc};
    use tehuti::{bar::Buffer, idle_inhibit, plugins, state::Compositor, volume};
    use tehuti_core::{bar::Tone, dispatch::Page, toggles::BuiltinMode};
    use tehuti_protocol::{ButtonName, ButtonState, Event, Manifest, PluginKind, StartPolicy, Text};

    fn press(compositor: &mut Compositor, name: ButtonName) {
        for state in [ButtonState::Pressed, ButtonState::Released] {
            compositor.handle(&Event::Button { name, state });
        }
    }

    fn say(compositor: &mut Compositor, phrase: &str) {
        compositor.handle(&Event::Speech {
            text: Text::new(phrase.into()).unwrap(),
        });
    }

    fn output(width: i32, height: i32, scale: i32) -> Output {
        let output = Output::new(
            "toggles-test".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Tehuti".into(),
                model: "Test".into(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (width, height).into(),
                refresh: 60_000,
            }),
            Some(Transform::Normal),
            Some(Scale::Integer(scale)),
            None,
        );
        output
    }

    fn bar_pixels(buffer: &mut Buffer, output: &Output, compositor: &Compositor) -> Vec<u8> {
        let bar = compositor.voice_bar();
        let (memory, _) = buffer.update(output, &bar, "12:34").unwrap();
        let mut copy = memory.clone();
        let mut pixels = Vec::new();
        copy.render()
            .draw(|bytes| {
                pixels.extend_from_slice(bytes);
                Ok::<_, core::convert::Infallible>(Vec::new())
            })
            .unwrap();
        pixels
    }

    fn settings_pixels(compositor: &Compositor) -> Vec<u8> {
        let mut buffer = compositor.settings_buffer((1280, 720)).unwrap();
        let mut pixels = Vec::new();
        buffer
            .render()
            .draw(|bytes| {
                pixels.extend_from_slice(bytes);
                Ok::<_, core::convert::Infallible>(Vec::new())
            })
            .unwrap();
        pixels
    }

    #[test]
    fn builtin_modes_drive_idle_inhibit_and_mic_mute() {
        let (inhibit_tx, inhibit_rx) = mpsc::channel();
        let inhibit_backend = idle_inhibit::Backend::with_handler(move |active| {
            inhibit_tx.send(active).map_err(|error| error.to_string())
        })
        .unwrap();

        let (audio_tx, audio_rx) = mpsc::channel();
        let audio_backend = volume::Backend::with_command(move |args| {
            audio_tx.send(args.to_vec()).map_err(|error| error.to_string())?;
            Ok(String::new())
        })
        .unwrap();

        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_idle_inhibit_backend(inhibit_backend);
        compositor.set_volume_backend(audio_backend);

        say(&mut compositor, "Stay awake on");
        assert!(
            compositor.toggles().builtin_active(BuiltinMode::StayAwake),
            "stay awake active"
        );
        assert!(
            inhibit_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "idle inhibitor acquired"
        );
        say(&mut compositor, "stay awake off");
        assert!(
            !compositor.toggles().builtin_active(BuiltinMode::StayAwake),
            "stay awake inactive"
        );
        assert!(
            !inhibit_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "idle inhibitor released"
        );

        say(&mut compositor, "microphone mute on");
        assert_eq!(
            audio_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            ["set-mute", "@DEFAULT_AUDIO_SOURCE@", "1"]
        );
        say(&mut compositor, "microphone mute off");
        assert_eq!(
            audio_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            ["set-mute", "@DEFAULT_AUDIO_SOURCE@", "0"]
        );
    }

    #[test]
    fn do_not_disturb_suppresses_notices_until_bar_chip_turns_it_off() {
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_contexts(vec![
            tehuti_protocol::parse_context(
                "app_id = '*'\n[buttons]\n[speech]\n'ping' = { notify = 'HELLO' }\n",
            )
            .unwrap(),
        ]);

        say(&mut compositor, "ping");
        assert_eq!(compositor.visible_notice_text(), Some("HELLO"));
        say(&mut compositor, "do not disturb on");
        assert!(compositor.do_not_disturb(), "do not disturb active");
        assert_eq!(compositor.visible_notice_text(), None);
        say(&mut compositor, "ping");
        assert_eq!(compositor.visible_notice_text(), None);
        assert!(
            compositor
                .voice_bar()
                .segments()
                .iter()
                .any(|segment| segment.text == "Do not disturb" && segment.tone == Tone::Mode),
            "active do not disturb chip shown on bar"
        );

        press(&mut compositor, ButtonName::Guide);
        press(&mut compositor, ButtonName::DpadDown);
        assert!(
            compositor
                .voice_bar()
                .segments()
                .iter()
                .any(|segment| segment.text == "Do not disturb" && segment.tone == Tone::ModeFocused),
            "do not disturb chip is focused on bar"
        );
        press(&mut compositor, ButtonName::South);
        assert!(!compositor.do_not_disturb(), "confirm turns focused bar chip off");
        assert!(
            !compositor
                .voice_bar()
                .segments()
                .iter()
                .any(|segment| matches!(segment.tone, Tone::Mode | Tone::ModeFocused)),
            "chip disappears once turned off"
        );
        say(&mut compositor, "ping");
        assert_eq!(compositor.visible_notice_text(), Some("HELLO"));
    }

    #[test]
    fn bar_and_toggles_page_render_active_and_focused_states_distinctly() {
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let out = output(1280, 720, 1);
        let mut bar_buffer = Buffer::default();

        let idle_bar = bar_pixels(&mut bar_buffer, &out, &compositor);
        say(&mut compositor, "stay awake on");
        let active_bar = bar_pixels(&mut bar_buffer, &out, &compositor);
        assert_ne!(idle_bar, active_bar, "active mode chip changes bar pixels");

        press(&mut compositor, ButtonName::Guide);
        let columns_bar = bar_pixels(&mut bar_buffer, &out, &compositor);
        press(&mut compositor, ButtonName::DpadDown);
        let focused_bar = bar_pixels(&mut bar_buffer, &out, &compositor);
        assert_ne!(columns_bar, focused_bar, "focusing an active chip highlights it");

        press(&mut compositor, ButtonName::Guide);
        say(&mut compositor, "pop the hood");
        for _ in 0..3 {
            press(&mut compositor, ButtonName::DpadDown);
        }
        press(&mut compositor, ButtonName::South);
        assert_eq!(compositor.settings().page, Page::Toggles);
        let first_pixels = settings_pixels(&compositor);
        press(&mut compositor, ButtonName::South);
        assert!(
            !compositor.toggles().builtin_active(BuiltinMode::StayAwake),
            "flip turns mode off"
        );
        let flipped_pixels = settings_pixels(&compositor);
        assert_ne!(first_pixels, flipped_pixels, "flipping a toggle redraws the page");
    }

    fn write_toggle_plugin(directory: &Path, log_path: &Path) {
        let script_path = directory.join("toggle_plugin.py");
        let script = format!(
            r#"#!/usr/bin/env python3
import json, sys

log = open({log:?}, "a", buffering=1)
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        sys.stdout.write(json.dumps({{"jsonrpc":"2.0","id":msg["id"],"result":{{"protocol_version":0,"encoding":"json","events":[]}}}}) + "\n")
        sys.stdout.flush()
    elif method == "toggle":
        log.write(json.dumps(msg["params"]) + "\n")
        if msg["params"]["name"] == "night light" and msg["params"]["active"] is False:
            sys.stdout.write(json.dumps({{"jsonrpc":"2.0","method":"toggles","params":{{"toggles":[{{"name":"focus lens","active":True}}]}}}}) + "\n")
            sys.stdout.flush()
"#,
            log = log_path.to_str().unwrap()
        );
        std::fs::write(&script_path, script).unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let manifest = Manifest {
            kind: PluginKind::Application,
            verbs: Vec::new(),
            adjusters: Vec::new(),
            toggles: vec![tehuti_protocol::Toggle::new("night light", false).unwrap()],
            plugin_id: Some("toggle-test".into()),
            author: None,
            security_contact: None,
            executable: "./toggle_plugin.py".into(),
            app_ids: Vec::new(),
            events: Vec::new(),
            start: StartPolicy::Always,
            permissions: Vec::new(),
        };
        std::fs::write(
            directory.join("toggle.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
    }

    fn pump_until(
        event_loop: &mut EventLoop<'static, Compositor>,
        compositor: &mut Compositor,
        predicate: impl Fn(&Compositor) -> bool,
    ) {
        let deadline = std::time::Instant::now()
            .checked_add(Duration::from_secs(3))
            .unwrap();
        while !predicate(compositor) && std::time::Instant::now() < deadline {
            event_loop
                .dispatch(Some(Duration::from_millis(10)), compositor)
                .unwrap();
        }
    }

    #[test]
    fn plugin_registered_toggles_round_trip_over_stdio() {
        let directory = tempfile::tempdir().unwrap();
        let log_path = directory.path().join("plugin.log");
        write_toggle_plugin(directory.path(), &log_path);

        let mut event_loop: EventLoop<'static, Compositor> = EventLoop::try_new().unwrap();
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let installed = plugins::install(&event_loop, directory.path(), |state| state).unwrap();
        compositor.set_plugins(installed);

        pump_until(&mut event_loop, &mut compositor, |state| {
            state
                .toggles()
                .items()
                .iter()
                .any(|item| item.name == "night light")
        });
        assert!(
            compositor
                .toggles()
                .items()
                .iter()
                .any(|item| item.name == "night light" && !item.active),
            "manifest toggle registers once the plugin initializes"
        );

        say(&mut compositor, "night light on");
        assert!(
            compositor.toggles().is_active("night light"),
            "voice activates plugin toggle"
        );
        assert!(
            compositor
                .voice_bar()
                .segments()
                .iter()
                .any(|segment| segment.text == "Night light" && segment.tone == Tone::Mode),
            "active plugin toggle chip appears on bar"
        );

        press(&mut compositor, ButtonName::Guide);
        press(&mut compositor, ButtonName::DpadDown);
        press(&mut compositor, ButtonName::South);
        assert!(
            !compositor.toggles().is_active("night light"),
            "bar chip turns plugin toggle off"
        );

        pump_until(&mut event_loop, &mut compositor, |state| {
            state.toggles().is_active("focus lens")
        });
        assert!(
            compositor.toggles().is_active("focus lens"),
            "runtime toggles notice replaces toggles"
        );
        assert!(
            compositor
                .voice_bar()
                .segments()
                .iter()
                .any(|segment| segment.text == "Focus lens" && segment.tone == Tone::Mode),
            "replacement active toggle shows on bar"
        );

        let logged = std::fs::read_to_string(&log_path).unwrap();
        assert!(
            logged.contains(r#""name": "night light", "active": true"#),
            "plugin received on notice"
        );
        assert!(
            logged.contains(r#""name": "night light", "active": false"#),
            "plugin received off notice"
        );
    }
}
