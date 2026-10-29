//! The controller and microphone picked on the settings Input page.
//!
//! `controller` holds one pad name; no file means every pad. `speech-source`
//! holds `all`, `auto` or a `PipeWire` node name, and the speech sidecar reads it
//! at each press.

use core::time::Duration;
use std::{io, io::Write as _, path::PathBuf};

use tehuti_core::dispatch::{ControllerChoice, Microphone, MicrophoneChoice};

#[derive(Debug, Clone)]
pub struct Store {
    directory: PathBuf,
}

impl Store {
    #[must_use]
    pub const fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// # Errors
    /// Returns an error when neither config directory nor home is available.
    pub fn from_env() -> io::Result<Self> {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|root| Self::new(root.join("tehuti")))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "config directory unavailable"))
    }

    /// # Errors
    /// Returns file read errors other than a missing file.
    pub fn controller(&self) -> io::Result<ControllerChoice> {
        Ok(self
            .line("controller")?
            .map_or(ControllerChoice::Auto, ControllerChoice::Named))
    }

    /// # Errors
    /// Returns config directory, file write and atomic replacement errors.
    pub fn set_controller(&self, choice: &ControllerChoice) -> io::Result<()> {
        match choice {
            ControllerChoice::Auto => match std::fs::remove_file(self.directory.join("controller")) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
                Ok(()) | Err(_) => Ok(()),
            },
            ControllerChoice::Named(name) => self.replace("controller", name),
        }
    }

    /// # Errors
    /// Returns file read errors other than a missing file.
    pub fn microphone(&self) -> io::Result<MicrophoneChoice> {
        Ok(match self.line("speech-source")?.as_deref() {
            None | Some("all") => MicrophoneChoice::All,
            Some("auto") => MicrophoneChoice::Auto,
            Some(name) => MicrophoneChoice::Named(name.to_owned()),
        })
    }

    /// # Errors
    /// Returns config directory, file write and atomic replacement errors.
    pub fn set_microphone(&self, choice: &MicrophoneChoice) -> io::Result<()> {
        self.replace(
            "speech-source",
            match choice {
                MicrophoneChoice::All => "all",
                MicrophoneChoice::Auto => "auto",
                MicrophoneChoice::Named(name) => name,
            },
        )
    }

    fn line(&self, name: &str) -> io::Result<Option<String>> {
        match std::fs::read_to_string(self.directory.join(name)) {
            Ok(value) => Ok(value
                .lines()
                .next()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn replace(&self, name: &str, value: &str) -> io::Result<()> {
        if value.contains('\n') {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "choice spans lines"));
        }
        std::fs::create_dir_all(&self.directory)?;
        let path = self.directory.join(name);
        let temporary = path.with_extension("tmp");
        let mut file = std::fs::File::create(&temporary)?;
        writeln!(file, "{value}")?;
        file.sync_all()?;
        std::fs::rename(temporary, path)
    }
}

/// Capture sources in a `pw-dump` listing, as the speech sidecar finds them: no monitors.
#[must_use]
pub fn capture_sources(listing: &serde_json::Value) -> Vec<Microphone> {
    let mut microphones: Vec<Microphone> = Vec::new();
    for node in listing.as_array().into_iter().flatten() {
        if node.get("type").and_then(serde_json::Value::as_str) != Some("PipeWire:Interface:Node") {
            continue;
        }
        let Some(properties) = node.pointer("/info/props") else {
            continue;
        };
        let text = |key: &str| properties.get(key).and_then(serde_json::Value::as_str);
        let Some(name) = text("node.name").filter(|name| !name.is_empty()) else {
            continue;
        };
        if text("media.class") != Some("Audio/Source")
            || text("device.class") == Some("monitor")
            || name.ends_with(".monitor")
            || microphones.iter().any(|microphone| microphone.name == name)
        {
            continue;
        }
        microphones.push(Microphone {
            name: name.to_owned(),
            description: text("node.description")
                .or_else(|| text("node.nick"))
                .unwrap_or(name)
                .to_owned(),
        });
    }
    microphones
}

