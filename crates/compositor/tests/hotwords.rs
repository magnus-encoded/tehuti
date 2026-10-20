#[cfg(test)]
mod hotwords {
    use tehuti::desktop::application_hotwords;
    use tehuti_protocol::{Action, Text};

    #[test]
    fn generator_lists_generic_names_and_skips_hidden_desktop_entries() {
        let directory = tempfile::tempdir().unwrap();
        let applications = directory.path().join("applications");
        std::fs::create_dir_all(&applications).unwrap();
        for (entry_id, source) in [
            ("atlas.desktop", "Name=Atlas\nGenericName=Document Viewer\n"),
            ("helper.desktop", "Name=Helper\nNoDisplay=true\n"),
            ("hidden.desktop", "Name=Hidden\nHidden=true\n"),
        ] {
            std::fs::write(applications.join(entry_id), format!("[Desktop Entry]\n{source}")).unwrap();
        }
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/new-context.py");
        let output = std::process::Command::new("python3")
            .args([
                "-c",
                "import runpy,sys; print(runpy.run_path(sys.argv[1])['installed_apps']())",
            ])
            .arg(script)
            .env("XDG_DATA_HOME", directory.path())
            .env("XDG_DATA_DIRS", directory.path().join("empty"))
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "{'atlas.desktop': 'Atlas / Document Viewer'}\n"
        );
    }

    #[test]
    fn generator_writes_a_loadable_context_without_overwriting_and_dry_run_writes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/new-context.py");
        let generate = |dry_run: bool| {
            let mut command = std::process::Command::new("python3");
            let _configured = command
                .arg(&script)
                .args([
                    "--name",
                    "atlas",
                    "--app-id",
                    "org.example.Atlas",
                    "--hotword",
                    "atlas=path:~/Documents/atlas",
                ])
                .env("TEHUTI_CONTEXT_DIRECTORY", directory.path())
                .stdin(std::process::Stdio::null());
            if dry_run {
                let _preview = command.arg("--dry-run");
            }
            command.output().unwrap()
        };
        let preview = generate(true);
        assert!(preview.status.success());
        assert!(!directory.path().join("atlas").exists());
        assert!(generate(false).status.success());
        let contexts = tehuti::contexts::try_load(directory.path()).unwrap();
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts.first().unwrap().app_id(), "org.example.Atlas");
        assert!(!generate(false).status.success());
        assert_eq!(
            std::fs::read(directory.path().join("atlas/context.toml")).unwrap(),
            preview.stdout
        );
    }

    #[test]
    fn desktop_names_and_generic_names_follow_xdg_priority_and_hidden_masks() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user");
        let system = directory.path().join("system");
        std::fs::create_dir_all(user.join("applications")).unwrap();
        std::fs::create_dir_all(system.join("applications/vendor")).unwrap();
        for (root, entry_id, contents) in [
            (
                &user,
                "atlas.desktop",
                "[Desktop Entry]\nName=Atlas\nGenericName=Document Viewer\n",
            ),
            (&system, "atlas.desktop", "[Desktop Entry]\nName=Wrong Name\n"),
            (
                &user,
                "hidden.desktop",
                "[Desktop Entry]\nHidden=true\nName=Hidden\n",
            ),
            (&system, "hidden.desktop", "[Desktop Entry]\nName=Masked\n"),
            (
                &user,
                "helper.desktop",
                "[Desktop Entry]\nNoDisplay=true\nName=Helper\n",
            ),
            (
                &system,
                "vendor/editor.desktop",
                "[Desktop Entry]\nName=Editor\n[Other]\nName=Ignored\n",
            ),
        ] {
            std::fs::write(root.join("applications").join(entry_id), contents).unwrap();
        }
        let words = application_hotwords(&[user, system]);
        assert_eq!(
            words,
            vec![
                (
                    "Atlas".into(),
                    Action::Launch {
                        desktop_entry: Text::new("atlas.desktop".into()).unwrap()
                    }
                ),
                (
                    "Document Viewer".into(),
                    Action::Launch {
                        desktop_entry: Text::new("atlas.desktop".into()).unwrap()
                    }
                ),
                (
                    "Editor".into(),
                    Action::Launch {
                        desktop_entry: Text::new("vendor-editor.desktop".into()).unwrap()
                    }
                ),
            ]
        );
    }
}
