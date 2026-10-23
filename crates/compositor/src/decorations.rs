//! Whether clients draw their own title bars, chosen once at startup.

use std::{ffi::OsStr, io, path::Path};

/// Who draws window title bars and their buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Decorations {
    /// Clients draw their own title bars, as without the setting.
    #[default]
    Client,
    /// Clients are told the compositor decorates them, and Tehuti draws nothing.
    None,
}

impl Decorations {
    /// Reads `client` or `none`, ignoring case and surrounding whitespace.
    ///
    /// # Errors
    /// Returns the rejected text for any other value.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "client" => Ok(Self::Client),
            "none" => Ok(Self::None),
            _ => Err(value.trim().to_owned()),
        }
    }
}

/// Picks the setting: `TEHUTI_DECORATIONS` first, then the first line of the config file.
///
/// # Errors
/// Returns the default and a warning when the chosen value is not `client` or `none`.
pub fn choose(env: Option<&OsStr>, file: Option<&str>) -> Result<Decorations, (Decorations, String)> {
    if let Some(value) = env.filter(|value| !value.is_empty()) {
        return value
            .to_str()
            .ok_or_else(|| value.display().to_string())
            .and_then(Decorations::parse)
            .map_err(|rejected| {
                (
                    Decorations::default(),
                    format!("TEHUTI_DECORATIONS=\"{rejected}\" is not client or none; using client"),
                )
            });
    }
    let Some(line) = file
        .and_then(|text| text.lines().next())
        .filter(|line| !line.trim().is_empty())
    else {
        return Ok(Decorations::default());
    };
    Decorations::parse(line).map_err(|rejected| {
        (
            Decorations::default(),
            format!("decorations file holds \"{rejected}\", not client or none; using client"),
        )
    })
}

/// Reads the config file at `path`, treating a missing file as unset.
///
/// # Errors
/// Returns file read errors other than a missing file.
pub fn read_file(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Reads `TEHUTI_DECORATIONS`, otherwise `$XDG_CONFIG_HOME/tehuti/decorations`, logging bad values.
#[must_use]
pub fn from_env() -> Decorations {
    let file = crate::display_mode_store::config_root()
        .and_then(|root| read_file(&root.join("tehuti/decorations")))
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "decorations file unavailable");
            None
        });
    let env = std::env::var_os("TEHUTI_DECORATIONS");
    choose(env.as_deref(), file.as_deref()).unwrap_or_else(|(decorations, warning)| {
        tracing::warn!("{warning}");
        decorations
    })
}
