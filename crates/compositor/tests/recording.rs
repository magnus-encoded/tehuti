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
            for mode in ["capture", "named", "auto", "missing", "encoder_failure"] {
                isolated(mode);
            }
            return;
        }
        let mode = std::env::var("RECORDING_PROBE").unwrap();
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        compositor.load_input_choices(tehuti::input_choice_store::Store::from_env().unwrap());
        let colour = Rc::new(RefCell::new(image::Rgba([240, 20, 60, 255])));
        let (width, height) = if matches!(mode.as_str(), "duration" | "orientation") {
            (64, 64)
        } else {
            (2560, 1440)
        };
        compositor.set_capture_source(&colour, move |colour, _, _| {
            if colour[0] == 0 {
                return Err(std::io::Error::other("capture temporarily unavailable").into());
            }
            let mut frame = image::RgbaImage::from_pixel(width, height, *colour);
            for y in 0..16 {
                for x in 0..16 {
                    frame.put_pixel(x, y, image::Rgba([20, 240, 40, 255]));
                }
            }
            Ok(frame)
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
        let ready_deadline = Instant::now().checked_add(Duration::from_secs(8)).unwrap();
        while !home.join("encoder-ready").exists() && Instant::now() < ready_deadline {
            compositor.advance_time();
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            home.join("encoder-ready").exists(),
            "recording must start an encoder for wall-clock video"
        );
        if mode == "capture_failure" {
            *colour.borrow_mut() = image::Rgba([0, 0, 0, 255]);
            for _ in 0..5 {
                std::thread::sleep(Duration::from_millis(100));
                compositor.advance_time();
            }
            assert!(
                compositor
                    .voice_bar()
                    .segments()
                    .iter()
                    .any(|segment| segment.text == "REC")
            );
            *colour.borrow_mut() = image::Rgba([240, 20, 60, 255]);
        }
        if mode == "flood" {
            for _ in 0..2048 {
                compositor.host_key(smithay::input::keyboard::Keycode::new(38), true);
            }
            assert!(
                compositor
                    .voice_bar()
                    .segments()
                    .iter()
                    .any(|segment| segment.text == "REC"),
                "timeline pressure must leave recording active"
            );
        }
        if mode == "duration" {
            let until = Instant::now().checked_add(Duration::from_secs(7)).unwrap();
            while Instant::now() < until {
                compositor.advance_time();
                assert!(
                    compositor
                        .voice_bar()
                        .segments()
                        .iter()
                        .any(|segment| segment.text == "REC"),
                    "a mid-recording encoder stall must leave recording active"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
            assert!(home.join("encoder-released").exists());
        }
        if !mode.starts_with("encoder_failure") {
            compositor.say(Text::new("stop recording".into()).unwrap());
        }
        let saved_deadline = Instant::now().checked_add(Duration::from_secs(8)).unwrap();
        while !compositor
            .visible_notice_text()
            .is_some_and(|text| text.starts_with("RECORDING SAVED:") || text.starts_with("RECORDING FAILED:"))
            && Instant::now() < saved_deadline
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
        if mode.starts_with("encoder_failure") {
            assert!(
                compositor
                    .visible_notice_text()
                    .unwrap()
                    .starts_with("RECORDING FAILED:")
            );
            std::thread::sleep(Duration::from_secs(4));
            compositor.advance_time();
            assert!(
                compositor
                    .visible_notice_text()
                    .unwrap()
                    .starts_with("RECORDING FAILED:")
            );
            if mode == "encoder_failure_east" {
                compositor.handle(&Event::Button {
                    name: tehuti_protocol::ButtonName::East,
                    state: tehuti_protocol::ButtonState::Pressed,
                });
            } else {
                compositor.host_key(smithay::input::keyboard::Keycode::new(9), true);
            }
            assert!(
                !compositor
                    .visible_notice_text()
                    .is_some_and(|text| text.starts_with("RECORDING FAILED:"))
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
        if mode == "orientation" {
            assert_top_left_marker(folder);
            return;
        }
        if mode == "duration" {
            assert!(home.join("encoder-released").exists());
            assert_wall_duration(folder);
            return;
        }
        let video: serde_json::Value =
            serde_json::from_slice(&std::fs::read(folder.join("video.mp4")).unwrap()).unwrap();
        assert_eq!(video["pixel"], serde_json::json!([20, 240, 40, 255]));
        assert_eq!(video["size"], "1920x1080");
        assert_eq!(video["fps"], "30");
        let microphones = match mode.as_str() {
            "named" => vec!["mic-b"],
            "auto" => vec!["mic-a"],
            _ => vec!["mic-a", "mic-b"],
        };
        let mut inputs = vec!["pipe:0"];
        inputs.extend(microphones.iter().copied());
        inputs.push("speakers.monitor");
        assert_eq!(video["inputs"], serde_json::json!(inputs));
        let titles: Vec<_> = microphones
            .iter()
            .chain(core::iter::once(&"speakers.monitor"))
            .map(|source| format!("title={source}"))
            .collect();
        assert_eq!(video["titles"], serde_json::json!(titles));
        assert_eq!(video["dispositions"][0], "default");
        assert!(
            video["dispositions"]
                .as_array()
                .unwrap()
                .iter()
                .skip(1)
                .all(|value| value == "0")
        );
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

    #[test]
    fn east_button_dismisses_recording_encoder_failure() {
        isolated("encoder_failure_east");
    }

    #[test]
    fn recording_duration_matches_wall_time_with_a_delayed_encoder() {
        isolated("duration");
    }

    #[test]
    fn capture_failure_keeps_recording_active() {
        isolated("capture_failure");
    }

    #[test]
    fn timeline_pressure_keeps_recording_active() {
        isolated("flood");
    }

    #[test]
    fn recording_first_frame_keeps_a_top_left_marker() {
        isolated("orientation");
    }

    fn assert_top_left_marker(folder: &Path) {
        let output = std::process::Command::new("/usr/bin/ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(folder.join("video.mp4"))
            .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let frame = image::RgbaImage::from_raw(64, 64, output.stdout).unwrap();
        let marker = frame.get_pixel(8, 8);
        assert!(
            marker[1] > 200 && marker[0] < 60 && marker[2] < 80,
            "encoded first frame must keep the green marker top-left: {marker:?}"
        );
        for (x, y) in [(55, 8), (8, 55), (55, 55)] {
            assert!(frame.get_pixel(x, y)[0] > 200, "marker moved to another corner");
        }
    }

    fn assert_wall_duration(folder: &Path) {
        let timeline = std::fs::read_to_string(folder.join("timeline.jsonl")).unwrap();
        let elapsed_ms = timeline
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .find(|event| event["kind"] == "stop")
            .expect("timeline must contain the explicit stop")["time_ms"]
            .as_u64()
            .unwrap();
        let output = std::process::Command::new("/usr/bin/ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type,duration,start_time:stream_tags=handler_name:stream_disposition=default",
                "-of",
                "json",
            ])
            .arg(folder.join("video.mp4"))
            .output()
            .unwrap();
        assert!(output.status.success());
        let probe: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let streams = probe["streams"].as_array().unwrap();
        assert_eq!(
            streams.len(),
            4,
            "video, both microphones and desktop audio must survive"
        );
        let audio: Vec<_> = streams
            .iter()
            .filter(|stream| stream["codec_type"] == "audio")
            .collect();
        for (stream, source) in audio.iter().zip(["mic-a", "mic-b", "speakers.monitor"]) {
            assert_eq!(stream["tags"]["handler_name"], source);
        }
        assert_eq!(audio[0]["disposition"]["default"], 1);
        assert!(
            audio
                .iter()
                .skip(1)
                .all(|stream| stream["disposition"]["default"] == 0)
        );
        let elapsed = Duration::from_millis(elapsed_ms);
        assert!(elapsed >= Duration::from_millis(2900));
        for stream in streams {
            let duration = Duration::from_secs_f64(stream["duration"].as_str().unwrap().parse().unwrap());
            assert!(
                duration.abs_diff(elapsed) <= elapsed.div_f64(20.0),
                "video duration must match wall time within 5%, including audio: {duration:?} versus {elapsed:?}"
            );
            let start: f64 = stream["start_time"].as_str().unwrap().parse().unwrap();
            assert!(start.abs() <= 0.05, "tracks must share the session origin");
        }
    }

    fn executable(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn install_encoder(bin: &Path, mode: &str) {
        if mode != "missing" {
            executable(
                &bin.join("ffmpeg"),
                r"#!/usr/bin/python3
import json, os, pathlib, sys, time
args = sys.argv[1:]
if os.environ['RECORDING_PROBE'].startswith('encoder_failure'):
    pathlib.Path(os.environ['HOME'], 'encoder-ready').touch()
    sys.exit(1)
pixel = sys.stdin.buffer.read(4)
pathlib.Path(os.environ['HOME'], 'encoder-ready').touch()
if os.environ['RECORDING_PROBE'] == 'flood':
    time.sleep(3.2)
while sys.stdin.buffer.read(65536):
    pass
pathlib.Path(args[-1]).write_text(json.dumps({
    'pixel': list(pixel), 'size': args[args.index('-video_size') + 1],
    'fps': args[args.index('-framerate') + 1],
    'inputs': [args[i + 1] for i, arg in enumerate(args) if arg == '-i'],
    'titles': [arg for arg in args if arg.startswith('title=')],
    'dispositions': [args[i + 1] for i, arg in enumerate(args) if arg.startswith('-disposition:a:')],
}))
",
            );
        }
        if matches!(mode, "duration" | "orientation") {
            install_video_encoder(bin);
        }
    }

    fn install_video_encoder(bin: &Path) {
        executable(
            &bin.join("ffmpeg"),
            r"#!/usr/bin/python3
import os, subprocess, sys, time
from pathlib import Path
args = sys.argv[1:]
home = Path(os.environ['HOME'])
for index in range(len(args)):
    if args[index] == 'pulse':
        args[index] = 'lavfi'
    if args[index] in ('speakers.monitor', 'mic-a', 'mic-b'):
        args[index] = 'anullsrc=r=48000:cl=stereo'
# Audio fixtures need no microphone or PulseAudio server.
for index in reversed(range(len(args))):
    if args[index] == '-f' and args[index + 1] == 'lavfi':
        args[index:index] = ['-re']
child = subprocess.Popen(['/usr/bin/ffmpeg'] + args, stdin=subprocess.PIPE)
began = time.monotonic()
stalled = False
while True:
    frame = sys.stdin.buffer.read(64 * 64 * 4)
    if not frame:
        break
    child.stdin.write(frame)
    child.stdin.flush()
    home.joinpath('encoder-ready').touch()
    if os.environ['RECORDING_PROBE'] == 'duration' and not stalled and time.monotonic() - began >= 1:
        stalled = True
        time.sleep(3.2)
        home.joinpath('encoder-released').touch()
child.stdin.close()
sys.exit(child.wait())
",
        );
    }

    fn isolated(mode: &str) {
        if matches!(mode, "duration" | "orientation")
            && !["/usr/bin/ffmpeg", "/usr/bin/ffprobe"]
                .iter()
                .all(|program| Path::new(program).is_file())
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        executable(
            &bin.join("pactl"),
            "#!/bin/sh\ncase \"$1\" in get-default-source) echo mic-a;; *) echo speakers;; esac\n",
        );
        let listing = serde_json::json!(["mic-a", "mic-b"].map(|name| serde_json::json!({
            "type": "PipeWire:Interface:Node", "info": {"props": {
                "node.name": name, "media.class": "Audio/Source"}}
        })));
        executable(
            &bin.join("pw-dump"),
            &format!("#!/bin/sh\nprintf '%s' '{listing}'\n"),
        );
        let config = directory.path().join("config/tehuti");
        std::fs::create_dir_all(&config).unwrap();
        let choice = match mode {
            "named" => "mic-b",
            "auto" => "auto",
            _ => "all",
        };
        std::fs::write(config.join("speech-source"), choice).unwrap();
        install_encoder(&bin, mode);
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
