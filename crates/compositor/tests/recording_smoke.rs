//! Smoke test for the recorder's real encode path: the real `ffmpeg` records moving content for a
//! few seconds, and the file and timeline are checked. Skipped where `ffmpeg` or `ffprobe` is absent.
#[cfg(test)]
mod recording_smoke {
    use core::cell::RefCell;
    use core::time::Duration;
    use smithay::reexports::wayland_server::Display;
    use std::{os::unix::fs::PermissionsExt, path::Path, process::Command, rc::Rc, time::Instant};
    use tehuti::state::Compositor;
    use tehuti_protocol::Text;

    const PROBE: &str = "RECORDING_SMOKE_PROBE";
    const TEST: &str = "a_real_ffmpeg_take_has_moving_frames_wall_length_and_a_stop_line";

    #[expect(clippy::print_stderr, reason = "the skip must be visible in the test output")]
    fn skip(reason: &str) {
        assert!(
            std::env::var_os("CI").is_none(),
            "a missing tool must fail in CI: {reason}"
        );
        eprintln!("SKIPPED: {reason}");
    }

    fn find(program: &str) -> Option<std::path::PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|directory| directory.join(program))
            .find(|candidate| candidate.is_file())
    }

    fn executable(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn a_real_ffmpeg_take_has_moving_frames_wall_length_and_a_stop_line() {
        if std::env::var_os(PROBE).is_some() {
            record_in_probe();
            return;
        }
        let (Some(ffmpeg), Some(ffprobe)) = (find("ffmpeg"), find("ffprobe")) else {
            skip("ffmpeg or ffprobe is not on PATH; install the ffmpeg package to run this smoke test");
            return;
        };
        let Some(python) = find("python3") else {
            skip("python3 is not on PATH; it stands in for the audio server");
            return;
        };
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        executable(
            &bin.join("pactl"),
            "#!/bin/sh\ncase \"$1\" in get-default-source) echo mic-a;; *) echo speakers;; esac\n",
        );
        // No audio server in the gate: the real encoder runs unchanged except that its audio inputs
        // are silence stamped on the wall clock, as the audio server would stamp them.
        executable(
            &bin.join("ffmpeg"),
            &format!(
                "#!{}\nimport os, sys, time\nargs = sys.argv[1:]\nwhile '-wallclock' in args:\n    del args[args.index('-wallclock'):args.index('-wallclock') + 2]\nfor index in range(len(args)):\n    if args[index] == 'pulse':\n        args[index] = 'lavfi'\n    if args[index] in ('speakers.monitor', 'mic-a'):\n        args[index] = f'anullsrc=r=48000:cl=stereo,asetpts=PTS+{{time.time():.6f}}/TB'\nos.execv('{}', ['ffmpeg'] + args)\n",
                python.display(),
                ffmpeg.display()
            ),
        );
        let config = directory.path().join("config/tehuti");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("speech-source"), "auto").unwrap();
        let result = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("recording_smoke::{TEST}"), "--nocapture"])
            .env(PROBE, "1")
            .env("HOME", directory.path())
            .env("XDG_CONFIG_HOME", directory.path().join("config"))
            .env("PATH", &bin)
            .env("TEHUTI_TEST_SIZE", "1280x720")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}{}",
            result.status,
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let folders: Vec<_> = std::fs::read_dir(directory.path().join("Videos/Recordings"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(folders.len(), 1, "one take, one folder");
        check_take(&folders[0], &ffmpeg, &ffprobe);
    }

    /// Runs inside the isolated process: records about four seconds of changing colours.
    fn record_in_probe() {
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        compositor.load_input_choices(tehuti::input_choice_store::Store::from_env().unwrap());
        let colour = Rc::new(RefCell::new(image::Rgba([0, 0, 0, 255])));
        compositor.set_capture_source(&colour, |colour, _, _| {
            Ok(image::RgbaImage::from_pixel(64, 64, *colour))
        });
        compositor.say(Text::new("start recording".into()).unwrap());
        let started = Instant::now();
        let mut step: u8 = 0;
        while started.elapsed() < Duration::from_secs(4) {
            step = step.wrapping_add(7);
            *colour.borrow_mut() =
                image::Rgba([step, step.wrapping_mul(3), 255_u8.saturating_sub(step), 255]);
            compositor.advance_time();
            std::thread::sleep(Duration::from_millis(50));
        }
        compositor.say(Text::new("stop recording".into()).unwrap());
        let deadline = Instant::now().checked_add(Duration::from_secs(15)).unwrap();
        while !compositor
            .visible_notice_text()
            .is_some_and(|text| text.starts_with("RECORDING SAVED:") || text.starts_with("RECORDING FAILED:"))
            && Instant::now() < deadline
        {
            compositor.advance_time();
            std::thread::sleep(Duration::from_millis(10));
        }
        let notice = compositor.visible_notice_text();
        assert!(
            notice
                .as_ref()
                .is_some_and(|text| text.starts_with("RECORDING SAVED:")),
            "notice: {notice:?}"
        );
    }

    fn check_take(folder: &Path, ffmpeg: &Path, ffprobe: &Path) {
        let video = folder.join("video.mp4");
        // 1. The frames in the file differ from each other.
        let hashes = Command::new(ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&video)
            .args(["-map", "0:v:0", "-f", "framemd5", "-"])
            .output()
            .unwrap();
        assert!(
            hashes.status.success(),
            "{}",
            String::from_utf8_lossy(&hashes.stderr)
        );
        let text = String::from_utf8_lossy(&hashes.stdout);
        let frames: Vec<&str> = text
            .lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| line.rsplit(',').next())
            .map(str::trim)
            .collect();
        let distinct: std::collections::HashSet<_> = frames.iter().collect();
        assert!(
            frames.len() >= 40 && distinct.len() >= 20,
            "frozen picture: {} distinct of {} frames",
            distinct.len(),
            frames.len()
        );
        // 2. The media length is close to the wall time of the take.
        let timeline = std::fs::read_to_string(folder.join("timeline.jsonl")).unwrap();
        let last: serde_json::Value =
            serde_json::from_str(timeline.lines().last().expect("timeline is empty")).unwrap();
        // 3. The timeline ends with a stop line.
        assert_eq!(last["kind"], "stop", "timeline must end with a stop line: {last}");
        let wall = Duration::from_millis(last["time_ms"].as_u64().unwrap());
        let probe = Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "csv=p=0",
            ])
            .arg(&video)
            .output()
            .unwrap();
        assert!(
            probe.status.success(),
            "{}",
            String::from_utf8_lossy(&probe.stderr)
        );
        let media = Duration::from_secs_f64(String::from_utf8_lossy(&probe.stdout).trim().parse().unwrap());
        assert!(wall >= Duration::from_millis(3500), "take too short: {wall:?}");
        assert!(
            media.abs_diff(wall) <= wall.div_f64(10.0),
            "media length {media:?} is not close to wall time {wall:?}"
        );
    }
}
