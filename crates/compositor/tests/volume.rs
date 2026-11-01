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
        assert_eq!(after.len(), 400 * 104 * 4);
        assert_eq!(received.recv_timeout(Duration::from_secs(2)).unwrap()[2], "45%");
        compositor.handle(&Event::Button {
            name: ButtonName::East,
            state: ButtonState::Pressed,
        });
        assert!(compositor.adjuster().is_none());
        assert_eq!(received.recv_timeout(Duration::from_secs(2)).unwrap()[2], "40%");
    }
}