/// Lists capture sources off the event loop; an empty list when `pw-dump` fails or hangs.
#[must_use]
pub fn list_microphones() -> std::sync::mpsc::Receiver<Vec<Microphone>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("microphones".into())
        .spawn(move || {
            let _sent = sender.send(run_pw_dump().unwrap_or_else(|error| {
                tracing::warn!(%error, "microphone listing failed");
                Vec::new()
            }));
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "microphone listing thread failed");
    }
    receiver
}

fn run_pw_dump() -> io::Result<Vec<Microphone>> {
    let mut child = crate::child_process::command("pw-dump")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        if let Some(mut stdout) = stdout {
            let _read = io::Read::read_to_end(&mut stdout, &mut output);
        }
        output
    });
    let started = std::time::Instant::now();
    while child.try_wait()?.is_none() {
        if started.elapsed() >= Duration::from_secs(3) {
            let _killed = child.kill();
            let _reaped = child.wait();
            return Err(io::Error::new(io::ErrorKind::TimedOut, "pw-dump timed out"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = reader
        .join()
        .map_err(|_panic| io::Error::other("pw-dump reader panicked"))?;
    let listing = serde_json::from_slice(&output).map_err(io::Error::other)?;
    Ok(capture_sources(&listing))
}

#[cfg(test)]
mod tests {
    use super::{Store, capture_sources};
    use tehuti_core::dispatch::{ControllerChoice, Microphone, MicrophoneChoice};

    #[test]
    fn choices_survive_reopening_and_auto_controller_removes_its_file() -> std::io::Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::new(directory.path().join("tehuti"));
        assert_eq!(store.controller()?, ControllerChoice::Auto);
        assert_eq!(store.microphone()?, MicrophoneChoice::All);
        let pad = ControllerChoice::Named("Wireless Controller".into());
        store.set_controller(&pad)?;
        store.set_microphone(&MicrophoneChoice::Named("alsa_input.analog".into()))?;
        let reopened = Store::new(directory.path().join("tehuti"));
        assert_eq!(reopened.controller()?, pad);
        assert_eq!(
            reopened.microphone()?,
            MicrophoneChoice::Named("alsa_input.analog".into())
        );
        reopened.set_microphone(&MicrophoneChoice::Auto)?;
        assert_eq!(
            std::fs::read_to_string(directory.path().join("tehuti/speech-source"))?,
            "auto\n"
        );
        reopened.set_controller(&ControllerChoice::Auto)?;
        reopened.set_controller(&ControllerChoice::Auto)?;
        assert!(!directory.path().join("tehuti/controller").exists());
        assert_eq!(reopened.controller()?, ControllerChoice::Auto);
        Ok(())
    }

    #[test]
    fn pw_dump_listing_keeps_named_capture_devices_once() {
        let listing = serde_json::json!([
            {"type": "PipeWire:Interface:Node", "info": {"props": {
                "node.name": "alsa_input.pci-0000_00_1b.0.analog-stereo",
                "node.description": "Built-in Audio Analog Stereo",
                "media.class": "Audio/Source"}}},
            {"type": "PipeWire:Interface:Node", "info": {"props": {
                "node.name": "alsa_output.analog-stereo.monitor", "media.class": "Audio/Source"}}},
            {"type": "PipeWire:Interface:Node", "info": {"props": {
                "node.name": "renamed", "device.class": "monitor", "media.class": "Audio/Source"}}},
            {"type": "PipeWire:Interface:Node", "info": {"props": {
                "node.name": "alsa_output.analog-stereo", "media.class": "Audio/Sink"}}},
            {"type": "PipeWire:Interface:Node", "info": {"props": {
                "node.name": "bluez_input.headset", "media.class": "Audio/Source"}}},
            {"type": "PipeWire:Interface:Node", "info": {"props": {
                "node.name": "bluez_input.headset", "media.class": "Audio/Source"}}},
            {"type": "PipeWire:Interface:Device"}
        ]);
        assert_eq!(
            capture_sources(&listing),
            vec![
                Microphone {
                    name: "alsa_input.pci-0000_00_1b.0.analog-stereo".into(),
                    description: "Built-in Audio Analog Stereo".into(),
                },
                Microphone {
                    name: "bluez_input.headset".into(),
                    description: "bluez_input.headset".into(),
                },
            ]
        );
    }
}
