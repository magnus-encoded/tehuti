//! The launcher table follows edits to the applications directories.

#[cfg(test)]
mod applications_watch {
    use core::time::Duration;
    use smithay::reexports::{calloop, wayland_server::Display};
    use std::time::Instant;
    use tehuti::{
        applications_watch::{self, ApplicationsWatch},
        contexts::DEBOUNCE,
        destinations::Paths,
        state::Compositor,
    };

    struct Looped {
        watch: Option<ApplicationsWatch>,
    }

    fn names(compositor: &Compositor) -> Vec<String> {
        compositor
            .destinations()
            .entries()
            .iter()
            .map(|entry| entry.name.clone())
            .collect()
    }

    fn changed_within(
        event_loop: &mut calloop::EventLoop<'static, Looped>,
        looped: &mut Looped,
        limit: Duration,
    ) -> bool {
        let start = Instant::now();
        let far = || Instant::now().checked_add(DEBOUNCE).unwrap();
        while start.elapsed() < limit {
            event_loop.dispatch(Duration::from_millis(20), looped).unwrap();
            if looped.watch.as_mut().unwrap().poll(far()) {
                return true;
            }
        }
        false
    }

    #[test]
    fn only_desktop_files_and_directories_count_as_changes() {
        assert!(applications_watch::relevant(
            Some(b"reader.desktop"),
            false,
            false
        ));
        assert!(applications_watch::relevant(Some(b"vendor"), true, false));
        assert!(applications_watch::relevant(None, false, true));
        assert!(!applications_watch::relevant(
            Some(b"mimeinfo.cache"),
            false,
            false
        ));
        assert!(!applications_watch::relevant(None, false, false));
    }

    #[test]
    fn a_desktop_file_added_after_start_ranks_without_another_trigger() {
        let root = tempfile::tempdir().unwrap();
        let applications = root.path().join("data/applications");
        std::fs::create_dir_all(applications.join("vendor")).unwrap();
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.configure_destinations(Paths {
            config: root.path().join("config"),
            projects: root.path().join("projects"),
            data: vec![root.path().join("data"), root.path().join("missing")],
        });
        assert!(!names(&compositor).contains(&"Reader".into()));
        let mut event_loop: calloop::EventLoop<'static, Looped> = calloop::EventLoop::try_new().unwrap();
        let watch = ApplicationsWatch::insert(
            &event_loop.handle(),
            &[root.path().join("data"), root.path().join("missing")],
            |looped: &mut Looped| looped.watch.as_mut(),
        )
        .unwrap();
        let mut looped = Looped { watch: Some(watch) };
        std::fs::write(applications.join("mimeinfo.cache"), "ignored").unwrap();
        assert!(
            !changed_within(&mut event_loop, &mut looped, Duration::from_millis(500)),
            "unrelated files do not rebuild"
        );
        std::fs::write(
            applications.join("vendor/reader.desktop"),
            "[Desktop Entry]\nType=Application\nName=Reader\nExec=reader\n",
        )
        .unwrap();
        assert!(changed_within(
            &mut event_loop,
            &mut looped,
            Duration::from_secs(5)
        ));
        compositor.refresh_destinations();
        assert!(names(&compositor).contains(&"Reader".into()));
    }
}
