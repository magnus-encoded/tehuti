#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::sync::mpsc;
    use tehuti::volume::{Backend, Reply, Request};

    #[test]
    fn volume_worker_reads_current_value_and_applies_absolute_percentages() {
        let (sent, received) = mpsc::channel();
        let backend = Backend::with_command(move |args| {
            sent.send(args.to_vec()).map_err(|error| error.to_string())?;
            Ok("Volume: 0.42 [MUTED]\n".into())
        })
        .unwrap();
        backend.request(Request::Read).unwrap();
        assert_eq!(
            received.recv_timeout(Duration::from_secs(2)).unwrap(),
            ["get-volume", "@DEFAULT_AUDIO_SINK@"]
        );
        backend.request(Request::Set(47)).unwrap();
        assert_eq!(
            received.recv_timeout(Duration::from_secs(2)).unwrap(),
            ["set-volume", "@DEFAULT_AUDIO_SINK@", "47%"]
        );
        let deadline = std::time::Instant::now()
            .checked_add(Duration::from_secs(2))
            .unwrap();
        let mut replies = Vec::new();
        while replies.len() < 2 && std::time::Instant::now() < deadline {
            replies.extend(backend.poll());
            std::thread::yield_now();
        }
        assert_eq!(replies, [Reply::Read(Ok(42)), Reply::Set(Ok(()))]);
    }

    #[test]
    fn audio_errors_are_reported_without_stopping_the_worker() {
        let backend = Backend::with_command(|_| Err("unavailable".into())).unwrap();
        backend.request(Request::Read).unwrap();
        backend.request(Request::Set(30)).unwrap();
        let deadline = std::time::Instant::now()
            .checked_add(Duration::from_secs(2))
            .unwrap();
        let mut replies = Vec::new();
        while replies.len() < 2 && std::time::Instant::now() < deadline {
            replies.extend(backend.poll());
            std::thread::yield_now();
        }
        assert_eq!(
            replies,
            [
                Reply::Read(Err("unavailable".into())),
                Reply::Set(Err("unavailable".into()))
            ]
        );
    }
    #[test]
    fn spoken_volume_previews_through_the_audio_adapter_and_draws_a_gauge() {
        use smithay::reexports::wayland_server::Display;
        use tehuti::{adjuster::raster, state::Compositor};
        use tehuti_core::dispatch::Sticks;
        use tehuti_protocol::{ButtonName, ButtonState, Event, Text};
        let (sent, received) = mpsc::channel();
        let backend = Backend::with_command(move |args| {
            sent.send(args.to_vec()).map_err(|error| error.to_string())?;
            Ok("Volume: 0.40".into())
        })
        .unwrap();
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_volume_backend(backend);
        compositor.handle(&Event::Speech {
            text: Text::new("volume".into()).unwrap(),
        });
        assert_eq!(
            received.recv_timeout(Duration::from_secs(2)).unwrap()[0],
            "get-volume"
        );
        let deadline = std::time::Instant::now()
            .checked_add(Duration::from_secs(2))
            .unwrap();
        while compositor.adjuster().is_none() && std::time::Instant::now() < deadline {
            compositor.handle(&Event::Tick {
                elapsed: Duration::ZERO,
            });
            std::thread::yield_now();
        }
        let before = raster(compositor.adjuster().unwrap()).unwrap();
        compositor.sample_sticks(Sticks {
            left: (0, -1000),
            right: (0, 0),
        });
        compositor.handle(&Event::Tick {
            elapsed: Duration::from_millis(100),
        });
        assert_eq!(compositor.adjuster().unwrap().value(), 45);
        let after = raster(compositor.adjuster().unwrap()).unwrap();
        assert_ne!(before, after);
        assert_eq!(after.len(), 280 * 224 * 4);
        assert_eq!(received.recv_timeout(Duration::from_secs(2)).unwrap()[2], "45%");
        compositor.handle(&Event::Button {
            name: ButtonName::East,
            state: ButtonState::Pressed,
        });
        assert!(compositor.adjuster().is_none());
        assert_eq!(received.recv_timeout(Duration::from_secs(2)).unwrap()[2], "40%");
    }

    const SAMPLE_STATUS: &str = "\
PipeWire 'pipewire-0' [1.2.0, user@host, cookie:1]
 └─ Clients:
        31. WirePlumber                         [1.2.0]

Audio
 ├─ Devices:
 │      40. Built-in Audio                      [alsa]
 │
 ├─ Sinks:
 │  *   47. Built-in Speakers                   [vol: 0.50]
 │      51. Headphones                          [vol: 0.80 MUTED]
 │
 ├─ Sources:
 │  *   52. USB Microphone                      [vol: 1.00]
 │
 └─ Streams:

