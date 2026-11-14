//! Context reload swaps the whole set or keeps the old one.

#[cfg(test)]
mod context_reload {
    use core::time::Duration;
    use proptest::prelude::*;
    use smithay::reexports::{calloop, wayland_server::Display};
    use std::time::Instant;
    use tehuti::{
        contexts::{self, DEBOUNCE, Debounce},
        state::Compositor,
    };

    fn write(directory: &std::path::Path, name: &str, source: &str) {
        std::fs::create_dir_all(directory.join(name)).unwrap();
        std::fs::write(directory.join(name).join("context.toml"), source).unwrap();
    }

    fn app_ids_of(contexts: &[tehuti_protocol::Context]) -> Vec<String> {
        contexts
            .iter()
            .map(|context| context.app_id().to_owned())
            .collect()
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
    fn a_broken_file_is_skipped_and_named_while_the_rest_reload() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "one", "app_id = 'one'\n[buttons]");
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_contexts(contexts::load(directory.path()));
        write(directory.path(), "broken", "app_id = ");
        write(directory.path(), "two", "app_id = 'two'\n[buttons]");
        let reloaded = contexts::reload(directory.path()).unwrap();
        assert_eq!(reloaded.skipped.len(), 1);
        assert_eq!(
            reloaded.skipped[0].path,
            directory.path().join("broken/context.toml")
        );
        compositor.apply_reload(Ok(reloaded));
        assert_eq!(app_ids(&compositor), ["one", "two"]);
        let notice = compositor.visible_notice_text().unwrap();
        assert!(notice.starts_with("CONTEXT ERROR broken/"), "{notice}");
        assert!(notice.lines().all(|line| line.chars().count() <= 40), "{notice}");
    }

    #[test]
    fn a_broken_file_keeps_its_last_good_version() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "one", "app_id = 'one'\n[buttons]");
        write(directory.path(), "two", "app_id = 'two'\n[buttons]");
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_context_files(contexts::load_files(directory.path()));
        write(directory.path(), "one", "app_id = ");
        compositor.apply_reload(contexts::reload(directory.path()));
        assert_eq!(app_ids(&compositor), ["one", "two"]);
        let notice = compositor.visible_notice_text().unwrap();
        assert!(notice.starts_with("CONTEXT ERROR one/"), "{notice}");
        write(directory.path(), "one", "app_id = 'uno'\n[buttons]");
        compositor.apply_reload(contexts::reload(directory.path()));
        assert_eq!(app_ids(&compositor), ["uno", "two"], "the fix is picked up");
    }

    #[test]
    fn a_file_broken_twice_still_keeps_the_last_good_version() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "one", "app_id = 'one'\n[buttons]");
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_context_files(contexts::load_files(directory.path()));
        for broken in ["app_id = ", "app_id = 'x'\n[buttons.nope]"] {
            write(directory.path(), "one", broken);
            compositor.apply_reload(contexts::reload(directory.path()));
            assert_eq!(app_ids(&compositor), ["one"]);
        }
    }

    #[test]
    fn a_deleted_file_is_dropped_on_reload() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "one", "app_id = 'one'\n[buttons]");
        write(directory.path(), "two", "app_id = 'two'\n[buttons]");
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_context_files(contexts::load_files(directory.path()));
        std::fs::remove_dir_all(directory.path().join("one")).unwrap();
        compositor.apply_reload(contexts::reload(directory.path()));
        assert_eq!(app_ids(&compositor), ["two"]);
    }

    #[test]
    fn a_skipped_phrase_is_reported_by_file_and_phrase_while_the_rest_loads() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "one",
            "app_id = 'one'\n[buttons]\n[speech]\n'Open Ibis.' = { key = 'enter' }\n'Open IBIS.' = { key = 'tab' }\n'Go' = { key = 'space' }",
        );
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let reloaded = contexts::reload(directory.path()).unwrap();
        assert_eq!(reloaded.phrase_warnings.len(), 1);
        assert_eq!(
            reloaded.phrase_warnings[0].path,
            directory.path().join("one/context.toml")
        );
        compositor.apply_reload(Ok(reloaded));
        assert!(compositor.contexts()[0].speech_action("Go").is_some());
        let notice = compositor.visible_notice_text().unwrap();
        assert!(notice.contains("one/context.toml"), "{notice}");
        assert!(notice.contains("Open IBIS"), "{notice}");
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
    fn unreadable_directories_fail_the_reload() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "one", "app_id = 'one'\n[buttons]");
        std::os::unix::fs::symlink("loop", directory.path().join("loop")).unwrap();
        let error = contexts::try_load(directory.path()).unwrap_err();
        assert_eq!(error.path, directory.path().join("loop"));
        assert!(contexts::try_load(&directory.path().join("missing")).is_err());
        assert_eq!(
            app_ids_of(&contexts::load(directory.path())),
            ["one"],
            "startup skips the bad entry and keeps valid contexts"
        );
    }

    #[test]
    fn toml_errors_keep_location_and_message() {
        let error = tehuti_protocol::parse_context("app_id = 'one'\n[buttons]\nsouth = { key = 'nope' }")
            .unwrap_err();
        let short = contexts::short_error(&error);
        assert!(short.starts_with("L3 C"), "{short} from {error}");
        assert!(short.len() > "L3 C9 ".len(), "{short} from {error}");
        assert!(short.chars().count() <= 40);
        assert_eq!(contexts::short_error("plain failure\nmore"), "plain failure");
    }

    #[test]
    fn context_files_vocabulary_and_top_level_directories_are_relevant() {
        assert!(contexts::relevant(Some(b"context.toml"), false, true, false));
        assert!(contexts::relevant(Some(b"context.toml"), false, false, false));
        assert!(contexts::relevant(Some(b"vocabulary.txt"), false, true, false));
        assert!(!contexts::relevant(Some(b"vocabulary.txt"), false, false, false));
        assert!(contexts::relevant(Some(b"foot"), true, true, false));
        assert!(!contexts::relevant(Some(b"foot"), true, false, false));
        assert!(!contexts::relevant(
            Some(b"context.toml.swp"),
            false,
            false,
            false
        ));
        assert!(!contexts::relevant(Some(b"notes.txt"), false, true, false));
        assert!(!contexts::relevant(None, true, true, false));
        assert!(
            contexts::relevant(None, false, false, true),
            "an overflow reloads everything"
        );
    }

    struct Looped {
        watch: Option<contexts::ContextWatch>,
    }

    fn dispatch_until(
        event_loop: &mut calloop::EventLoop<'static, Looped>,
        looped: &mut Looped,
        limit: Duration,
        mut done: impl FnMut(&mut Looped) -> bool,
    ) -> bool {
        let start = Instant::now();
        while start.elapsed() < limit {
            event_loop.dispatch(Duration::from_millis(20), looped).unwrap();
            if done(looped) {
                return true;
            }
        }
        false
    }

    #[test]
    fn startup_raises_the_phrase_skip_notice_and_folds_do_not() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "one",
            "app_id = 'one'\n[buttons]\n[speech]\n'Ab part' = { key = 'enter' }\n'Abpart' = { key = 'tab' }",
        );
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_context_files(contexts::load_files(directory.path()));
        assert_eq!(
            compositor.visible_notice_text(),
            None,
            "a fold warning is not a skip"
        );
        write(
            directory.path(),
            "one",
            "app_id = 'one'\n[buttons]\n[speech]\n'Go' = { key = 'enter' }\n'go.' = { key = 'tab' }",
        );
        compositor.set_context_files(contexts::load_files(directory.path()));
        let notice = compositor.visible_notice_text().unwrap();
        assert!(notice.starts_with("PHRASE SKIPPED one/"), "{notice}");
    }

    #[test]
    fn a_fix_saved_after_a_skipped_load_is_picked_up_by_the_watcher() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "one", "app_id = 'one'\n[buttons]");
        let mut event_loop: calloop::EventLoop<'static, Looped> = calloop::EventLoop::try_new().unwrap();
        let watch =
            contexts::ContextWatch::insert(&event_loop.handle(), directory.path(), |looped: &mut Looped| {
                looped.watch.as_mut()
            })
            .unwrap();
        let mut looped = Looped { watch: Some(watch) };
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_context_files(contexts::load_files(directory.path()));
        let far = || Instant::now().checked_add(DEBOUNCE).unwrap();
        for (source, expected) in [("app_id = ", "one"), ("app_id = 'fixed'\n[buttons]", "fixed")] {
            write(directory.path(), "one", source);
            let mut reloaded = None;
            assert!(dispatch_until(
                &mut event_loop,
                &mut looped,
                Duration::from_secs(5),
                |looped| {
                    reloaded = looped.watch.as_mut().unwrap().poll(far());
                    reloaded.is_some()
                }
            ));
            compositor.apply_reload(reloaded.unwrap());
            assert_eq!(app_ids(&compositor), [expected]);
        }
    }

    #[test]
    fn a_file_write_reloads_through_the_event_loop() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "one", "app_id = 'one'\n[buttons]");
        let mut event_loop: calloop::EventLoop<'static, Looped> = calloop::EventLoop::try_new().unwrap();
        let watch =
            contexts::ContextWatch::insert(&event_loop.handle(), directory.path(), |looped: &mut Looped| {
                looped.watch.as_mut()
            })
            .unwrap();
        let mut looped = Looped { watch: Some(watch) };
        std::fs::write(directory.path().join("notes.txt"), "ignored").unwrap();
        let far = || Instant::now().checked_add(DEBOUNCE).unwrap();
        assert!(
            !dispatch_until(
                &mut event_loop,
                &mut looped,
                Duration::from_millis(500),
                |looped| { looped.watch.as_mut().unwrap().poll(far()).is_some() }
            ),
            "unrelated files do not reload"
        );
        write(directory.path(), "one", "app_id = 'renamed'\n[buttons]");
        let mut reloaded = None;
        assert!(dispatch_until(
            &mut event_loop,
            &mut looped,
            Duration::from_secs(5),
            |looped| {
                reloaded = looped.watch.as_mut().unwrap().poll(far());
                reloaded.is_some()
            }
        ));
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.apply_reload(reloaded.unwrap());
        assert_eq!(app_ids(&compositor), ["renamed"]);
    }

    #[test]
    fn a_speech_session_survives_a_reload() {
        use tehuti_core::speech::SpeechInput;
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "one", "app_id = 'one'\n[buttons]");
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_contexts(contexts::load(directory.path()));
        compositor.speech_input(SpeechInput::Available);
        compositor.speech_input(SpeechInput::Press);
        compositor.reload_contexts(contexts::try_load(directory.path()));
        compositor.speech_input(SpeechInput::Release);
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("CONTEXTS RELOADED")
        );
        compositor.speech_input(SpeechInput::Transcript {
            session: 0,
            text: tehuti_protocol::Text::new("unmapped".into()).unwrap(),
        });
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("NO SPEECH MAPPING"),
            "the transcript for the session started before the reload is delivered"
        );
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
