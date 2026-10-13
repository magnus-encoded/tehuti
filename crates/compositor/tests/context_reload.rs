//! Context reload swaps the whole set or keeps the old one.

#[cfg(test)]
mod context_reload {
    use core::time::Duration;
    use proptest::prelude::*;
    use smithay::reexports::wayland_server::Display;
    use std::time::Instant;
    use tehuti::{
        contexts::{self, DEBOUNCE, Debounce},
        state::Compositor,
    };

    fn write(directory: &std::path::Path, name: &str, source: &str) {
        std::fs::create_dir_all(directory.join(name)).unwrap();
        std::fs::write(directory.join(name).join("context.toml"), source).unwrap();
    }

    fn app_ids(compositor: &Compositor) -> Vec<String> {
        compositor
            .contexts()
            .iter()
            .map(|context| context.app_id().to_owned())
            .collect()
    }

    #[test]
    fn reload_swaps_mappings() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "one", "app_id = 'one'\n[buttons]");
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_contexts(contexts::load(directory.path()));
        write(directory.path(), "two", "app_id = 'two'\n[buttons]");
        compositor.reload_contexts(contexts::try_load(directory.path()));
        assert_eq!(app_ids(&compositor), ["one", "two"]);
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("CONTEXTS RELOADED")
        );
    }

    #[test]
    fn a_broken_file_keeps_the_old_set_and_names_the_file() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "one", "app_id = 'one'\n[buttons]");
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_contexts(contexts::load(directory.path()));
        write(directory.path(), "broken", "app_id = ");
        write(directory.path(), "two", "app_id = 'two'\n[buttons]");
        let reload = contexts::try_load(directory.path());
        let error = reload.clone().unwrap_err();
        assert_eq!(error.path, directory.path().join("broken/context.toml"));
        compositor.reload_contexts(reload);
        assert_eq!(app_ids(&compositor), ["one"]);
        let notice = compositor.visible_notice_text().unwrap();
        assert!(notice.starts_with("CONTEXT ERROR broken/"), "{notice}");
        assert!(notice.lines().all(|line| line.chars().count() <= 40), "{notice}");
    }

    #[test]
    fn startup_load_still_skips_broken_files() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "broken", "app_id = ");
        write(directory.path(), "one", "app_id = 'one'\n[buttons]");
        assert_eq!(contexts::load(directory.path()).len(), 1);
    }

    #[test]
    fn an_idle_debounce_is_never_due() {
        assert!(!Debounce::default().due(Instant::now()));
    }

    proptest! {
        #[test]
        fn debounce_coalesces_a_burst_into_one_reload(
            gaps in prop::collection::vec(0..300_u64, 1..30),
            checks in prop::collection::vec(0..300_u64, 0..30),
        ) {
            let start = Instant::now();
            let mut debounce = Debounce::default();
            let mut now = start;
            let mut reloaded = false;
            for (index, gap) in gaps.iter().enumerate() {
                now = now.checked_add(Duration::from_millis(*gap)).unwrap();
                debounce.changed(now);
                if let Some(check) = checks.get(index) {
                    let probe = now.checked_add(Duration::from_millis(*check).min(DEBOUNCE.saturating_sub(Duration::from_millis(1)))).unwrap();
                    reloaded |= debounce.due(probe);
                }
            }
            prop_assert!(!reloaded, "no reload while edits keep arriving");
            let quiet = now.checked_add(DEBOUNCE).unwrap();
            prop_assert!(debounce.due(quiet));
            prop_assert!(!debounce.due(quiet.checked_add(DEBOUNCE).unwrap()), "one reload per burst");
        }
    }
}
