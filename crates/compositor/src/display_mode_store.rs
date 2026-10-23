//! The confirmed display choice, replaced atomically in the user config directory.

use std::{io, io::Write as _, path::PathBuf};

/// `$XDG_CONFIG_HOME`, otherwise `$HOME/.config`.
///
/// # Errors
/// Returns an error when neither config directory nor home is available.
pub fn config_root() -> io::Result<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "config directory unavailable"))
}

#[derive(Debug)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    #[must_use]
    pub const fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// # Errors
    /// Returns an error when neither config directory nor home is available.
    pub fn from_env() -> io::Result<Self> {
        Ok(Self::new(config_root()?.join("tehuti/display-mode")))
    }

    /// # Errors
    /// Returns file read errors other than a missing file.
    pub fn read(&self) -> io::Result<Option<String>> {
        match std::fs::read_to_string(&self.path) {
            Ok(value) => Ok(Some(value.lines().next().unwrap_or_default().trim().to_owned())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// # Errors
    /// Returns config directory, file write and atomic replacement errors.
    pub fn write(&self, mode: &str) -> io::Result<()> {
        self.replace(mode, self.read_scale()?)
    }

    /// # Errors
    /// Returns file read errors other than a missing file.
    pub fn read_scale(&self) -> io::Result<Option<i32>> {
        match std::fs::read_to_string(&self.path) {
            Ok(value) => Ok(value
                .lines()
                .nth(1)
                .and_then(|line| line.parse().ok())
                .filter(|scale| *scale > 0)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// # Errors
    /// Returns an error for invalid scales or failed atomic replacement.
    pub fn write_scale(&self, scale: i32) -> io::Result<()> {
        if scale < 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "scale must be positive",
            ));
        }
        self.replace(self.read()?.as_deref().unwrap_or_default(), Some(scale))
    }

    fn replace(&self, mode: &str, scale: Option<i32>) -> io::Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("config directory unavailable"))?;
        std::fs::create_dir_all(parent)?;
        let temporary = self.path.with_extension("tmp");
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(mode.as_bytes())?;
        if let Some(scale) = scale {
            write!(file, "\n{scale}")?;
        }
        file.sync_all()?;
        std::fs::rename(temporary, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::Store;

    #[test]
    fn confirmed_choice_survives_reopening_and_replacement() -> std::io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("tehuti/display-mode");
        let store = Store::new(path.clone());
        assert_eq!(store.read()?, None);
        store.write("3840x2160@120")?;
        assert_eq!(Store::new(path.clone()).read()?, Some("3840x2160@120".into()));
        store.write("1920x1080@60")?;
        assert_eq!(Store::new(path).read()?, Some("1920x1080@60".into()));
        Ok(())
    }

    #[test]
    fn persistence_failure_returns_an_error() -> std::io::Result<()> {
        let directory = tempfile::tempdir()?;
        let parent = directory.path().join("file");
        std::fs::write(&parent, "not a directory")?;
        assert!(
            Store::new(parent.join("display-mode"))
                .write("3840x2160@120")
                .is_err()
        );
        Ok(())
    }
}

#[cfg(test)]
mod scale_tests {
    use super::Store;

    #[test]
    fn scale_and_mode_survive_independent_updates() -> std::io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("display-mode");
        let store = Store::new(path.clone());
        store.write("3840x2160@60")?;
        assert_eq!(store.read_scale()?, None);
        store.write_scale(3)?;
        let reopened = Store::new(path);
        assert_eq!(reopened.read_scale()?, Some(3));
        assert_eq!(reopened.read()?, Some("3840x2160@60".into()));
        reopened.write("1920x1080@60")?;
        assert_eq!(reopened.read_scale()?, Some(3));
        assert!(reopened.write_scale(0).is_err());
        assert_eq!(reopened.read_scale()?, Some(3));
        Ok(())
    }
}
