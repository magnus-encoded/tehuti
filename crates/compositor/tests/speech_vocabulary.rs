//! Vocabulary discovery uses isolated files, without a model or microphone.
#[cfg(test)]
mod speech_vocabulary {
    use tehuti::speech_vocabulary;
    use tehuti_protocol::parse_context;

    #[test]
    fn vocabulary_contains_phrases_user_words_projects_skills_and_desktop_names() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join("vocabulary.txt"), "# names\nOrchid\n\nAtlas\nOrchid\n").unwrap();
        std::fs::create_dir_all(root.join("Projects/Bluebird")).unwrap();
        std::fs::write(root.join("Projects/not-a-project"), "").unwrap();
        std::fs::create_dir_all(root.join("skills/review-code")).unwrap();
        std::fs::create_dir_all(root.join("data/applications/tools")).unwrap();
        std::fs::write(root.join("data/applications/tools/browser.desktop"),
            "[Desktop Entry]\nType=Application\nName=Web Navigator\nGenericName=Web Browser\nName[de]=Ignored\n[Desktop Action New]\nName=Ignored Action\n").unwrap();
        let contexts = vec![parse_context("app_id = '*'\n[buttons]\n[speech]\n'Open browser' = { key = 'enter' }\n'Invoke review-code' = { text = '/review-code ' }").unwrap()];
        let words = speech_vocabulary::read(
            &contexts,
            root,
            &root.join("Projects"),
            &[root.join("skills")],
            &[root.join("data")],
        );
        for expected in [
            "Open browser",
            "Orchid",
            "Atlas",
            "Bluebird",
            "review-code",
            "Web Navigator",
            "Web Browser",
        ] {
            assert!(words.iter().any(|word| word == expected), "missing {expected}");
        }
        for excluded in ["not-a-project", "Ignored", "Ignored Action", "# names"] {
            assert!(
                !words.iter().any(|word| word == excluded),
                "unexpected {excluded}"
            );
        }
        assert_eq!(words.iter().filter(|word| *word == "Orchid").count(), 1);
        let empty = speech_vocabulary::read(&[], &root.join("missing"), &root.join("missing"), &[], &[]);
        assert_eq!(empty, Vec::<String>::new());
    }
    #[test]
    fn generic_project_command_resolves_folder_names_and_quotes_the_path() {
        use tehuti_protocol::{Action, Text};
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("Bluebird")).unwrap();
        std::fs::create_dir(root.join("builder's kit")).unwrap();
        let mut contexts = vec![parse_context("app_id = 'terminal'\n[buttons]\n[speech]\n'Work on <project>' = { text = 'cd -- {project}' }").unwrap()];
        speech_vocabulary::configure_projects(&mut contexts, root);
        assert_eq!(
            contexts[0].speech_action("Work on blue bird."),
            Some(Action::Text {
                text: Text::new(format!("cd -- '{}'", root.join("Bluebird").display())).unwrap(),
            })
        );
        assert_eq!(
            contexts[0].speech_action("Work on bluebrd."),
            Some(Action::Text {
                text: Text::new(format!("cd -- '{}'", root.join("Bluebird").display())).unwrap(),
            })
        );
        assert_eq!(
            contexts[0].speech_action("Work on builder's kit."),
            Some(Action::Text {
                text: Text::new(format!(
                    "cd -- '{}'",
                    root.join("builder's kit")
                        .display()
                        .to_string()
                        .replace('\'', "'\\''")
                ))
                .unwrap(),
            })
        );
        assert_eq!(contexts[0].speech_action("Work on missing."), None);
        std::fs::create_dir(root.join("Bluebord")).unwrap();
        speech_vocabulary::configure_projects(&mut contexts, root);
        assert_eq!(contexts[0].speech_action("Work on bluebrd."), None);
    }
}
