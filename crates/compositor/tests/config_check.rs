//! `tehuti --check-config` reports what the real parsers reject and what routing never reaches.

#[cfg(test)]
mod config_check {
    use std::{path::Path, process::Command};

    use tehuti::config_check::{Severity, check};

    #[test]
    fn legacy_favourites_load_with_a_warning() {
        for entries in ["[]", "['browser.desktop']"] {
            let directory = tempfile::tempdir().unwrap();
            write(
                directory.path(),
                "global/context.toml",
                &format!("app_id = '*'\nfavourites = {entries}\n[buttons]\n"),
            );
            let report = check(directory.path());
            assert!(!report.has_errors(), "{report}");
            assert_eq!(report.findings.len(), 1, "{report}");
            assert_eq!(report.findings[0].severity, Severity::Warning);
            assert!(report.findings[0].message.contains("favourites"));
        }
    }

    #[test]
    fn duplicate_normalized_speech_phrases_are_an_error() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "global/context.toml",
            "app_id = '*'\n[buttons]\n[speech]\n'Go' = { key = 'enter' }\n'go.' = { key = 'tab' }\n",
        );
        let report = check(directory.path());
        assert!(report.has_errors(), "{report}");
        assert!(report.to_string().contains("normalize to"), "{report}");
    }

    #[test]
    fn shipped_contexts_have_no_duplicate_or_empty_speech_phrases() {
        let report = check(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/contexts"));
        assert!(!report.has_errors(), "{report}");
    }

    #[test]
    fn check_config_rejects_a_key_hold_outside_one_to_a_thousand() {
        for (value, rejected) in [("0", true), ("1001", true), ("1", false), ("1000", false)] {
            let directory = tempfile::tempdir().unwrap();
            write(
                directory.path(),
                "global/context.toml",
                &format!("app_id = '*'\nkey_hold_ms = {value}\n[buttons]\n"),
            );
            let report = check(directory.path());
            assert_eq!(report.has_errors(), rejected, "{value}: {report}");
            if rejected {
                assert!(report.to_string().contains("key_hold_ms"), "{report}");
            }
        }
    }

    #[test]
    fn unknown_ticker_button_names_warn_without_failing() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "global/context.toml",
            "app_id = '*'\n[buttons]\n[ticker.buttons]\nsouth = 'Jump'\nturbo = 'Zoom'\n",
        );
        let report = check(directory.path());
        assert!(!report.has_errors(), "{report}");
        assert_eq!(report.findings.len(), 1, "{report}");
        assert_eq!(report.findings[0].severity, Severity::Warning);
        assert!(report.findings[0].message.contains("turbo"));
    }

    #[test]
    fn reports_each_broken_menu_entry() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "menu.toml",
            r#"
