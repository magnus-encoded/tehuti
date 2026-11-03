//! `tehuti --check-config`: parse a config directory with the real parsers and report problems.

use core::fmt;
use std::path::{Path, PathBuf};

use tehuti_core::shadowing::{ButtonOwner, Shadowing, shadowed};
use tehuti_protocol::ButtonName;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Invalid configuration is skipped and must be corrected.
    Error,
    /// The file loads, but part of it never takes effect.
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: PathBuf,
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Files read, whether or not they parsed.
    pub checked: usize,
    pub findings: Vec<Finding>,
}

impl Report {
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == Severity::Error)
    }

    fn count(&self, severity: Severity) -> usize {
        self.findings
            .iter()
            .filter(|finding| finding.severity == severity)
            .count()
    }

    fn push(&mut self, path: &Path, severity: Severity, message: String) {
        self.findings.push(Finding {
            path: path.to_path_buf(),
            severity,
            message,
        });
    }
}

impl fmt::Display for Report {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for finding in &self.findings {
            let label = match finding.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            let mut lines = finding.message.lines();
            writeln!(
                formatter,
                "{}: {label}: {}",
                finding.path.display(),
                lines.next().unwrap_or_default()
            )?;
            for line in lines {
                writeln!(formatter, "    {line}")?;
            }
        }
        writeln!(
            formatter,
            "{} files checked, {} errors, {} warnings",
            self.checked,
            self.count(Severity::Error),
            self.count(Severity::Warning)
        )
    }
}

/// Checks every `context.toml` that Tehuti would load from `directory`, and its `menu.toml`.
#[must_use]
pub fn check(directory: &Path) -> Report {
    let mut report = Report::default();
    match crate::contexts::load_each(directory) {
        Ok(outcomes) => {
            report.checked = outcomes.len();
            let mut loaded = Vec::new();
            for outcome in outcomes {
                match outcome {
                    Ok(file) => loaded.push(file),
                    Err(error) => report.push(&error.path, Severity::Error, error.message),
                }
            }
            report_shadowing(&mut report, &loaded);
        }
        Err(error) => report.push(&error.path, Severity::Error, error.message),
    }
    let menu = directory.join("menu.toml");
    match std::fs::read_to_string(&menu) {
        Ok(text) => {
            report.checked = report.checked.saturating_add(1);
            match crate::system_menu::Config::parse(&text) {
                Ok(config) => {
                    for notice in config.notices() {
                        report.push(&menu, Severity::Error, notice.clone());
                    }
                }
                Err(error) => report.push(&menu, Severity::Error, error.to_string()),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => report.push(&menu, Severity::Error, error.to_string()),
    }
    check_plugins(directory, &mut report);
    report
}

/// Validates `<directory>/plugins/*.json`, the only place manifests load from, and flags
/// `plugins/` folders inside context folders, which nothing loads.
fn check_plugins(directory: &Path, report: &mut Report) {
    let plugins = directory.join("plugins");
    if let Ok(entries) = std::fs::read_dir(&plugins) {
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect();
        paths.sort();
        for path in paths
            .into_iter()
            .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        {
            report.checked = report.checked.saturating_add(1);
            let parsed = std::fs::read(&path)
                .map_err(|error| error.to_string())
                .and_then(|bytes| tehuti_protocol::parse_manifest(&bytes).map(drop));
            if let Err(message) = parsed {
                report.push(&path, Severity::Error, message);
            }
        }
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut folders: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.join("context.toml").is_file() && path.join("plugins").is_dir())
        .collect();
    folders.sort();
    for folder in folders {
        report.push(
            &folder.join("plugins"),
            Severity::Warning,
            format!("never loaded: plugins load only from {}", plugins.display()),
        );
    }
}

fn report_shadowing(report: &mut Report, loaded: &[(PathBuf, tehuti_protocol::Context)]) {
    let contexts: Vec<_> = loaded.iter().map(|(_, context)| context.clone()).collect();
    for (path, context) in loaded {
        let mut reserved: Vec<_> = context
            .speech_mappings()
            .filter(|mapping| tehuti_core::unit_verbs::launcher_request(mapping.phrase).is_some())
            .collect();
        reserved.sort_by_key(|mapping| mapping.phrase);
        for mapping in reserved {
            report.push(
                path,
                Severity::Warning,
                format!(
                    "[speech] {:?} never fires: core owns this reserved unit verb",
                    mapping.phrase
                ),
            );
        }
    }
    for shadowing in shadowed(&contexts) {
        match shadowing {
            Shadowing::Context { context, earlier } => {
                if let (Some((path, shadowed_context)), Some((earlier_path, _))) =
                    (loaded.get(context), loaded.get(earlier))
                {
                    report.push(
                        path,
                        Severity::Warning,
                        format!(
                            "app_id {:?} is already claimed by {}; this context never becomes active",
                            shadowed_context.app_id(),
                            earlier_path.display()
                        ),
                    );
                }
            }
            Shadowing::Button {
                context,
                button,
                owner,
            } => {
                if let Some((path, _)) = loaded.get(context) {
                    let reason = match owner {
                        ButtonOwner::EscapeChord => "the escape chord consumes every Guide press",
                        ButtonOwner::GlobalEnter => {
                            "the global layer sends Enter for South unless a focused context maps it"
                        }
                        ButtonOwner::PushToTalk => {
                            "it is in the global context's push_to_talk list (default record)"
                        }
                    };
                    report.push(
                        path,
                        Severity::Warning,
                        format!("[buttons] {} never fires: {reason}", snake_case(button)),
                    );
                }
            }
        }
    }
}

fn snake_case(button: ButtonName) -> String {
    serde_json::to_value(button)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{button:?}"))
}
