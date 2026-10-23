//! Real speech children fail without blocking synthetic controller routing.

#[cfg(test)]
mod speech_process {
    use core::time::Duration;
    use std::time::Instant;

    use tehuti::speech::SpeechProcess;
    use tehuti_core::{
        State,
        speech::{SpeechEffect, SpeechInput, SpeechState, speech_step},
        step,
    };
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, KeyName, Text};

    fn child(source: &str) -> SpeechProcess {
        SpeechProcess::new(vec!["python3".into(), "-u".into(), "-c".into(), source.into()])
    }

    fn wait_for(process: &mut SpeechProcess, expected: &SpeechInput) {
        let deadline = Instant::now().checked_add(Duration::from_secs(3)).unwrap();
        loop {
            if process.poll(Duration::ZERO).contains(expected) {
                return;
            }
            assert!(Instant::now() < deadline, "child did not emit {expected:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn release_reaches_source_tail_scheduler_without_transport_delay() {
        let mut process = child(
            r"import sys,json
print(json.dumps({'ready':True}),flush=True)
for line in sys.stdin:
 c=json.loads(line)
 if 'stop' in c:
  print(json.dumps({'session':c['stop'],'text':'released'}),flush=True)
",
        );
        wait_for(&mut process, &SpeechInput::Available);
        process.send(&SpeechEffect::Start { session: 0 }).unwrap();
        process
            .send(&SpeechEffect::Stop {
                session: 0,
                transcribe: true,
            })
            .unwrap();
        wait_for(
            &mut process,
            &SpeechInput::Transcript {
                session: 0,
                text: Text::new("released".into()).unwrap(),
            },
        );
    }

    #[test]
    fn hanging_child_leaves_button_and_escape_routing_available() {
        let mut process = child("import time\nprint('{\"ready\":true}', flush=True)\ntime.sleep(60)");
        wait_for(&mut process, &SpeechInput::Available);
        let began = Instant::now();
        let mut state = State::new();
        for _attempt in 0..100 {
            process.send(&SpeechEffect::Start { session: 0 }).unwrap();
            let _inputs = process.poll(Duration::ZERO);
            let (next, actions) = step(
                state,
                &Event::Button {
                    name: ButtonName::South,
                    state: ButtonState::Pressed,
                },
            );
            state = next;
            assert_eq!(
                actions,
                vec![Action::Key {
                    key: KeyName::Enter,
                    mods: Vec::new()
                }]
            );
        }
        let (state, _) = step(
            state,
            &Event::Button {
                name: ButtonName::Guide,
                state: ButtonState::Pressed,
            },
        );
        let (state, _) = step(
            state,
            &Event::Button {
                name: ButtonName::Start,
                state: ButtonState::Pressed,
            },
        );
        let (_, actions) = step(
            state,
            &Event::Tick {
                elapsed: Duration::from_secs(3),
            },
        );
        assert_eq!(actions, vec![Action::Quit]);
        assert!(
            began.elapsed() < Duration::from_secs(2),
            "speech polling blocked controller input"
        );
    }

    #[test]
    fn crash_closes_capture_once_and_restarts_after_backoff() {
        let mut process = child(
            "import sys,time\nprint('{\"ready\":true}', flush=True)\nsys.stdin.readline()\ntime.sleep(0.02)\nsys.exit(1)",
        );
        wait_for(&mut process, &SpeechInput::Available);
        let (state, _) = speech_step(SpeechState::default(), SpeechInput::Available);
        let (state, start) = speech_step(state, SpeechInput::Press);
        for effect in start {
            process.send(&effect).unwrap();
        }
        wait_for(&mut process, &SpeechInput::Unavailable);
        let (state, effects) = speech_step(state, SpeechInput::Unavailable);
        assert_eq!(
            effects,
            vec![SpeechEffect::Stop {
                session: 0,
                transcribe: false
            }]
        );
        assert!(process.send(&SpeechEffect::Start { session: 1 }).is_err());
        assert_eq!(process.take_notice(), Some("Speech is down, restarting"));
        assert_eq!(process.take_notice(), None);
        wait_for(&mut process, &SpeechInput::Available);
        assert_eq!(process.take_notice(), Some("Speech is back"));
        assert_eq!(process.take_notice(), None);
        let (state, ready_effects) = speech_step(state, SpeechInput::Available);
        assert_eq!(ready_effects, vec![]);
        let (_, released_effects) = speech_step(state, SpeechInput::Release);
        assert_eq!(released_effects, vec![], "crash already closed this capture");
    }

    #[test]
    fn repeated_exits_back_off_and_a_minute_alive_resets_the_delay() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("starts");
        let release = directory.path().join("exit");
        let source = format!(
            "import pathlib,time\nmarker=pathlib.Path({:?})\nrelease=pathlib.Path({:?})\nwith marker.open('a') as output: output.write('start\\n')\nwhile not release.exists(): time.sleep(0.001)\nraise SystemExit(7)",
            marker.to_string_lossy(),
            release.to_string_lossy()
        );
        let mut process = child(&source);
        let _initial_inputs = process.poll(Duration::ZERO);
        wait_for_starts(&marker, 1);
        for (attempt, seconds) in [0_u64, 1, 2, 5, 10, 20, 30, 30].into_iter().enumerate() {
            std::fs::write(&release, "").unwrap();
            wait_for(&mut process, &SpeechInput::Unavailable);
            std::fs::remove_file(&release).unwrap();
            if attempt == 0 {
                assert_eq!(process.take_notice(), Some("Speech is down, restarting"));
            }
            assert_eq!(process.take_notice(), None, "one notice per outage");
            if seconds == 1 {
                assert_eq!(process.poll(Duration::from_millis(999)), vec![]);
                assert!(process.send(&SpeechEffect::Start { session: 1 }).is_err());
            } else if seconds > 0 {
                assert_eq!(
                    process.poll(Duration::from_millis(
                        seconds.saturating_mul(1000).saturating_sub(1)
                    )),
                    vec![]
                );
                assert_eq!(
                    std::fs::read_to_string(&marker).unwrap().lines().count(),
                    attempt.saturating_add(1),
                    "restart happened before its deadline"
                );
            }
            let _restart_inputs = if seconds == 1 {
                process.poll(Duration::from_millis(1))
            } else {
                process.poll(Duration::from_millis(u64::from(seconds > 0)))
            };
            wait_for_starts(&marker, attempt.saturating_add(2));
        }
        let _alive_inputs = process.poll(Duration::from_secs(60));
        std::fs::write(&release, "").unwrap();
        wait_for(&mut process, &SpeechInput::Unavailable);
        std::fs::remove_file(&release).unwrap();
        let _reset_inputs = process.poll(Duration::ZERO);
        wait_for_starts(&marker, 10);
    }

    fn wait_for_starts(marker: &std::path::Path, expected: usize) {
        let deadline = Instant::now().checked_add(Duration::from_secs(3)).unwrap();
        loop {
            let count = std::fs::read_to_string(marker).map_or(0, |contents| contents.lines().count());
            if count == expected {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "expected {expected} starts, got {count}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn malformed_output_is_dropped_and_stale_reply_does_not_deliver_speech() {
        let mut process = child(
            "import sys,time\nprint('{\"ready\":true}', flush=True)\nsys.stdin.readline()\nprint('bad json')\nprint('{\"session\":0,\"text\":\"bad\\\\u0000text\"}')\nprint('{\"session\":0,\"text\":\"valid\",\"extra\":true}')\nprint('{\"session\":0,\"text\":\"valid\"}', flush=True)\ntime.sleep(60)",
        );
        wait_for(&mut process, &SpeechInput::Available);
        process.send(&SpeechEffect::Start { session: 0 }).unwrap();
        let expected = SpeechInput::Transcript {
            session: 0,
            text: Text::new("valid".into()).unwrap(),
        };
        let deadline = Instant::now().checked_add(Duration::from_secs(3)).unwrap();
        let mut received = Vec::new();
        while received.is_empty() {
            received.extend(process.poll(Duration::ZERO));
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(received, vec![expected.clone()]);
        let (_, effects) = speech_step(SpeechState::default(), expected);
        assert_eq!(effects, vec![]);
    }
    #[test]
    fn closed_stdout_fails_even_while_the_child_is_alive() {
        let mut process = child("import os,time\nos.close(1)\ntime.sleep(60)");
        wait_for(&mut process, &SpeechInput::Unavailable);
        assert_eq!(process.take_notice(), Some("Speech is down, restarting"));
        assert!(process.send(&SpeechEffect::Start { session: 0 }).is_err());
    }

    #[test]
    fn silent_startup_times_out_without_waiting_on_the_child() {
        let mut process = child("import time\ntime.sleep(60)");
        assert_eq!(process.poll(Duration::ZERO), vec![]);
        assert_eq!(
            process.poll(Duration::from_secs(120)),
            vec![SpeechInput::Unavailable]
        );
        assert!(process.send(&SpeechEffect::Start { session: 0 }).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dropping_speech_process_kills_its_descendants() {
        let marker =
            std::env::temp_dir().join(format!("tehuti-speech-descendant-{}.pid", std::process::id()));
        let source = format!(
            "import subprocess,time\nchild=subprocess.Popen(['python3','-c','import time; time.sleep(60)'])\nopen({:?},'w').write(str(child.pid))\nprint('{{\"ready\":true}}',flush=True)\ntime.sleep(60)",
            marker.to_string_lossy()
        );
        let mut process = child(&source);
        wait_for(&mut process, &SpeechInput::Available);
        let descendant: i32 = std::fs::read_to_string(&marker).unwrap().parse().unwrap();

        drop(process);

        let deadline = Instant::now().checked_add(Duration::from_secs(2)).unwrap();
        loop {
            let status = std::fs::read_to_string(format!("/proc/{descendant}/stat"));
            let terminated = status.map_or(true, |status| {
                status
                    .rsplit_once(')')
                    .is_some_and(|(_, state)| state.trim_start().starts_with('Z'))
            });
            if terminated {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "speech descendant {descendant} survived"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        std::fs::remove_file(marker).unwrap();
    }

    #[test]
    fn compositor_escape_works_while_speech_child_never_reads_commands() {
        use smithay::reexports::wayland_server::Display;
        use tehuti::{
            input::{SyntheticPad, route},
            state::Compositor,
        };

        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_speech(child(
            "import time\nprint('{\"ready\":true}', flush=True)\ntime.sleep(60)",
        ));
        compositor.handle(&Event::Tick {
            elapsed: Duration::ZERO,
        });
        let mut pad = SyntheticPad::default();
        for event in [
            Event::Button {
                name: ButtonName::Record,
                state: ButtonState::Pressed,
            },
            Event::Focus {
                app_id: Some("test".into()),
                title: String::new(),
            },
            Event::Button {
                name: ButtonName::South,
                state: ButtonState::Pressed,
            },
            Event::Button {
                name: ButtonName::Guide,
                state: ButtonState::Pressed,
            },
            Event::Button {
                name: ButtonName::Start,
                state: ButtonState::Pressed,
            },
            Event::Tick {
                elapsed: Duration::from_secs(3),
            },
        ] {
            pad.push(event);
        }
        let began = Instant::now();
        route(&mut compositor, &mut pad).unwrap();
        assert!(compositor.quit_requested());
        assert!(began.elapsed() < Duration::from_secs(2));
    }
}
