//! Session restore: capture, store and paced relaunch.
#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::time::Instant;

    use tehuti::session_restore::{
        MAX_FILE_BYTES, MAX_WINDOWS, Placements, Saved, Store, TOKEN_VARIABLE, parse_enabled, save_due,
        token_from_environment,
    };
    use tehuti_core::layout::{
        Command, Layout, LayoutConfig, OpenAt, OutputGeometry, OutputId, Rect, WindowId, WindowRules,
    };

    fn layout_with(windows: &[(u64, &str)], workspace_breaks: &[u64]) -> (Layout, Vec<(u64, String)>) {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        let mut layout = Layout::new(LayoutConfig::default());
        layout
            .add_output(OutputId(0), OutputGeometry { area, usable: area }, None)
            .expect("output");
        let mut names = Vec::new();
        for (identity, app_id) in windows {
            if workspace_breaks.contains(identity) {
                layout.apply(Command::FocusWorkspaceDown);
            }
            layout
                .add_window(WindowId(*identity), WindowRules::default())
                .expect("window");
            names.push((*identity, (*app_id).to_owned()));
        }
        (layout, names)
    }

    fn capture(layout: &Layout, names: &[(u64, String)], installed: &[&str]) -> Saved {
        let installed: Vec<String> = installed.iter().map(|entry| (*entry).to_owned()).collect();
        Saved::capture(
            &layout.snapshot(),
            |window| {
                names
                    .iter()
                    .find(|(identity, _)| *identity == window.0)
                    .map(|(_, name)| name.clone())
            },
            &installed,
        )
    }

    #[test]
    fn capture_records_app_workspace_and_column_in_layout_order() {
        let (layout, names) = layout_with(
            &[
                (1, "org.example.Alpha"),
                (2, "org.example.Beta"),
                (3, "org.example.Gamma"),
            ],
            &[3],
        );
        let saved = capture(
            &layout,
            &names,
            &["org.example.Alpha", "org.example.Beta", "org.example.Gamma"],
        );
        let places: Vec<_> = saved
            .windows()
            .iter()
            .map(|window| (window.app_id.as_str(), window.workspace, window.column))
            .collect();
        assert_eq!(
            places,
            vec![
                ("org.example.Alpha", 0, 0),
                ("org.example.Beta", 0, 1),
                ("org.example.Gamma", 1, 0)
            ]
        );
    }

    #[test]
    fn capture_skips_apps_without_an_installed_desktop_entry() {
        let (layout, names) = layout_with(&[(1, "org.example.Alpha"), (2, "unknown-class")], &[]);
        let saved = capture(&layout, &names, &["org.example.Alpha"]);
        assert_eq!(saved.windows().len(), 1);
    }

    #[test]
    fn text_round_trips() {
        let (layout, names) = layout_with(&[(1, "org.example.Alpha"), (2, "org.example.Alpha")], &[]);
        let saved = capture(&layout, &names, &["org.example.Alpha"]);
        assert_eq!(Saved::parse(&saved.to_text()).expect("parse"), saved);
    }

    #[test]
    fn garbage_does_not_parse() {
        for text in [
            "",
            "not json",
            "{\"version\":99,\"windows\":[]}",
            "{\"version\":1,\"windows\":[{\"app_id\":\"-rf\",\"workspace\":0,\"column\":0}]}",
        ] {
            assert!(Saved::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn store_takes_the_file_once_and_discards_a_bad_one() {
        let directory = tempfile::tempdir().expect("dir");
        let store = Store::new(directory.path().join("state/session.json"));
        assert!(store.take().expect("missing is fine").is_none());
        let (layout, names) = layout_with(&[(1, "org.example.Alpha")], &[]);
        let saved = capture(&layout, &names, &["org.example.Alpha"]);
        store.save(&saved).expect("save");
        assert_eq!(store.take().expect("take"), Some(saved));
        assert!(store.take().expect("taken").is_none());
        std::fs::write(directory.path().join("state/session.json"), "garbage").expect("write");
        assert!(store.take().is_err());
        assert!(store.take().expect("bad file removed").is_none());
    }

    fn two_of_one_and_one_other() -> Saved {
        let (layout, names) = layout_with(
            &[
                (1, "org.example.Alpha"),
                (2, "org.example.Alpha"),
                (3, "org.example.Beta"),
            ],
            &[],
        );
        capture(&layout, &names, &["org.example.Alpha", "org.example.Beta"])
    }

    #[test]
    fn a_window_claims_the_place_of_its_launch_token_once() {
        let now = Instant::now();
        let mut placements = Placements::new(&two_of_one_and_one_other(), now);
        let second = placements.claim(Some(1), now).expect("second");
        let first = placements.claim(Some(0), now).expect("first");
        assert_eq!((first.column, second.column), (0, 1));
        assert!(placements.claim(Some(1), now).is_none());
        assert!(placements.claim(Some(7), now).is_none());
    }

    #[test]
    fn a_hand_opened_window_cannot_take_a_saved_place() {
        let now = Instant::now();
        let mut placements = Placements::new(&two_of_one_and_one_other(), now);
        assert!(placements.claim(None, now).is_none());
        assert!(!placements.is_empty());
        assert!(placements.claim(Some(0), now).is_some());
    }

    #[test]
    fn the_launch_token_is_read_from_a_process_environment() {
        let token = format!("{}=2", TOKEN_VARIABLE);
        let environ = format!("HOME=/home/x\0{token}\0PATH=/bin\0");
        assert_eq!(token_from_environment(environ.as_bytes()), Some(2));
        assert_eq!(token_from_environment(b"HOME=/home/x\0"), None);
        let bad = format!("{TOKEN_VARIABLE}=abc\0");
        assert_eq!(token_from_environment(bad.as_bytes()), None);
        let lookalike = format!("X{TOKEN_VARIABLE}=1\0");
        assert_eq!(token_from_environment(lookalike.as_bytes()), None);
    }

    #[test]
    fn a_slow_launch_still_lands_in_its_saved_place() {
        let now = Instant::now();
        let mut placements = Placements::new(&two_of_one_and_one_other(), now);
        let late = now.checked_add(Duration::from_secs(30)).expect("time");
        assert!(placements.claim(Some(2), late).is_some());
    }

    #[test]
    fn unclaimed_places_expire() {
        let now = Instant::now();
        let mut placements = Placements::new(&two_of_one_and_one_other(), now);
        let late = now.checked_add(Duration::from_secs(3600)).expect("time");
        assert!(placements.claim(Some(2), late).is_none());
        assert!(placements.is_empty());
    }

    #[test]
    fn the_session_file_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().expect("dir");
        let path = directory.path().join("state/session.json");
        let store = Store::new(path.clone());
        let (layout, names) = layout_with(&[(1, "org.example.Alpha")], &[]);
        let saved = capture(&layout, &names, &["org.example.Alpha"]);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path.with_extension("tmp"), "stale").expect("stale");
        std::fs::set_permissions(path.with_extension("tmp"), std::fs::Permissions::from_mode(0o644))
            .expect("mode");
        store.save(&saved).expect("save");
        let mode = std::fs::metadata(&path).expect("file").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn an_empty_session_keeps_the_earlier_good_one() {
        let directory = tempfile::tempdir().expect("dir");
        let store = Store::new(directory.path().join("session.json"));
        let (layout, names) = layout_with(&[(1, "org.example.Alpha")], &[]);
        let good = capture(&layout, &names, &["org.example.Alpha"]);
        store.save(&good).expect("save");
        let empty = capture(&layout, &names, &[]);
        assert!(empty.windows().is_empty());
        store.save(&empty).expect("empty save is not an error");
        assert_eq!(store.take().expect("take"), Some(good));
    }

    #[test]
    fn placed_windows_land_in_their_place_and_leave_focus_alone() {
        let (mut layout, _) = layout_with(&[(1, "a"), (2, "b")], &[]);
        let focused = layout.snapshot().focused_window;
        let active = layout.snapshot().outputs[0].active_workspace;
        let place = |workspace, column| WindowRules {
            open_at: Some(OpenAt { workspace, column }),
            ..WindowRules::default()
        };
        layout.add_window(WindowId(10), place(0, 0)).expect("first");
        layout.add_window(WindowId(11), place(1, 0)).expect("second");
        let snapshot = layout.snapshot();
        assert_eq!(snapshot.focused_window, focused);
        assert_eq!(snapshot.outputs[0].active_workspace, active);
        let columns = |workspace: usize| -> Vec<u64> {
            snapshot.outputs[0].workspaces[workspace]
                .columns
                .iter()
                .map(|column| column.tiles[0].window.0)
                .collect()
        };
        assert_eq!(columns(0), vec![10, 1, 2]);
        assert!(columns(1).contains(&11));
    }

    #[test]
    fn the_file_is_capped_in_size_and_window_count() {
        let directory = tempfile::tempdir().expect("dir");
        let path = directory.path().join("session.json");
        let store = Store::new(path.clone());
        std::fs::write(
            &path,
            vec![b' '; usize::try_from(MAX_FILE_BYTES).expect("fits") + 1],
        )
        .expect("write");
        assert!(store.take().is_err());
        assert!(!path.exists());
        let many: Vec<String> = (0..=MAX_WINDOWS)
            .map(|index| {
                format!("{{\"app_id\":\"org.example.A{index}\",\"workspace\":0,\"column\":{index}}}")
            })
            .collect();
        let text = format!("{{\"version\":1,\"windows\":[{}]}}", many.join(","));
        assert!(Saved::parse(&text).is_err());
    }

    #[test]
    fn capture_stops_at_the_window_cap() {
        let count = u64::try_from(MAX_WINDOWS).expect("fits");
        let names: Vec<(u64, &str)> = (0..=count)
            .map(|identity| (identity, "org.example.Alpha"))
            .collect();
        let (layout, names) = layout_with(&names, &[]);
        assert_eq!(
            capture(&layout, &names, &["org.example.Alpha"]).windows().len(),
            MAX_WINDOWS
        );
    }

    #[test]
    fn saves_only_on_a_clean_quit_with_the_setting_on() {
        assert!(save_due(true, true, true));
        assert!(!save_due(false, true, true));
        assert!(!save_due(true, false, true));
        assert!(!save_due(true, true, false));
    }

    #[test]
    fn the_setting_defaults_to_off() {
        assert_eq!(parse_enabled(""), Ok(false));
        assert_eq!(parse_enabled("restore_session = true"), Ok(true));
        assert_eq!(parse_enabled("restore_session = false"), Ok(false));
        assert!(parse_enabled("restore_session = 3").is_err());
        assert!(parse_enabled("[[").is_err());
    }
}
