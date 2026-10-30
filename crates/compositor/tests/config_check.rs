//! `tehuti --check-config` reports what the real parsers reject and what routing never reaches.

#[cfg(test)]
mod config_check {
    use std::{path::Path, process::Command};

    use tehuti::config_check::{Severity, check};

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
        let output = Command::new(env!("CARGO_BIN_EXE_tehuti"))
            .args(["--check-config".as_ref(), directory.path().as_os_str()])
            .env_clear()
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8(output.stdout).unwrap().contains("3 warnings"));
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
}
