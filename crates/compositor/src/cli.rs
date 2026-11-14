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
}

/// Parses the arguments after the program name.
///
/// Only the first argument is read as an option: everything after the
/// context directory belongs to the application.
///
/// # Errors
/// Returns an error when the context directory is missing or the first argument is an unknown option.
pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Invocation, UsageError> {
    let mut arguments = arguments.into_iter();
    let first = arguments.next().ok_or(UsageError::MissingContextDirectory)?;
    match first.to_str() {
        Some("--help" | "-h") => Ok(Invocation::Help),
        Some("--version" | "-V") => Ok(Invocation::Version),
        Some("--check-config") => Ok(Invocation::CheckConfig(arguments.collect())),
        Some("--install-skill") => Ok(Invocation::InstallSkill(arguments.next())),
        _ if first.as_encoded_bytes().starts_with(b"-") => Err(UsageError::UnknownOption(first)),
        _ => Ok(Invocation::Session {
            context_directory: first,
            application: arguments.next(),
            application_arguments: arguments.collect(),
        }),
    }
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
