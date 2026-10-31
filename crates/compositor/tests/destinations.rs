//! Destination discovery through the shell's table boundary.
#[cfg(test)]
mod destinations {
    use tehuti::destinations::{Source, Table, Window};
    use tehuti_protocol::parse_context;

    #[test]
    fn desktop_captions_use_the_table_name_instead_of_a_legacy_launch_phrase() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("applications")).unwrap();
        std::fs::write(
            root.path().join("applications/reader.desktop"),
            "[Desktop Entry]\nType=Application\nName=Library\nExec=reader\n",
        )
        .unwrap();
        let contexts = vec![
            parse_context("app_id = '*'\n[buttons]\n[speech]\nRead = { launch = 'reader.desktop' }").unwrap(),
        ];
        let table = Table::read(
            root.path(),
            &root.path().join("projects"),
            &[root.path().into()],
            &contexts,
            &[],
        )
        .unwrap();
        assert_eq!(
            table.desktop_signposts(&contexts),
            [("reader.desktop".into(), "Library".into())]
        );
    }

    #[test]
    fn generated_destinations_keep_routes_sources_and_window_names() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("data/applications")).unwrap();
        std::fs::create_dir_all(root.path().join("projects/Orchid")).unwrap();
        std::fs::write(
            root.path().join("data/applications/reader.desktop"),
            "[Desktop Entry]\nType=Application\nName=Reader\nIcon=reader\nExec=reader\n",
        )
        .unwrap();
        let mut contexts = vec![
            parse_context(
                "app_id = '*'\n[buttons]\n[speech]\n'Work on <project>' = { text = 'cd -- {project}' }",
            )
            .unwrap(),
        ];
        tehuti::speech_vocabulary::configure_projects(&mut contexts, &root.path().join("projects"));
        let windows = vec![Window {
            identity: 7,
            app_id: "reader".into(),
            title: "Chapter one".into(),
            chip: "Books".into(),
        }];
        let table = Table::read(
            root.path(),
            &root.path().join("projects"),
            &[root.path().join("data")],
            &contexts,
            &windows,
        )
        .unwrap();
        let reader = table
            .entries()
            .iter()
            .find(|entry| entry.name == "Reader")
            .unwrap();
        assert_eq!(reader.source, Source::Desktop);
        assert_eq!(reader.routes[0].steps[0].icon, "reader");
        assert!(
            table
                .entries()
                .iter()
                .any(|entry| entry.name == "Orchid" && entry.source == Source::Project)
        );
        let window = table
            .entries()
            .iter()
            .find(|entry| entry.name == "Books")
            .unwrap();
        assert_eq!(window.source, Source::Window);
        assert_eq!(window.extra_names, ["Chapter one"]);
    }

    #[test]
    fn saved_window_survives_reload_and_custom_routes_replace_generated_names() {
        let root = tempfile::tempdir().unwrap();
        let window = Window {
            identity: 3,
            app_id: "reader".into(),
            title: "Reading".into(),
            chip: "Books".into(),
        };
        tehuti::destinations::save(root.path(), "Library", &window).unwrap();
        let table = Table::read(root.path(), root.path(), &[], &[], &[]).unwrap();
        let saved = table
            .entries()
            .iter()
            .find(|entry| entry.name == "Library")
            .unwrap();
        assert_eq!(saved.source, Source::Custom);
        assert_eq!(
            saved.routes[0].steps[0].target,
            tehuti::destinations::Target::Application("reader".into())
        );
        std::fs::write(
            root.path().join("destinations.toml"),
            r"
[[destinations]]
name = 'Books'
extra_names = ['Library']
[[destinations.routes]]
steps = [{ icon = 'reader', target = { kind = 'desktop', value = 'reader.desktop' } }]
[[destinations.routes]]
steps = [{ icon = 'web-browser', target = { kind = 'url', value = 'https://example.org' } }]
",
        )
        .unwrap();
        let replaced = Table::read(root.path(), root.path(), &[], &[], &[window]).unwrap();
        assert_eq!(replaced.entries().len(), 1);
        assert_eq!(replaced.entries()[0].routes.len(), 2);
        assert_eq!(replaced.entries()[0].extra_names, ["Library"]);
        assert!(replaced.vocabulary().contains(&"Library".to_owned()));
    }

    #[test]
    fn compositor_reloads_destinations_and_save_without_focus_reports_failure() {
        use smithay::reexports::wayland_server::Display;
        use tehuti::{destinations::Paths, state::Compositor};
        let root = tempfile::tempdir().unwrap();
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.configure_destinations(Paths {
            config: root.path().into(),
            projects: root.path().join("projects"),
            data: Vec::new(),
        });
        std::fs::write(root.path().join("destinations.toml"), "[[destinations]]\nname = 'Orchid'\nroutes = [{ steps = [{icon = 'web-browser', target = {kind = 'url', value = 'https://example.org'}}] }]").unwrap();
        compositor.reload_contexts(Ok(Vec::new()));
        assert!(
            compositor
                .destinations()
                .entries()
                .iter()
                .any(|entry| entry.name == "Orchid")
        );
        compositor.say(tehuti_protocol::Text::new("Save this as Library".into()).unwrap());
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("NO FOCUSED WINDOW")
        );
        std::fs::write(root.path().join("destinations.toml"), "invalid = [").unwrap();
        compositor.reload_contexts(Ok(Vec::new()));
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("DESTINATION CONFIG ERROR")
        );
        assert!(
            compositor
                .destinations()
                .entries()
                .iter()
                .any(|entry| entry.name == "Orchid")
        );
    }

    #[test]
    fn invalid_custom_routes_do_not_enter_the_table() {
        let root = tempfile::tempdir().unwrap();
        for target in [
            "{kind = 'desktop', value = ''}",
            "{kind = 'url', value = 'javascript:alert(1)'}",
            "{kind = 'application', value = ' '}",
        ] {
            std::fs::write(root.path().join("destinations.toml"), format!("[[destinations]]\nname = 'Library'\nroutes = [{{ steps = [{{icon = 'reader', target = {target}}}] }}]")).unwrap();
            assert!(Table::read(root.path(), root.path(), &[], &[], &[]).is_err());
        }
    }

    #[test]
    fn project_names_follow_the_context_that_offers_work_on() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("Orchid")).unwrap();
        let mut contexts = vec![parse_context("app_id = 'terminal'\n[buttons]\n[speech]\n'Work on <project>' = { text = 'cd -- {project}' }").unwrap()];
        tehuti::speech_vocabulary::configure_projects(&mut contexts, root.path());
        let table = Table::read(root.path(), root.path(), &[], &contexts, &[]).unwrap();
        assert_eq!(table.vocabulary_for(Some("terminal")), ["Orchid"]);
        assert_eq!(table.vocabulary_for(Some("reader")), Vec::<String>::new());
        assert_eq!(table.vocabulary_for(None), Vec::<String>::new());
    }
}
