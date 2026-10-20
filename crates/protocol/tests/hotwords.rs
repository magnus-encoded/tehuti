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
                path: Text::new(format!("{}/Documents/atlas", std::env::var("HOME").unwrap())).unwrap()
            })
        );
        assert_eq!(
            context.hotwords().get("accept"),
            Some(&Action::Key { key: KeyName::Enter })
        );
    }

    #[test]
    fn home_alone_expands_and_other_paths_remain_literal() {
        for (source, expected) in [
            ("~", std::env::var("HOME").unwrap()),
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
            "'two words' = { path = '/tmp/atlas' }",
            "'!' = { path = '/tmp/atlas' }",
            "atlas = { app = '../atlas.desktop' }",
            "atlas = { app = 'atlas' }",
            "atlas = { path = '' }",
            "atlas = { action = 'missing' }",
            "atlas = { path = '/tmp/atlas', app = 'atlas.desktop' }",
            "atlas = false",
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
    fn generator_output_parses_with_the_rust_loader() {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/new-context.py");
        let output = std::process::Command::new("python3")
            .arg(script)
            .args([
                "--name",
                "atlas",
                "--app-id",
                "org.example.Atlas",
                "--no-apps",
                "--hotword",
                "browser=app:firefox.desktop",
                "--hotword",
                "atlas=path:~/Documents/atlas",
                "--dry-run",
            ])
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let context = parse_context(&String::from_utf8(output.stdout).unwrap()).unwrap();
        assert_eq!(context.app_id(), "org.example.Atlas");
        assert_eq!(context.hotwords().len(), 2);
        assert_eq!(context.app_hotwords(), Some(false));
    }
}
