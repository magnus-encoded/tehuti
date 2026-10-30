#[cfg(test)]
mod tests {
    use tehuti_core::launcher::{Destination, Launcher, Safety};

    #[test]
    fn sound_ranked_routes_can_be_corrected_and_confirmed() {
        let mut launcher = Launcher::new(
            "pokemon",
            vec![
                Destination {
                    name: "Other".into(),
                    route: "Other (app)".into(),
                    open: false,
                    focused: false,
                    safety: Safety::Launch,
                },
                Destination {
                    name: "Pokémon".into(),
                    route: "Browser → game.example".into(),
                    open: false,
                    focused: false,
                    safety: Safety::Launch,
                },
            ],
            None,
        );
        assert_eq!(
            launcher
                .list()
                .candidates()
                .first()
                .map(|row| row.candidate.name.as_str()),
            Some("Pokémon")
        );
        assert_eq!(launcher.confirm(), Some(1));
    }

    fn browser(safety: Safety) -> Destination {
        Destination {
            name: "Browser".into(),
            route: "Browser (app)".into(),
            open: false,
            focused: false,
            safety,
        }
    }

    #[test]
    fn only_heard_matched_safe_rows_expire_after_five_seconds() {
        let mut launcher = Launcher::new("browser", vec![browser(Safety::Launch)], Some(950));
        assert!(launcher.checks().heard && launcher.checks().matched && launcher.checks().safe);
        assert_eq!(launcher.tick(core::time::Duration::from_secs(4)), None);
        assert_eq!(launcher.tick(core::time::Duration::from_secs(1)), Some(0));
        let mut uncertain = Launcher::new("browser", vec![browser(Safety::Launch)], None);
        assert_eq!(uncertain.footer(), "Not sure what I heard");
        assert_eq!(uncertain.tick(core::time::Duration::from_secs(60)), None);
        let mut changes = Launcher::new("browser", vec![browser(Safety::Changes)], Some(950));
        assert!(!changes.checks().safe);
        assert_eq!(changes.tick(core::time::Duration::from_secs(60)), None);
    }

    #[test]
    fn movement_and_speech_stop_automatic_picking() {
        for speech in [false, true] {
            let mut launcher = Launcher::new("browser", vec![browser(Safety::Launch)], Some(950));
            if speech {
                launcher.stop_timer();
            } else {
                launcher.move_selection(true);
            }
            assert_eq!(launcher.tick(core::time::Duration::from_secs(60)), None);
        }
    }

    #[test]
    fn optional_jev_reply_reorders_rows_and_rejects_incomplete_or_late_scores() {
        let destinations = vec![
            browser(Safety::Launch),
            Destination {
                name: "Browsers".into(),
                ..browser(Safety::Launch)
            },
        ];
        let mut launcher = Launcher::new("browser", destinations, Some(950));
        assert!(launcher.jev_request(true, true, "app", Some("editor")).is_some());
        let fake_jev = |request: &tehuti_core::launcher::JevRequest| {
            assert_eq!(request.query, "browser");
            assert_eq!(request.level, "app");
            assert_eq!(request.focused_app.as_deref(), Some("editor"));
            vec![100, 980]
        };
        let request = launcher.jev_request(true, true, "app", Some("editor")).unwrap();
        launcher.jev_reply(&fake_jev(&request));
        assert_eq!(
            launcher
                .list()
                .candidates()
                .first()
                .map(|row| row.candidate.name.as_str()),
            Some("Browsers")
        );
        assert_eq!(launcher.confirm(), Some(1));
        let mut local = Launcher::new("browser", vec![browser(Safety::Launch)], None);
        assert!(local.jev_request(false, true, "app", None).is_none());
        assert!(local.jev_request(true, false, "app", None).is_none());
        assert!(local.jev_request(true, true, "app", None).is_some());
        let _picked = local.tick(core::time::Duration::from_millis(1501));
        local.jev_reply(&[0]);
        assert_eq!(
            local
                .list()
                .candidates()
                .first()
                .map(|row| row.candidate.name.as_str()),
            Some("Browser")
        );
    }

    #[test]
    fn timer_settings_disable_or_change_the_delay() {
        let mut launcher = Launcher::new("browser", vec![browser(Safety::Launch)], Some(950));
        launcher.configure(false, core::time::Duration::from_secs(1));
        assert_eq!(launcher.tick(core::time::Duration::from_secs(10)), None);
        let mut delayed = Launcher::new("browser", vec![browser(Safety::Launch)], Some(950));
        delayed.configure(true, core::time::Duration::from_secs(2));
        assert_eq!(delayed.tick(core::time::Duration::from_secs(2)), Some(0));
    }

    #[test]
    fn work_and_new_are_reserved_units() {
        use tehuti_core::unit_verbs::{DestinationKind, launcher_request};
        assert_eq!(
            launcher_request("work orchid").map(|request| request.kind),
            Some(DestinationKind::Project)
        );
        assert_eq!(
            launcher_request("new tab").map(|request| request.kind),
            None
        );
        assert_eq!(
            launcher_request("new").map(|request| request.query),
            Some(String::new())
        );
    }

    #[test]
    fn launcher_shows_its_verb_query_and_scope() {
        let mut launcher = Launcher::new("browser", vec![browser(Safety::Launch)], None);
        launcher.set_scope("Open", "Columns", Some("editor"));
        assert_eq!(launcher.list().slots().title, "Open browser");
        assert_eq!(launcher.list().slots().scopes, ["Columns · editor"]);
    }

    #[test]
    fn equal_matches_prefer_open_and_focused_routes_and_nothing_found_cannot_pick() {
        let mut launcher = Launcher::new(
            "browser",
            vec![
                browser(Safety::Launch),
                Destination {
                    open: true,
                    focused: true,
                    ..browser(Safety::Focus)
                },
            ],
            None,
        );
        assert_eq!(launcher.confirm(), Some(1));
        let mut empty = Launcher::new("unrelated", vec![browser(Safety::Launch)], Some(1000));
        assert_eq!(empty.footer(), "Nothing found");
        assert_eq!(empty.confirm(), None);
    }

    proptest::proptest! {
        #[test]
        fn manual_selection_never_auto_picks(moves in proptest::collection::vec(proptest::bool::ANY, 1..30), elapsed in 0u64..10000) {
            let mut launcher = Launcher::new("browser", vec![browser(Safety::Launch); 8], Some(1000));
            for down in moves { launcher.move_selection(down); }
            proptest::prop_assert_eq!(launcher.tick(core::time::Duration::from_millis(elapsed)), None);
            proptest::prop_assert!(launcher.list().candidates().len() <= 5);
            let picked = launcher.confirm();
            proptest::prop_assert!(picked.is_some_and(|index| index < 5));
        }
        #[test]
        fn changes_never_auto_pick(elapsed in 0u64..10000, confidence in 0u16..=1000) {
            let mut launcher = Launcher::new("browser", vec![browser(Safety::Changes)], Some(confidence));
            proptest::prop_assert_eq!(launcher.tick(core::time::Duration::from_millis(elapsed)), None);
        }
    }

    #[test]
    fn context_bonuses_do_not_make_a_weak_sound_match_sure() {
        let launcher = Launcher::new(
            "browser",
            vec![Destination {
                name: "Browner".into(),
                open: true,
                focused: true,
                ..browser(Safety::Focus)
            }],
            Some(1000),
        );
        assert!(!launcher.checks().matched);
    }
}
