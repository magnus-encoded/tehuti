#[cfg(test)]
mod hotwords {
    use tehuti_protocol::{Action, KeyName, Text, parse_context};

    #[test]
    fn hotwords_resolve_apps_paths_and_existing_speech_actions() {
        let context = parse_context(
            r#"
app_id = "*"
[buttons]
[speech]
confirm = { key = "enter" }
[hotwords]
apps = false
browser = { app = "firefox.desktop" }
atlas = { path = "~/Documents/atlas" }
accept = { action = "confirm" }
"#,
        )
        .unwrap();
        assert_eq!(context.app_hotwords(), Some(false));
        assert_eq!(
            context.hotwords().get("browser"),
            Some(&Action::Launch {
                desktop_entry: Text::new("firefox.desktop".into()).unwrap()
            })
        );
        assert_eq!(
            context.hotwords().get("atlas"),
            Some(&Action::OpenPath {
                path: Text::new("~/Documents/atlas".into()).unwrap()
            })
        );
        assert_eq!(
            context.hotwords().get("accept"),
            Some(&Action::Key { key: KeyName::Enter })
        );
    }

    #[test]
    fn parser_keeps_paths_literal() {
        for (source, expected) in [
            ("~", "~".into()),
            ("/tmp/atlas", "/tmp/atlas".into()),
            ("Documents/atlas", "Documents/atlas".into()),
        ] {
            let context = parse_context(&format!(
                "app_id = '*'\n[buttons]\n[hotwords]\natlas = {{ path = '{source}' }}"
            ))
            .unwrap();
            assert_eq!(
                context.hotwords().get("atlas"),
                Some(&Action::OpenPath {
                    path: Text::new(expected).unwrap()
                })
            );
        }
    }

    #[test]
    fn invalid_hotword_boundaries_are_rejected() {
        for entry in [
            "'!' = { path = '/tmp/atlas' }",
            "atlas = { app = '../atlas.desktop' }",
            "atlas = { app = 'atlas' }",
            "atlas = { path = '' }",
            "atlas = { action = 'missing' }",
            "atlas = { path = '/tmp/atlas', app = 'atlas.desktop' }",
            "atlas = false",
            "atlas = { url = '' }",
            "atlas = { url = 'relative/path' }",
            "atlas = { url = 'https://example.org/a b' }",
            "atlas = { recognition-only = false }",
            "atlas = { recognition-only = true, path = '/tmp/atlas' }",
            "apps = { app = 'atlas.desktop' }",
            "atlas = { path = '/tmp/a' }\n'ATLAS!' = { path = '/tmp/b' }",
        ] {
            assert!(
                parse_context(&format!("app_id = '*'\n[buttons]\n[hotwords]\n{entry}")).is_err(),
                "{entry}"
            );
        }
    }

    #[test]
    fn fixture_covers_every_hotword_target() {
        let context = parse_context(include_str!("fixtures/atlas/context.toml")).unwrap();
        assert_eq!(context.app_id(), "org.example.Atlas");
        assert_eq!(context.hotwords().len(), 5);
        assert_eq!(context.app_hotwords(), Some(false));
        assert_eq!(context.hotwords().get("quasar"), Some(&Action::RecognitionOnly));
        assert_eq!(
            context.hotwords().get("website"),
            Some(&Action::OpenUrl {
                url: Text::new("https://example.org/atlas".into()).unwrap()
            })
        );
        assert_eq!(
            context.hotwords().get("advance"),
            Some(&Action::Key { key: KeyName::Tab })
        );
    }
}
