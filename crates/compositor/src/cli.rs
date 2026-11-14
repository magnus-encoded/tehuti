//! The command line, parsed before Tehuti touches the session.

use std::ffi::OsString;

pub const USAGE: &str = "\
usage: tehuti <context-directory> [application] [arguments...]
       tehuti --test-driver <context-directory> [application] [arguments...]
       tehuti --help | -h
       tehuti --version | -V
       tehuti --check-config [config-directory]
       tehuti --install-skill [path]
";

/// What one run of the binary does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    Help,
    Version,
    /// Check the configuration directory; the rest of the arguments follow the option.
    CheckConfig(Vec<OsString>),
    /// Install the agent skill under the given skills directory, or `~/.agents/skills`.
    InstallSkill(Option<OsString>),
    /// Start the compositor with this context directory and optional first client.
    Session {
        context_directory: OsString,
        application: Option<OsString>,
        application_arguments: Vec<OsString>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UsageError {
    #[error("missing context directory")]
    MissingContextDirectory,
    #[error("unknown option {}", .0.to_string_lossy())]
    UnknownOption(OsString),
    #[error(
        "{} is an option: write --{}, or ./{} for a directory",
        .0.to_string_lossy(),
        .0.to_string_lossy(),
        .0.to_string_lossy()
    )]
    MissingDashes(OsString),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opt {
    Help,
    Version,
    CheckConfig,
    InstallSkill,
    /// Taken off by `main` before parsing; anywhere else it is unknown.
    TestDriver,
}

/// Every long option, without its dashes; `parse` matches options only through this table.
const OPTIONS: &[(&str, Opt)] = &[
    ("help", Opt::Help),
    ("version", Opt::Version),
    ("check-config", Opt::CheckConfig),
    ("install-skill", Opt::InstallSkill),
    ("test-driver", Opt::TestDriver),
];

fn long_option(word: &str) -> Option<Opt> {
    OPTIONS
        .iter()
        .find(|(name, _)| *name == word)
        .map(|(_, option)| *option)
}

fn option(argument: &str) -> Option<Opt> {
    match argument {
        "-h" => Some(Opt::Help),
        "-V" => Some(Opt::Version),
        _ => argument.strip_prefix("--").and_then(long_option),
    }
}

/// Parses the arguments after the program name, asking the file system whether a bare option word is a directory.
///
/// # Errors
/// As [`parse_with`].
pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Invocation, UsageError> {
    parse_with(arguments, |path| std::path::Path::new(path).is_dir())
}

/// Parses the arguments after the program name.
///
/// Only the first argument is read as an option: everything after the
/// context directory belongs to the application. A bare option word such as
/// `help` is a context directory only when `is_directory` says it exists.
///
/// # Errors
/// Returns an error when the context directory is missing, the first argument is an unknown option,
/// or it is an option name without its dashes that names no directory.
pub fn parse_with(
    arguments: impl IntoIterator<Item = OsString>,
    is_directory: impl Fn(&std::ffi::OsStr) -> bool,
) -> Result<Invocation, UsageError> {
    let mut arguments = arguments.into_iter();
    let first = arguments.next().ok_or(UsageError::MissingContextDirectory)?;
    match first.to_str().and_then(option) {
        Some(Opt::Help) => return Ok(Invocation::Help),
        Some(Opt::Version) => return Ok(Invocation::Version),
        Some(Opt::CheckConfig) => return Ok(Invocation::CheckConfig(arguments.collect())),
        Some(Opt::InstallSkill) => return Ok(Invocation::InstallSkill(arguments.next())),
        Some(Opt::TestDriver) | None => {}
    }
    if first.as_encoded_bytes().starts_with(b"-") {
        return Err(UsageError::UnknownOption(first));
    }
    if first.to_str().and_then(long_option).is_some() && !is_directory(&first) {
        return Err(UsageError::MissingDashes(first));
    }
    Ok(Invocation::Session {
        context_directory: first,
        application: arguments.next(),
        application_arguments: arguments.collect(),
    })
}

#[must_use]
pub fn format_version(version: &str, hash: &str, date: &str) -> String {
    format!("tehuti {version} ({hash} {date})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_option_name_without_dashes_is_rejected_not_read_as_a_context_directory() {
        for word in ["check-config", "help", "version", "install-skill", "test-driver"] {
            assert_eq!(
                parse([word, "--help"].map(OsString::from)),
                Err(UsageError::MissingDashes(OsString::from(word))),
                "{word}"
            );
        }
        assert!(matches!(
            parse([OsString::from("contexts")]),
            Ok(Invocation::Session { .. })
        ));
        let message = parse([OsString::from("help")]).unwrap_err().to_string();
        assert!(
            message.contains("--help") && message.contains("./help"),
            "{message}"
        );
    }

    #[test]
    fn an_existing_directory_named_like_an_option_starts_a_session() {
        for word in ["help", "./help"] {
            assert_eq!(
                parse_with([OsString::from(word)], |path| path == "help" || path == "./help"),
                Ok(Invocation::Session {
                    context_directory: OsString::from(word),
                    application: None,
                    application_arguments: vec![],
                }),
                "{word}"
            );
        }
        assert!(matches!(
            parse_with([OsString::from("./help")], |_| false),
            Ok(Invocation::Session { .. })
        ));
    }

    #[test]
    fn install_skill_without_a_path_uses_the_default() {
        assert_eq!(
            parse([OsString::from("--install-skill")]),
            Ok(Invocation::InstallSkill(None))
        );
    }

    #[test]
    fn install_skill_takes_an_optional_path() {
        assert_eq!(
            parse(["--install-skill", "/tmp/skills"].map(OsString::from)),
            Ok(Invocation::InstallSkill(Some(OsString::from("/tmp/skills"))))
        );
    }
}
