#[cfg(test)]
mod hotwords {
    use tehuti::desktop::application_hotwords;
    use tehuti_protocol::{Action, Text};

    #[test]
    fn fixture_loads_through_the_context_directory() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../protocol/tests/fixtures");
        let contexts = tehuti::contexts::try_load(&directory).unwrap();
        assert_eq!(contexts.len(), 1);
        let context = contexts.first().unwrap();
        assert_eq!(context.app_id(), "org.example.Atlas");
        assert_eq!(context.hotwords().len(), 5);
        assert_eq!(
            context.hotwords().get("website"),
            Some(&Action::OpenUrl {
                url: Text::new("https://example.org/atlas".into()).unwrap()
            })
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
