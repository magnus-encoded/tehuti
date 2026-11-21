//! Session restore: capture, store and paced relaunch.
#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::time::Instant;

    use tehuti::session_restore::{
        MAX_FILE_BYTES, MAX_WINDOWS, Placements, Saved, Store, parse_enabled, save_due,
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
    fn a_window_claims_its_saved_place_once_in_saved_order() {
        let now = Instant::now();
        let mut placements = Placements::new(&two_of_one_and_one_other(), now);
        let first = placements.claim("org.example.Alpha", now).expect("first");
        let second = placements.claim("org.example.Alpha", now).expect("second");
        assert_eq!((first.column, second.column), (0, 1));
        assert!(placements.claim("org.example.Alpha", now).is_none());
        assert!(placements.claim("org.example.Other", now).is_none());
    }

    #[test]
    fn a_slow_launch_still_lands_in_its_saved_place() {
        let now = Instant::now();
        let mut placements = Placements::new(&two_of_one_and_one_other(), now);
        let late = now.checked_add(Duration::from_secs(30)).expect("time");
        assert!(placements.claim("org.example.Beta", late).is_some());
    }

    #[test]
    fn unclaimed_places_expire() {
        let now = Instant::now();
        let mut placements = Placements::new(&two_of_one_and_one_other(), now);
        let late = now.checked_add(Duration::from_secs(3600)).expect("time");
        assert!(placements.claim("org.example.Beta", late).is_none());
        assert!(placements.is_empty());
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
        let names: Vec<(u64, &str)> = (0..=MAX_WINDOWS as u64)
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
