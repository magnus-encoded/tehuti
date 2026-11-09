#[cfg(test)]
mod recording {
    use core::{cell::RefCell, time::Duration};
    use smithay::reexports::wayland_server::Display;
    use std::{os::unix::fs::PermissionsExt, path::Path, rc::Rc, time::Instant};
    use tehuti::state::Compositor;
    use tehuti_protocol::{Event, Text};

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one session checks video, timeline and failure recovery through public controls"
    )]
    fn voice_recording_writes_shared_capture_and_a_searchable_timeline() {
        if std::env::var_os("RECORDING_PROBE").is_none() {
            for mode in ["capture", "missing", "encoder_failure"] {
                isolated(mode);
            }
            return;
        }
        let mode = std::env::var("RECORDING_PROBE").unwrap();
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let colour = Rc::new(RefCell::new(image::Rgba([240, 20, 60, 255])));
        compositor.set_capture_source(&colour, |colour, _, _| {
            Ok(image::RgbaImage::from_pixel(2560, 1440, *colour))
        });
        compositor.say(Text::new("start recording".into()).unwrap());
        if mode == "missing" {
            assert!(
                compositor
                    .visible_notice_text()
                    .unwrap()
                    .contains("ffmpeg package")
            );
            assert!(!compositor.quit_requested());
            return;
        }
        if mode == "capture" {
            assert!(
                compositor
                    .voice_bar()
                    .segments()
                    .iter()
                    .any(|segment| segment.text == "REC")
            );
            // Repeated starts leave one encoder and one recording folder.
            compositor.say(Text::new("start recording".into()).unwrap());
        }
        compositor.handle(&Event::Focus {
            app_id: Some("org.test.Player".into()),
            title: "A voice controlled game".into(),
        });
        compositor.say(Text::new("an unmapped phrase".into()).unwrap());
        compositor.host_key(smithay::input::keyboard::Keycode::new(38), true);
        let deadline = Instant::now().checked_add(Duration::from_secs(8)).unwrap();
        while !home.join("encoder-ready").exists() && Instant::now() < deadline {
            compositor.advance_time();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(home.join("encoder-ready").exists(), "encoder started");
        compositor.say(Text::new("stop recording".into()).unwrap());
        while !compositor
            .visible_notice_text()
            .is_some_and(|text| text.starts_with("RECORDING SAVED:") || text.starts_with("RECORDING FAILED:"))
            && Instant::now() < deadline
        {
            compositor.advance_time();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !compositor
                .voice_bar()
                .segments()
                .iter()
                .any(|segment| segment.text == "REC")
        );
        if mode == "encoder_failure" {
            assert!(
                compositor
                    .visible_notice_text()
                    .unwrap()
                    .starts_with("RECORDING FAILED:")
            );
            assert!(!compositor.quit_requested());
            return;
        }
        assert!(
            compositor
                .visible_notice_text()
                .unwrap()
                .starts_with("RECORDING SAVED:")
        );
        let folders: Vec<_> = std::fs::read_dir(home.join("Videos/Recordings"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(folders.len(), 1);
        let folder = &folders[0];
        let video: serde_json::Value =
            serde_json::from_slice(&std::fs::read(folder.join("video.mp4")).unwrap()).unwrap();
        assert_eq!(video["pixel"], serde_json::json!([240, 20, 60, 255]));
        assert_eq!(video["size"], "1920x1080");
        assert_eq!(video["fps"], "30");
        assert_eq!(
            video["inputs"],
            serde_json::json!(["pipe:0", "speakers.monitor", "default"])
        );
        assert_eq!(video["maps"], serde_json::json!(["0:v:0", "1:a:0", "2:a:0"]));
        let text = std::fs::read_to_string(folder.join("timeline.jsonl")).unwrap();
        let events: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(events.iter().any(|event| event["kind"] == "screen"
            && event["data"]["bar"].is_array()
            && event["data"]["notices"].is_array()
            && event["data"]["windows"].is_array()));
        assert!(events.iter().any(|event| event["kind"] == "stop"));
        assert!(events.iter().any(|event| event["kind"] == "key"));
        assert!(text.contains("an unmapped phrase"));
        assert!(text.contains("SpeechRouted"));
        assert!(text.contains("org.test.Player"));
        assert!(
            events
                .windows(2)
                .all(|pair| pair[0]["time_ms"].as_u64() <= pair[1]["time_ms"].as_u64())
        );
    }

    fn executable(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn isolated(mode: &str) {
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        executable(&bin.join("pactl"), "#!/bin/sh\nprintf 'speakers\\n'\n");
        if mode != "missing" {
            executable(
                &bin.join("ffmpeg"),
                r"#!/usr/bin/python3
import json, os, pathlib, sys
args = sys.argv[1:]
pathlib.Path(os.environ['HOME'], 'encoder-ready').touch()
if os.environ['RECORDING_PROBE'] == 'encoder_failure':
    sys.exit(1)
pixel = sys.stdin.buffer.read(4)
while sys.stdin.buffer.read(65536):
    pass
pathlib.Path(args[-1]).write_text(json.dumps({
    'pixel': list(pixel), 'size': args[args.index('-video_size') + 1],
    'fps': args[args.index('-framerate') + 1],
    'inputs': [args[i + 1] for i, arg in enumerate(args) if arg == '-i'],
    'maps': [args[i + 1] for i, arg in enumerate(args) if arg == '-map'],
}))
",
            );
        }
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "recording::voice_recording_writes_shared_capture_and_a_searchable_timeline",
            ])
            .env("RECORDING_PROBE", mode)
            .env("HOME", directory.path())
            .env("XDG_CONFIG_HOME", directory.path().join("config"))
            .env("PATH", bin)
            .env("TEHUTI_TEST_SIZE", "1280x720")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{mode}: {}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