reboot = ["session-reboot"]
unknown = ["ignored"]
[[entry]]
label = "Broken"
exec = []
order = 0
[[entry]]
label = "Terminal"
exec = ["terminal"]
order = 1
"#,
        );
        let report = check(directory.path());
        assert_eq!(report.findings.len(), 2, "{report}");
        assert!(report.to_string().contains("entry[0]"));
        assert!(report.to_string().contains("unknown"));
        assert!(report.has_errors());
    }

    #[test]
    fn a_layout_phrase_with_no_button_is_flagged_as_voice_only() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "global/context.toml",
            "app_id = '*'\n[buttons]\n[speech]\n'Half width' = { layout = 'maximize-column' }\n",
        );
        let report = check(directory.path());
        assert!(!report.has_errors(), "{report}");
        let finding = report
            .findings
            .iter()
            .find(|finding| finding.message.contains("only by voice"))
            .unwrap_or_else(|| panic!("no voice-only warning: {report}"));
        assert_eq!(finding.severity, Severity::Warning);
        assert!(finding.message.contains("Half width"), "{report}");
    }

    #[test]
    fn a_layout_phrase_with_a_button_is_not_voice_only() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "global/context.toml",
            "app_id = '*'\n[buttons]\nnorth = { layout = 'maximize-column' }\n[speech]\n'Half width' = { layout = 'maximize-column' }\n",
        );
        let report = check(directory.path());
        assert!(!report.to_string().contains("only by voice"), "{report}");
    }

    #[test]
    fn shipped_examples_map_no_voice_only_step_phrases() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let report = check(&examples);
        assert!(!report.to_string().contains("only by voice"), "{report}");
        let global = std::fs::read_to_string(examples.join("global/context.toml")).unwrap();
        let context = tehuti_protocol::parse_context(&global).unwrap();
        for mapping in context.speech_mappings() {
            assert!(
                !matches!(
                    mapping.action,
                    tehuti_protocol::Action::Layout {
                        action: tehuti_protocol::LayoutAction::SwitchPresetColumnWidth
                            | tehuti_protocol::LayoutAction::SwitchPresetColumnWidthBack
                    }
                ),
                "step phrase {:?} remains",
                mapping.phrase
            );
        }
    }

    #[test]
    fn goal_phrases_and_menu_actions_are_not_voice_only() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "global/context.toml",
            "app_id = '*'\n[buttons]\n[speech]\n'Half width' = { key = 'h' }\n'Restart' = { restart = true }\n",
        );
        let report = check(directory.path());
        assert!(!report.to_string().contains("only by voice"), "{report}");
    }

    fn write(directory: &Path, relative: &str, text: &str) {
        let path = directory.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn findings(directory: &Path) -> Vec<(String, Severity, String)> {
        check(directory)
            .findings
            .into_iter()
            .map(|finding| {
                let relative = finding
                    .path
                    .strip_prefix(directory)
                    .unwrap()
                    .display()
                    .to_string();
                (relative, finding.severity, finding.message)
            })
            .collect()
    }

    fn broken_config() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            root,
            "global/context.toml",
            "app_id = '*'\npush_to_talk = ['record', 'start']\n[buttons]\nsouth = { key = 'tab' }\n",
        );
        write(
            root,
            "foot/context.toml",
            "app_id = 'foot'\n[buttons]\nsouth = { key = 'tab' }\nguide = { key = 'tab' }\nstart = { key = 'tab' }\neast = { key = 'escape' }\n",
        );
        write(root, "foot2/context.toml", "app_id = 'foot'\n[buttons]\n");
        write(
            root,
            "typo/context.toml",
            "app_id = 'typo'\ncolour = 'red'\n[buttons]\n",
        );
        write(
            root,
            "menu.toml",
            "reboot = ['systemctl', 'reboot']\nsleep = ['systemctl', 'suspend']\n",
        );
        directory
    }

    const GOOD_MANIFEST: &str =
        r#"{"executable":"run.sh","app_ids":["foot"],"events":[],"start":"always","permissions":[]}"#;

    #[test]
    fn reports_a_bad_plugin_manifest_and_accepts_a_good_one() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "plugins/good.json", GOOD_MANIFEST);
        write(
            directory.path(),
            "plugins/bad.json",
            r#"{"executable":"run.sh","colour":"red"}"#,
        );
        write(directory.path(), "plugins/notes.txt", "not a manifest");
        let report = check(directory.path());
        let found = findings(directory.path());
        assert_eq!(found.len(), 1, "{report}");
        assert_eq!(found[0].0, "plugins/bad.json");
        assert_eq!(found[0].1, Severity::Error);
        assert!(found[0].2.contains("colour"), "{}", found[0].2);
        assert_eq!(report.checked, 2, "two manifests, no contexts");
    }

    #[test]
    fn warns_when_a_context_folder_holds_a_plugins_directory() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "plugins/good.json", GOOD_MANIFEST);
        write(
            directory.path(),
            "foot/context.toml",
            "app_id = 'foot'\n[buttons]\n",
        );
        write(directory.path(), "foot/plugins/stray.json", GOOD_MANIFEST);
        let found = findings(directory.path());
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].0, "foot/plugins");
        assert_eq!(found[0].1, Severity::Warning);
        assert!(found[0].2.contains("never loaded"), "{}", found[0].2);
        assert!(found[0].2.contains("plugins"), "{}", found[0].2);
    }

    #[test]
    fn reports_parse_errors_unknown_fields_and_shadowed_mappings() {
        let directory = broken_config();
        let found = findings(directory.path());
        let messages: Vec<String> = found
            .iter()
            .map(|(path, severity, message)| {
                format!("{path} {severity:?} {}", message.lines().next().unwrap())
            })
            .collect();
        assert!(
            messages.contains(&"foot/context.toml Warning [buttons] guide never fires: the escape chord consumes every Guide press".into()),
            "{messages:#?}"
        );
        assert!(
            messages.contains(&"global/context.toml Warning [buttons] south never fires: the global layer sends Enter for South unless a focused context maps it".into()),
            "{messages:#?}"
        );
        assert!(
            !messages
                .iter()
                .any(|line| line.starts_with("foot/context.toml Warning [buttons] south")),
            "a focused context's South replaces the global Enter: {messages:#?}"
        );
        assert!(
            messages
                .iter()
                .any(|line| line.starts_with("foot/context.toml Warning [buttons] start never fires")),
            "{messages:#?}"
        );
        assert!(
            !messages.iter().any(|line| line.contains("[buttons] east")),
            "a reachable mapping is not reported: {messages:#?}"
        );
        assert!(
            messages
                .iter()
                .any(|line| line
                    .starts_with("foot2/context.toml Warning app_id \"foot\" is already claimed by")),
            "{messages:#?}"
        );
        let typo = found
            .iter()
            .find(|(path, _, _)| path == "typo/context.toml")
            .unwrap();
        assert_eq!(typo.1, Severity::Error);
        assert!(typo.2.contains("unknown field `colour`"), "{}", typo.2);
        let menu = found.iter().find(|(path, _, _)| path == "menu.toml").unwrap();
        assert_eq!(menu.1, Severity::Error);
        assert!(menu.2.contains("\"sleep\""), "{}", menu.2);
        assert_eq!(check(directory.path()).checked, 5, "four contexts and the menu");
    }

    #[test]
    fn invoke_phrases_warn_but_a_lone_invoke_remains_reachable() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "context.toml",
            "app_id = '*'\n[buttons]\n[speech]\n'Invoke help' = { key = 'enter' }\n'Invoke <name>' = { text = '/{name} ' }\n'Invoke' = { key = 'enter' }\n'Inventory help' = { key = 'enter' }",
        );
        let report = check(directory.path());
        assert!(!report.has_errors(), "{report}");
        let invoked: Vec<_> = report
            .findings
            .iter()
            .filter(|finding| finding.message.contains("built-in invoke verb"))
            .collect();
        assert_eq!(invoked.len(), 2, "{report}");
        assert!(
            invoked
                .iter()
                .all(|finding| finding.severity == Severity::Warning),
            "{report}"
        );
    }

    #[test]
    fn reserved_unit_verbs_warn_but_unrelated_phrases_remain_reachable() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "context.toml",
            "app_id = '*'\n[buttons]\n[speech]\n'OPEN notes' = { key = 'enter' }\n'Åpne notes' = { key = 'enter' }\n'Work on <project>' = { text = 'cd -- {project}' }\n'Open' = { key = 'enter' }\n'Opening notes' = { key = 'enter' }",
        );
        let report = check(directory.path());
        assert!(!report.has_errors(), "{report}");
        assert_eq!(report.findings.len(), 3, "{report}");
        assert!(
            report
                .findings
                .iter()
                .all(|finding| finding.severity == Severity::Warning
                    && finding.message.contains("reserved unit verb")),
            "{report}"
        );
        let text = report.to_string();
        for word in ["\"open\"", "\"åpne\"", "\"work on\""] {
            assert!(
                text.contains(&format!("reserved unit verb {word}")),
                "{word}: {text}"
            );
        }
        let output = Command::new(env!("CARGO_BIN_EXE_tehuti"))
            .args(["--check-config".as_ref(), directory.path().as_os_str()])
            .env_clear()
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8(output.stdout).unwrap().contains("3 warnings"));
    }

    #[test]
    fn invalid_window_widths_are_errors_and_valid_ones_are_not() {
        for bad in ["\"3/2\"", "\"0/3\"", "\"two\"", "1.5", "0.0", "-5", "0"] {
            let directory = tempfile::tempdir().unwrap();
            write(
                directory.path(),
                "game/context.toml",
                &format!("app_id = 'org.example.Game'\nbuttons = {{}}\n[window]\nwidth = {bad}\n"),
            );
            let report = check(directory.path());
            assert!(report.has_errors(), "{bad}: {report}");
            assert!(
                report.to_string().contains("game/context.toml: error:"),
                "{bad}: {report}"
            );
            let output = Command::new(env!("CARGO_BIN_EXE_tehuti"))
                .args(["--check-config".as_ref(), directory.path().as_os_str()])
                .env_clear()
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(1), "{bad}: {output:?}");
        }
        for good in ["\"2/3\"", "1280", "0.667", "1"] {
            let directory = tempfile::tempdir().unwrap();
            write(
                directory.path(),
                "game/context.toml",
                &format!("app_id = 'org.example.Game'\nbuttons = {{}}\n[window]\nwidth = {good}\n"),
            );
            let report = check(directory.path());
            assert!(!report.has_errors(), "{good}: {report}");
        }
    }

    #[test]
    fn a_clean_directory_has_no_findings() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "foot/context.toml",
            "app_id = 'foot'\n[buttons]\nsouth = { key = 'enter' }\neast = { key = 'escape' }\n",
        );
        let report = check(directory.path());
        assert_eq!(report.findings, vec![]);
        assert!(!report.has_errors());
        assert_eq!(report.to_string(), "1 files checked, 0 errors, 0 warnings\n");
    }

    #[test]
    fn generated_contexts_without_gated_fields_pass_the_current_parser() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/fixtures/generate-context");
        for parser in ["supertux2", "mgba"] {
            let directory = tempfile::tempdir().unwrap();
            let source = std::fs::read_to_string(fixtures.join(format!("{parser}.off.toml"))).unwrap();
            write(directory.path(), "context.toml", &source);
            let report = check(directory.path());
            assert!(!report.has_errors(), "{parser}: {report}");
            let output = Command::new(env!("CARGO_BIN_EXE_tehuti"))
                .args(["--check-config".as_ref(), directory.path().as_os_str()])
                .env_clear()
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }
    }

    #[test]
    fn a_speech_only_context_checks_clean() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "notes/context.toml",
            "app_id = 'org.example.Notes'\n[speech]\n'save it' = { key = 's', mods = ['ctrl'] }\n",
        );
        let report = check(directory.path());
        assert!(!report.has_errors(), "{report}");
    }

    #[test]
    fn a_missing_directory_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let report = check(&directory.path().join("absent"));
        assert!(report.has_errors(), "{report}");
    }

    #[test]
    fn the_binary_exits_non_zero_on_errors_and_zero_on_warnings() {
        let directory = broken_config();
        let output = Command::new(env!("CARGO_BIN_EXE_tehuti"))
            .args(["--check-config".as_ref(), directory.path().as_os_str()])
            .env_clear()
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("typo/context.toml: error:"), "{stdout}");
        assert!(
            stdout.ends_with("5 files checked, 2 errors, 4 warnings\n"),
            "{stdout}"
        );

        std::fs::remove_dir_all(directory.path().join("typo")).unwrap();
        std::fs::remove_file(directory.path().join("menu.toml")).unwrap();
        let fixed = Command::new(env!("CARGO_BIN_EXE_tehuti"))
            .args(["--check-config".as_ref(), directory.path().as_os_str()])
            .env_clear()
            .output()
            .unwrap();
        assert_eq!(fixed.status.code(), Some(0), "warnings alone pass: {fixed:?}");
    }

    #[test]
    fn the_default_directory_is_tehuti_under_the_config_home() {
        let config_home = tempfile::tempdir().unwrap();
        write(config_home.path(), "tehuti/broken/context.toml", "not toml [");
        let output = Command::new(env!("CARGO_BIN_EXE_tehuti"))
            .arg("--check-config")
            .env_clear()
            .env("XDG_CONFIG_HOME", config_home.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("tehuti/broken/context.toml: error:"), "{stdout}");
    }

    #[test]
    fn titled_contexts_coexist_with_the_plain_one_but_duplicates_warn() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "proton/context.toml",
            "app_id = 'steam_proton'\n[buttons]\n",
        );
        let titled = "app_id = 'steam_proton'\ntitle = 'World of Warcraft'\n[buttons]\n";
        write(directory.path(), "wow/context.toml", titled);
        let distinct = check(directory.path());
        assert!(distinct.findings.is_empty(), "{distinct}");
        write(directory.path(), "wow2/context.toml", titled);
        let report = check(directory.path());
        assert_eq!(report.findings.len(), 1, "{report}");
        assert!(
            report
                .to_string()
                .contains("with title \"World of Warcraft\" is already claimed by"),
            "{report}"
        );
    }
}
