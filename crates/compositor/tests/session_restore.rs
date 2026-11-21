//! Session restore: capture, store and paced relaunch.
#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::time::Instant;

    use tehuti::session_restore::{Restore, Saved, Store};
    use tehuti_core::layout::{
        Command, Layout, LayoutConfig, OutputGeometry, OutputId, Rect, WindowId, WindowRules,
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
    fn restore_launches_one_window_at_a_time_in_saved_order() {
        let mut restore = Restore::new(two_of_one_and_one_other());
        let start = Instant::now();
        let first = restore.next(start, &[]).expect("first");
        assert_eq!((first.app_id.as_str(), first.column), ("org.example.Alpha", 0));
        assert!(restore.next(start, &[]).is_none(), "waits for the first window");
        let open = vec!["org.example.Alpha".to_owned()];
        let second = restore.next(start, &open).expect("second");
        assert_eq!((second.app_id.as_str(), second.column), ("org.example.Alpha", 1));
    }

    #[test]
    fn restore_moves_on_when_a_window_never_appears() {
        let mut restore = Restore::new(two_of_one_and_one_other());
        let start = Instant::now();
        let _first = restore.next(start, &[]).expect("first");
        let later = start.checked_add(Duration::from_secs(60)).expect("time");
        assert!(restore.next(later, &[]).is_some());
    }

    #[test]
    fn restore_never_duplicates_windows_that_are_already_open() {
        let mut restore = Restore::new(two_of_one_and_one_other());
        let open = vec!["org.example.Alpha".to_owned(), "org.example.Alpha".to_owned()];
        let first = restore.next(Instant::now(), &open).expect("beta only");
        assert_eq!(first.app_id, "org.example.Beta");
    }
}
