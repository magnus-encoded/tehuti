//! The confirmed display choice, replaced atomically in the user config directory.

use std::{io, path::PathBuf};

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
        let root = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "config directory unavailable"))?;
        Ok(Self::new(root.join("tehuti/display-mode")))
    }

    pub(crate) fn sibling(&self, name: &str) -> Self {
        Self::new(self.path.with_file_name(name))
    }

    /// # Errors
    /// Returns file read errors other than a missing file.
    pub fn read(&self) -> io::Result<Option<String>> {
        match std::fs::read_to_string(&self.path) {
            Ok(value) => Ok(Some(value.trim().to_owned())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// # Errors
    /// Returns config directory, file write and atomic replacement errors.
    pub fn write(&self, mode: &str) -> io::Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("config directory unavailable"))?;
        std::fs::create_dir_all(parent)?;
        let temporary = self.path.with_extension("tmp");
        std::fs::write(&temporary, mode)?;
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
