//! Contexts reuse sections from `shared/<name>.toml` through `include = ["<name>"]`.

#[cfg(test)]
mod shared_sections {
    use std::{path::Path, process::Command};

    use tehuti::{config_check::check, contexts};
    use tehuti_protocol::{ButtonName, KeyName};

    const TABS: &str = "[buttons.left_bumper]\nkey = 'tab'\n";

    fn write(directory: &Path, relative: &str, text: &str) {
        let path = directory.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn app(name: &str, extra: &str) -> String {
        format!("app_id = '{name}'\n{extra}")
    }

    fn find<'a>(loaded: &'a [tehuti_protocol::Context], name: &str) -> Option<&'a tehuti_protocol::Context> {
        loaded.iter().find(|context| context.app_id() == name)
    }

    fn share(directory: &Path, arguments: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_tehuti"))
            .arg("config")
            .arg("share")
            .args(arguments)
            .env("XDG_CONFIG_HOME", directory.parent().unwrap())
            .env("HOME", directory)
            .output()
            .unwrap()
    }

    #[test]
    fn an_included_section_applies_only_to_the_context_that_includes_it() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "shared/tabs.toml", TABS);
        write(
            directory.path(),
            "kitty/context.toml",
            &app("kitty", "include = ['tabs']\n"),
        );
        write(
            directory.path(),
            "other/context.toml",
            &app("other", "[buttons.east]\nkey = 'enter'\n"),
        );
        write(
            directory.path(),
            "global/context.toml",
            &app("*", "[buttons.east]\nkey = 'enter'\n"),
        );
        let loaded = contexts::load(directory.path());
        assert_eq!(loaded.len(), 3);
        assert_eq!(
            find(&loaded, "kitty").unwrap().key_for(ButtonName::LeftBumper),
            Some(KeyName::Tab)
        );
        assert_eq!(
            find(&loaded, "other").unwrap().key_for(ButtonName::LeftBumper),
            None
        );
        assert_eq!(find(&loaded, "*").unwrap().key_for(ButtonName::LeftBumper), None);
    }

    #[test]
    fn a_context_entry_merges_over_the_shared_one_field_by_field() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "shared/tabs.toml",
            "[buttons.left_bumper]\nkey = 'tab'\nmods = ['shift']\n",
        );
        write(
            directory.path(),
            "kitty/context.toml",
            &app(
                "kitty",
                "include = ['tabs']\n[buttons.left_bumper]\nkey = 'escape'\n",
            ),
        );
        let loaded = contexts::load(directory.path());
        let expected = tehuti_protocol::parse_context(&app(
            "expected",
            "[buttons.left_bumper]\nkey = 'escape'\nmods = ['shift']\n",
        ))
        .unwrap();
        assert_eq!(
            find(&loaded, "kitty").unwrap().action_for(ButtonName::LeftBumper),
            expected.action_for(ButtonName::LeftBumper)
        );
        let report = check(directory.path());
        assert!(!report.has_errors(), "{report}");
        let text = report.to_string();
        assert!(
            text.contains("warning") && text.contains("buttons.left_bumper.key") && text.contains("tabs"),
            "{text}"
        );
        assert!(!text.contains("buttons.left_bumper.mods"), "{text}");
    }

    #[test]
    fn an_include_naming_a_missing_section_is_a_config_error_that_lists_existing_ones() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "shared/tabs.toml", TABS);
        write(
            directory.path(),
            "kitty/context.toml",
            &app("kitty", "include = ['missing']\n"),
        );
        assert_eq!(contexts::load(directory.path()).len(), 0);
        let report = check(directory.path());
        assert!(report.has_errors(), "{report}");
        let text = report.to_string();
        assert!(text.contains("missing"), "{text}");
        assert!(text.contains("shared sections: tabs"), "{text}");
    }

    #[test]
    fn shared_sections_cannot_include_other_shared_sections() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "shared/inner.toml", TABS);
        write(directory.path(), "shared/outer.toml", "include = ['inner']\n");
        write(
            directory.path(),
            "kitty/context.toml",
            &app("kitty", "include = ['outer']\n"),
        );
        assert_eq!(contexts::load(directory.path()).len(), 0);
        let report = check(directory.path());
        assert!(report.has_errors(), "{report}");
        assert!(report.to_string().contains("outer"), "{report}");
    }

    #[test]
    fn shared_names_may_hold_one_folder_level_and_context_is_reserved() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "shared/tabs/ctrl-tab.toml", TABS);
        write(
            directory.path(),
            "kitty/context.toml",
            &app("kitty", "include = ['tabs/ctrl-tab']\n"),
        );
        let loaded = contexts::load(directory.path());
        assert_eq!(
            find(&loaded, "kitty").unwrap().key_for(ButtonName::LeftBumper),
            Some(KeyName::Tab)
        );
        let report = check(directory.path());
        assert!(!report.has_errors(), "{report}");
        assert!(
            report.to_string().contains("shared sections: tabs/ctrl-tab"),
            "{report}"
        );
        for name in ["a/b/c", "context", "tabs/context"] {
            write(
                directory.path(),
                "other/context.toml",
                &app("other", &format!("include = ['{name}']\n")),
            );
            assert!(check(directory.path()).has_errors(), "{name}");
        }
    }

    #[test]
    fn share_takes_several_paths_into_one_file_and_requires_as() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("tehuti");
        let original = app(
            "kitty",
            "[buttons.left_bumper]\nkey = 'tab'\n[buttons.right_bumper]\nkey = 'enter'\n",
        );
        write(&directory, "kitty/context.toml", &original);
        let without = share(
            &directory,
            &["kitty", "buttons.left_bumper", "buttons.right_bumper"],
        );
        assert!(!without.status.success());
        assert!(String::from_utf8_lossy(&without.stderr).contains("--as"));
        assert_eq!(
            std::fs::read_to_string(directory.join("kitty/context.toml")).unwrap(),
            original
        );
        let output = share(
            &directory,
            &[
                "kitty",
                "buttons.left_bumper",
                "buttons.right_bumper",
                "--as",
                "tabs/ctrl-tab",
            ],
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let shared = std::fs::read_to_string(directory.join("shared/tabs/ctrl-tab.toml")).unwrap();
        assert!(
            shared.contains("left_bumper") && shared.contains("right_bumper"),
            "{shared}"
        );
        let loaded = contexts::load(&directory);
        let kitty = find(&loaded, "kitty").unwrap();
        assert_eq!(kitty.key_for(ButtonName::LeftBumper), Some(KeyName::Tab));
        assert_eq!(kitty.key_for(ButtonName::RightBumper), Some(KeyName::Enter));
    }

    #[test]
    fn share_keeps_the_comments_of_the_context_file() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("tehuti");
        write(
            &directory,
            "kitty/context.toml",
            "# my terminal\napp_id = 'kitty'\n\n[buttons.left_bumper] # tabs\nkey = 'tab'\n\n# the rest\n[buttons.east]\nkey = 'enter'\n",
        );
        let output = share(&directory, &["kitty", "buttons.left_bumper", "--as", "tabs"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rewritten = std::fs::read_to_string(directory.join("kitty/context.toml")).unwrap();
        assert!(rewritten.contains("# my terminal"), "{rewritten}");
        assert!(rewritten.contains("# the rest"), "{rewritten}");
        assert!(rewritten.contains("include"), "{rewritten}");
    }

    #[test]
    fn share_moves_a_section_out_replaces_it_with_an_include_and_keeps_a_backup() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("tehuti");
        let original = app(
            "kitty",
            "[buttons.left_bumper]\nkey = 'tab'\n[buttons.east]\nkey = 'enter'\n",
        );
        write(&directory, "kitty/context.toml", &original);
        let output = share(&directory, &["kitty", "buttons.left_bumper", "--as", "tabs"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let shared = std::fs::read_to_string(directory.join("shared/tabs.toml")).unwrap();
        assert!(
            shared.contains("left_bumper") && shared.contains("tab"),
            "{shared}"
        );
        let rewritten = std::fs::read_to_string(directory.join("kitty/context.toml")).unwrap();
        assert!(
            rewritten.contains("include") && rewritten.contains("tabs"),
            "{rewritten}"
        );
        let backups: Vec<_> = std::fs::read_dir(directory.join("kitty"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.file_name().unwrap() != "context.toml")
            .collect();
        assert_eq!(backups.len(), 1, "{backups:?}");
        assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), original);
        let loaded = contexts::load(&directory);
        let kitty = find(&loaded, "kitty").unwrap();
        assert_eq!(kitty.key_for(ButtonName::LeftBumper), Some(KeyName::Tab));
        assert_eq!(kitty.key_for(ButtonName::East), Some(KeyName::Enter));
    }

    #[test]
    fn share_names_the_section_after_its_last_segment_by_default() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("tehuti");
        write(
            &directory,
            "kitty/context.toml",
            &app("kitty", "[buttons.left_bumper]\nkey = 'tab'\n"),
        );
        let output = share(&directory, &["kitty", "buttons.left_bumper"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(directory.join("shared/left_bumper.toml").is_file());
    }

    #[test]
    fn share_refuses_a_name_clash_and_changes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("tehuti");
        let original = app("kitty", "[buttons.left_bumper]\nkey = 'tab'\n");
        write(&directory, "kitty/context.toml", &original);
        write(
            &directory,
            "shared/tabs.toml",
            "[buttons.right_bumper]\nkey = 'enter'\n",
        );
        let output = share(&directory, &["kitty", "buttons.left_bumper", "--as", "tabs"]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("--as"));
        assert_eq!(
            std::fs::read_to_string(directory.join("shared/tabs.toml")).unwrap(),
            "[buttons.right_bumper]\nkey = 'enter'\n"
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("kitty/context.toml")).unwrap(),
            original
        );
    }
}
