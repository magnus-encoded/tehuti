//! Hotwords cross the compositor's capture, desktop and launcher boundaries.

#[cfg(test)]
mod hotword_routing {
    use core::time::Duration;
    use smithay::reexports::wayland_server::Display;
    use std::{os::unix::fs::PermissionsExt, path::Path, time::Instant};
    use tehuti::{
        input::{SyntheticPad, route},
        speech::SpeechProcess,
        state::Compositor,
    };
    use tehuti_protocol::{ButtonName, ButtonState, Event, parse_context};

    fn isolated(probe: &str) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("applications")).unwrap();
        std::fs::create_dir_all(root.join("tehuti")).unwrap();
        std::fs::write(root.join("tehuti/vocabulary.txt"), "  Quasar\n\nNebula\n").unwrap();
        std::fs::write(
            root.join("applications/atlas.desktop"),
            "[Desktop Entry]\nName=Atlas\n",
        )
        .unwrap();
        let opener = root.join("xdg-open");
        std::fs::write(
            &opener,
            "#!/bin/sh\nprintf '%s\\n' \"$WAYLAND_DISPLAY\" \"$@\" > \"$TEHUTI_TEST_ROOT/opening\"\n/bin/mv \"$TEHUTI_TEST_ROOT/opening\" \"$TEHUTI_TEST_ROOT/opened\"\n",
        )
        .unwrap();
        std::fs::set_permissions(opener, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("hotword_routing::{probe}")])
            .env("TEHUTI_TEST_ROOT", root)
            .env("HOME", root)
            .env("XDG_DATA_HOME", root)
            .env("XDG_DATA_DIRS", root.join("empty"))
            .env("XDG_CONFIG_HOME", root)
            .env("PATH", root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn tick(compositor: &mut Compositor) {
        compositor.handle(&Event::Tick {
            elapsed: Duration::from_millis(10),
        });
    }

    fn press(compositor: &mut Compositor, state: ButtonState) {
        let mut pad = SyntheticPad::default();
        pad.push(Event::Button {
            name: ButtonName::Record,
            state,
        });
        route(compositor, &mut pad).unwrap();
    }

    fn wait_for(compositor: &mut Compositor, path: &Path, seconds: u64) {
        let deadline = Instant::now().checked_add(Duration::from_secs(seconds)).unwrap();
        while !path.exists() {
            tick(compositor);
            assert!(Instant::now() < deadline, "missing {}", path.display());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn record_press_freezes_all_context_phrases_and_user_vocabulary() {
        isolated("capture_probe");
    }

    #[test]
    fn capture_probe() {
        let Some(root) = std::env::var_os("TEHUTI_TEST_ROOT").map(std::path::PathBuf::from) else {
            return;
        };
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_contexts(vec![
            parse_context(
                "app_id = '*'\n[buttons]\n[hotwords]\napps = false\natlas = { path = '/tmp/atlas' }\nnebular = { recognition-only = true }",
            )
            .unwrap(),
            parse_context("app_id = 'editor'\n[buttons]\n[speech]\n'open settings' = { key = 'enter' }")
                .unwrap(),
            parse_context("app_id = 'viewer'\n[buttons]\n[speech]\n'next page' = { key = 'tab' }").unwrap(),
        ]);
        let source = r#"import json,os,sys
from pathlib import Path
root=Path(os.environ['TEHUTI_TEST_ROOT'])
print('{"ready":true}',flush=True)
(root/'ready').touch()
for line in sys.stdin:
    command=json.loads(line)
    if 'start' in command:
        (root/'capturing').write_text(json.dumps(command['hotwords']))
        (root/'capturing').replace(root/'captured')
    if 'stop' in command:
        (root/'stopped').write_text('done')
"#;
        compositor.set_speech(SpeechProcess::new(vec![
            "/usr/bin/python3".into(),
            "-u".into(),
            "-c".into(),
            source.into(),
        ]));
        wait_for(&mut compositor, &root.join("ready"), 3);
        tick(&mut compositor);
        press(&mut compositor, ButtonState::Pressed);
        compositor.reload_contexts(Ok(vec![]));
        std::fs::write(root.join("tehuti/vocabulary.txt"), "Changed\n").unwrap();
        press(&mut compositor, ButtonState::Released);
        wait_for(&mut compositor, &root.join("stopped"), 3);
        let words: Vec<String> =
            serde_json::from_str(&std::fs::read_to_string(root.join("captured")).unwrap()).unwrap();
        assert_eq!(
            words,
            [
                "Nebula",
                "Quasar",
                "atlas",
                "nebular",
                "next page",
                "open settings"
            ]
        );
    }

    #[test]
    fn desktop_poll_refreshes_the_app_hotwords_used_for_speech() {
        isolated("refresh_probe");
    }

    #[test]
    fn refresh_probe() {
        let Some(root) = std::env::var_os("TEHUTI_TEST_ROOT").map(std::path::PathBuf::from) else {
            return;
        };
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_contexts(vec![]);
        std::fs::write(
            root.join("applications/atlas.desktop"),
            "[Desktop Entry]\nName=Fresh Atlas\n",
        )
        .unwrap();
        compositor.set_speech(SpeechProcess::new(vec![
            "/usr/bin/python3".into(),
            "-u".into(),
            "-c".into(),
            r#"import json,os,sys
from pathlib import Path
print('{"ready":true}',flush=True)
for line in sys.stdin:
    command=json.loads(line)
    if 'fresh atlas' in command.get('hotwords',[]):
        Path(os.environ['TEHUTI_TEST_ROOT'],'refreshed').touch()
"#
            .into(),
        ]));
        let deadline = Instant::now().checked_add(Duration::from_secs(35)).unwrap();
        while !root.join("refreshed").exists() {
            tick(&mut compositor);
            press(&mut compositor, ButtonState::Pressed);
            press(&mut compositor, ButtonState::Released);
            assert!(
                Instant::now() < deadline,
                "desktop refresh did not reach capture vocabulary"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn path_speech_runs_the_default_handler_from_the_compositor() {
        isolated("open_probe");
    }

    #[test]
    fn open_probe() {
        let Some(root) = std::env::var_os("TEHUTI_TEST_ROOT").map(std::path::PathBuf::from) else {
            return;
        };
        std::fs::write(
            root.join("context.toml"),
            "app_id = '*'\n[buttons]\n[hotwords]\n'atlas project' = { path = '~/Documents/atlas' }",
        )
        .unwrap();
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_contexts(tehuti::contexts::load(&root));
        compositor.set_launch_socket("test-socket".into());
        compositor.handle(&Event::Speech {
            text: tehuti_protocol::Text::new("open atlas project".into()).unwrap(),
        });
        wait_for(&mut compositor, &root.join("opened"), 3);
        assert_eq!(
            std::fs::read_to_string(root.join("opened")).unwrap(),
            format!("test-socket\n{}/Documents/atlas\n", root.display())
        );
    }
    #[test]
    fn url_speech_runs_the_default_handler_without_expanding_the_url() {
        isolated("url_probe");
    }

    #[test]
    fn url_probe() {
        let Some(root) = std::env::var_os("TEHUTI_TEST_ROOT").map(std::path::PathBuf::from) else {
            return;
        };
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_contexts(vec![parse_context(
            "app_id = '*'\n[buttons]\n[hotwords]\natlas = { url = 'https://example.org/~/atlas?q=one&two=three' }",
        ).unwrap()]);
        compositor.set_launch_socket("test-socket".into());
        compositor.handle(&Event::Speech {
            text: tehuti_protocol::Text::new("open atlas".into()).unwrap(),
        });
        wait_for(&mut compositor, &root.join("opened"), 3);
        assert_eq!(
            std::fs::read_to_string(root.join("opened")).unwrap(),
            "test-socket\nhttps://example.org/~/atlas?q=one&two=three\n"
        );
    }
}
