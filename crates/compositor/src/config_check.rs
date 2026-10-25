//! `tehuti --check-config`: parse a config directory with the real parsers and report problems.

use core::fmt;
use std::path::{Path, PathBuf};

use tehuti_core::shadowing::{ButtonOwner, Shadowing, shadowed};
use tehuti_protocol::ButtonName;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// The file is rejected; Tehuti skips it at startup and refuses it on reload.
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
            if let Err(error) = crate::system_menu::Config::parse(&text) {
                report.push(&menu, Severity::Error, error.to_string());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => report.push(&menu, Severity::Error, error.to_string()),
    }
    report
}

fn report_shadowing(report: &mut Report, loaded: &[(PathBuf, tehuti_protocol::Context)]) {
    let contexts: Vec<_> = loaded.iter().map(|(_, context)| context.clone()).collect();
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
