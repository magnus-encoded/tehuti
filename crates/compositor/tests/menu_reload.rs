//! Editing menu.toml reaches the running system menu without a restart.

#[cfg(test)]
mod menu_reload {
    use core::time::Duration;
    use smithay::reexports::{calloop, wayland_server::Display};
    use std::time::Instant;
    use tehuti::{
        contexts::DEBOUNCE,
        menu_watch::MenuWatch,
        state::Compositor,
        system_menu::{self, Config},
    };
    use tehuti_core::menu::Command;

    struct Looped {
        watch: Option<MenuWatch>,
    }

    fn compositor() -> (Display<Compositor>, Compositor) {
        let display: Display<Compositor> = Display::new().unwrap();
        let compositor = Compositor::new(&display.handle()).unwrap();
        (display, compositor)
    }

    fn entry(label: &str) -> String {
        format!("[[entry]]\nlabel = \"{label}\"\norder = 0\nexec = [\"example-tool\"]\n")
    }

    fn program(compositor: &Compositor, command: Command) -> Option<String> {
        let mut found = None;
        let _result = compositor.menu_config().run(command, |program, _| {
            found = Some(program.to_owned());
            Ok(())
        });
        found
    }

    #[test]
    fn a_reloaded_config_replaces_the_menu_entries() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("menu.toml");
        std::fs::write(&path, entry("Alpha")).unwrap();
        let (_display, mut compositor) = compositor();
        compositor.reload_menu_config(system_menu::load_from(&path));
        assert_eq!(compositor.menu_config().labels(), ["Alpha"]);
        std::fs::write(&path, entry("Beta")).unwrap();
        compositor.reload_menu_config(system_menu::load_from(&path));
        assert_eq!(compositor.menu_config().labels(), ["Beta"]);
    }

    #[test]
    fn a_reloaded_command_replaces_the_old_one() {
        let (_display, mut compositor) = compositor();
        let config = Config::parse("task_monitor = [\"example-monitor\"]").unwrap();
        compositor.reload_menu_config(Ok(config));
        assert_eq!(
            program(&compositor, Command::TaskMonitor).as_deref(),
            Some("example-monitor")
        );
    }

    #[test]
    fn an_invalid_file_keeps_the_old_menu_and_raises_a_notice() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("menu.toml");
        std::fs::write(&path, entry("Alpha")).unwrap();
        let (_display, mut compositor) = compositor();
        compositor.reload_menu_config(system_menu::load_from(&path));
        std::fs::write(&path, "invalid TOML").unwrap();
        compositor.reload_menu_config(system_menu::load_from(&path));
        assert_eq!(compositor.menu_config().labels(), ["Alpha"]);
        assert!(
            compositor
                .visible_notice_text()
                .unwrap()
                .starts_with("MENU CONFIG")
        );
    }

    #[test]
    fn a_skipped_entry_in_a_reloaded_file_raises_its_notice() {
        let (_display, mut compositor) = compositor();
        let config = Config::parse("bogus = [\"x\"]").unwrap();
        compositor.reload_menu_config(Ok(config));
        assert!(compositor.visible_notice_text().unwrap().contains("bogus"));
    }

    #[test]
    fn writing_the_file_wakes_the_watch_but_other_files_do_not() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("menu.toml");
        let mut event_loop: calloop::EventLoop<'static, Looped> = calloop::EventLoop::try_new().unwrap();
        let watch = MenuWatch::insert(&event_loop.handle(), &path, |looped: &mut Looped| {
            looped.watch.as_mut()
        })
        .unwrap();
        let mut looped = Looped { watch: Some(watch) };
        let far = || Instant::now().checked_add(DEBOUNCE).unwrap();
        let mut dispatch_until = |limit: Duration| {
            let start = Instant::now();
            while start.elapsed() < limit {
                event_loop
                    .dispatch(Duration::from_millis(20), &mut looped)
                    .unwrap();
                if looped.watch.as_mut().unwrap().poll(far()) {
                    return true;
                }
            }
            false
        };
        std::fs::write(directory.path().join("notes.txt"), "ignored").unwrap();
        assert!(!dispatch_until(Duration::from_millis(500)));
        std::fs::write(&path, entry("Alpha")).unwrap();
        assert!(dispatch_until(Duration::from_secs(5)));
    }
}
