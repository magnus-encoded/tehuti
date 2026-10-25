//! The command line, parsed before Tehuti touches the session.

use std::ffi::OsString;

pub const USAGE: &str = "\
usage: tehuti <context-directory> [application] [arguments...]
       tehuti --help | -h
       tehuti --version | -V
       tehuti --check-config [config-directory]
";

/// What one run of the binary does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    Help,
    Version,
    /// Check the configuration directory; the rest of the arguments follow the option.
    CheckConfig(Vec<OsString>),
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
        _ if first.as_encoded_bytes().starts_with(b"-") => Err(UsageError::UnknownOption(first)),
        _ => Ok(Invocation::Session {
            context_directory: first,
            application: arguments.next(),
            application_arguments: arguments.collect(),
        }),
    }
}