Video
 ├─ Sinks:
 │      99. Virtual Camera
";

    #[test]
    fn parse_sinks_reads_only_audio_sinks_and_their_default_marker() {
        let sinks = tehuti::volume::parse_sinks(SAMPLE_STATUS);
        assert_eq!(sinks.len(), 2);
        assert_eq!(sinks[0].id, 47);
        assert_eq!(sinks[0].name, "Built-in Speakers");
        assert!(sinks[0].default, "starred sink must be marked default");
        assert_eq!(sinks[1].id, 51);
        assert_eq!(sinks[1].name, "Headphones");
        assert!(!sinks[1].default, "unstarred sink must not be default");
    }

    struct FakeAudio {
        sink_muted: std::sync::Arc<std::sync::atomic::AtomicBool>,
        mic_muted: std::sync::Arc<std::sync::atomic::AtomicBool>,
        default_sink: std::sync::Arc<std::sync::atomic::AtomicU32>,
        received: mpsc::Receiver<Vec<String>>,
    }

    fn fake_audio_backend() -> (Backend, FakeAudio) {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicU32},
        };
        let sink_muted = Arc::new(AtomicBool::new(false));
        let mic_muted = Arc::new(AtomicBool::new(false));
        let default_sink = Arc::new(AtomicU32::new(47));
        let sink_flag = Arc::clone(&sink_muted);
        let mic_flag = Arc::clone(&mic_muted);
        let sink_id = Arc::clone(&default_sink);
        let (sent, received) = mpsc::channel();
        let backend = Backend::with_command(move |args| {
            sent.send(args.to_vec()).map_err(|error| error.to_string())?;
            fake_wpctl(args, &sink_flag, &mic_flag, &sink_id)
        })
        .unwrap();
        (
            backend,
            FakeAudio {
                sink_muted,
                mic_muted,
                default_sink,
                received,
            },
        )
    }

    fn fake_wpctl(
        args: &[String],
        sink_flag: &std::sync::atomic::AtomicBool,
        mic_flag: &std::sync::atomic::AtomicBool,
        sink_id: &std::sync::atomic::AtomicU32,
    ) -> Result<String, String> {
        use std::sync::atomic::Ordering;
        let is_source = args.get(1).map(String::as_str) == Some("@DEFAULT_AUDIO_SOURCE@");
        let flag = if is_source { mic_flag } else { sink_flag };
        match args.first().map(String::as_str) {
            Some("set-mute") => {
                match args.get(2).map(String::as_str) {
                    Some("1") => flag.store(true, Ordering::SeqCst),
                    Some("0") => flag.store(false, Ordering::SeqCst),
                    Some("toggle") => {
                        let previous = flag.load(Ordering::SeqCst);
                        flag.store(!previous, Ordering::SeqCst);
                    }
                    _ => return Err("unexpected mode".into()),
                }
                Ok(String::new())
            }
            Some("get-volume") => Ok(if flag.load(Ordering::SeqCst) {
                "Volume: 0.50 [MUTED]\n".into()
            } else {
                "Volume: 0.50\n".into()
            }),
            Some("status") => {
                let current = sink_id.load(Ordering::SeqCst);
                let first = if current == 47 { "*" } else { " " };
                let second = if current == 51 { "*" } else { " " };
                Ok(format!(
                    "Audio\n ├─ Sinks:\n │  {first}  47. Built-in Speakers [vol: 0.50]\n \
                     │  {second}  51. Headphones [vol: 0.80]\n"
                ))
            }
            Some("set-default") => {
                let chosen: u32 = args.get(1).and_then(|value| value.parse().ok()).ok_or("bad id")?;
                sink_id.store(chosen, Ordering::SeqCst);
                Ok(String::new())
            }
            _ => Err("unexpected command".into()),
        }
    }

    fn wait_bar(compositor: &mut tehuti::state::Compositor, expected: &str) {
        use tehuti_protocol::Event;
        let deadline = std::time::Instant::now()
            .checked_add(Duration::from_secs(2))
            .unwrap();
        while std::time::Instant::now() < deadline {
            compositor.handle(&Event::Tick {
                elapsed: Duration::ZERO,
            });
            if compositor
                .voice_bar()
                .segments()
                .iter()
                .any(|segment| segment.text == expected)
            {
                return;
            }
            std::thread::yield_now();
        }
        let shown: Vec<_> = compositor
            .voice_bar()
            .segments()
            .into_iter()
            .map(|segment| segment.text)
            .collect();
        panic!("expected {expected:?} on bar, saw {shown:?}");
    }

    fn tap_guide_chord(compositor: &mut tehuti::state::Compositor, button: tehuti_protocol::ButtonName) {
        use tehuti_protocol::{ButtonName, ButtonState, Event};
        for (name, state) in [
            (ButtonName::Guide, ButtonState::Pressed),
            (button, ButtonState::Pressed),
            (button, ButtonState::Released),
            (ButtonName::Guide, ButtonState::Released),
        ] {
            compositor.handle(&Event::Button { name, state });
        }
    }

    #[test]
    fn mute_and_microphone_mute_work_by_voice_and_pad_toggle_on_the_bar() {
        use smithay::reexports::wayland_server::Display;
        use std::sync::atomic::Ordering;
        use tehuti::state::Compositor;
        use tehuti_protocol::{ButtonName, Event, Text};

        let (backend, audio) = fake_audio_backend();
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_volume_backend(backend);

        compositor.handle(&Event::Speech {
            text: Text::new("mute".into()).unwrap(),
        });
        wait_bar(&mut compositor, "Mute");
        assert!(
            audio.sink_muted.load(Ordering::SeqCst),
            "voice mute sets sink muted"
        );

        compositor.handle(&Event::Speech {
            text: Text::new("unmute".into()).unwrap(),
        });
        wait_bar(&mut compositor, "Unmute");
        assert!(
            !audio.sink_muted.load(Ordering::SeqCst),
            "voice unmute clears sink muted"
        );

        tap_guide_chord(&mut compositor, ButtonName::West);
        wait_bar(&mut compositor, "Mute");
        assert!(
            audio.sink_muted.load(Ordering::SeqCst),
            "pad chord toggles sink mute"
        );

        compositor.handle(&Event::Speech {
            text: Text::new("microphone mute".into()).unwrap(),
        });
        wait_bar(&mut compositor, "Microphone mute");
        assert!(
            audio.mic_muted.load(Ordering::SeqCst),
            "voice microphone mute sets source muted"
        );

        compositor.handle(&Event::Speech {
            text: Text::new("microphone on".into()).unwrap(),
        });
        wait_bar(&mut compositor, "Microphone on");
        assert!(
            !audio.mic_muted.load(Ordering::SeqCst),
            "voice microphone on clears source muted"
        );

        tap_guide_chord(&mut compositor, ButtonName::North);
        wait_bar(&mut compositor, "Microphone mute");
        assert!(
            audio.mic_muted.load(Ordering::SeqCst),
            "pad chord toggles mic mute"
        );

        let commands: Vec<_> = audio.received.try_iter().collect();
        for expected in [
            ["set-mute", "@DEFAULT_AUDIO_SINK@", "1"],
            ["set-mute", "@DEFAULT_AUDIO_SINK@", "0"],
            ["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"],
            ["set-mute", "@DEFAULT_AUDIO_SOURCE@", "1"],
            ["set-mute", "@DEFAULT_AUDIO_SOURCE@", "0"],
            ["set-mute", "@DEFAULT_AUDIO_SOURCE@", "toggle"],
        ] {
            assert!(
                commands.iter().any(|command| command == &expected),
                "missing wpctl call {expected:?}: {commands:?}"
            );
        }
    }

    #[test]
    fn output_cycle_on_pad_and_named_output_by_voice_show_voice_names() {
        use smithay::reexports::wayland_server::Display;
        use std::sync::atomic::Ordering;
        use tehuti::state::Compositor;
        use tehuti_protocol::{ButtonName, Event, Text};

        let (backend, audio) = fake_audio_backend();
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_volume_backend(backend);

        tap_guide_chord(&mut compositor, ButtonName::East);
        wait_bar(&mut compositor, "Speaker headphones");
        assert_eq!(audio.default_sink.load(Ordering::SeqCst), 51);

        compositor.handle(&Event::Speech {
            text: Text::new("speaker built in speakers".into()).unwrap(),
        });
        wait_bar(&mut compositor, "Speaker built in speakers");
        assert_eq!(audio.default_sink.load(Ordering::SeqCst), 47);

        let commands: Vec<_> = audio.received.try_iter().collect();
        assert!(
            commands.iter().any(|command| command == &["set-default", "51"]),
            "pad cycle must set default sink 51: {commands:?}"
        );
        assert!(
            commands.iter().any(|command| command == &["set-default", "47"]),
            "voice speaker must set default sink 47: {commands:?}"
        );
    }
}
