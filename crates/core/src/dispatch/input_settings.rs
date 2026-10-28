//! The Input page of the settings view: which controller and which microphones to use.

/// Which connected pad drives Tehuti.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ControllerChoice {
    /// Every connected pad.
    #[default]
    Auto,
    /// Only pads with this device name, while one is connected.
    Named(String),
}

/// A connected pad's saved kernel name and its visible, numbered label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerInfo {
    pub name: String,
    pub label: String,
}

impl From<&str> for ControllerInfo {
    fn from(name: &str) -> Self {
        Self {
            name: name.into(),
            label: format!("{} #1", name.to_uppercase()),
        }
    }
}

impl From<String> for ControllerInfo {
    fn from(name: String) -> Self {
        Self {
            label: format!("{} #1", name.to_uppercase()),
            name,
        }
    }
}

/// Which capture sources push-to-talk records.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MicrophoneChoice {
    /// Every capture source at once; the loudest recording wins.
    #[default]
    All,
    /// The `PipeWire` default source.
    Auto,
    /// One `PipeWire` node.
    Named(String),
}

/// A `PipeWire` capture source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Microphone {
    /// The `node.name` that the speech sidecar targets.
    pub name: String,
    /// The `node.description` shown to the user.
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputRow {
    #[default]
    Controller,
    Microphone,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InputSettings {
    pub row: InputRow,
    /// Connected pads in device path order.
    pub controllers: Vec<ControllerInfo>,
    pub controller: ControllerChoice,
    pub microphones: Vec<Microphone>,
    pub microphone: MicrophoneChoice,
}

impl InputSettings {
    /// Auto, then each connected pad name once, then a saved pad that is not connected.
    #[must_use]
    pub fn controller_options(&self) -> Vec<ControllerChoice> {
        let mut options = vec![ControllerChoice::Auto];
        for controller in &self.controllers {
            let option = ControllerChoice::Named(controller.name.clone());
            if !options.contains(&option) {
                options.push(option);
            }
        }
        if !options.contains(&self.controller) {
            options.push(self.controller.clone());
        }
        options
    }

    /// All, Auto, then each listed source, then a saved source that is not listed.
    #[must_use]
    pub fn microphone_options(&self) -> Vec<MicrophoneChoice> {
        let mut options = vec![MicrophoneChoice::All, MicrophoneChoice::Auto];
        for microphone in &self.microphones {
            let option = MicrophoneChoice::Named(microphone.name.clone());
            if !options.contains(&option) {
                options.push(option);
            }
        }
        if !options.contains(&self.microphone) {
            options.push(self.microphone.clone());
        }
        options
    }

    #[must_use]
    pub fn controller_label(&self) -> String {
        match &self.controller {
            ControllerChoice::Auto => {
                if self.controllers.is_empty() {
                    return "AUTO (NO PAD)".into();
                }
                let names: Vec<_> = self
                    .controllers
                    .iter()
                    .map(|controller| controller.label.clone())
                    .collect();
                let full = format!("AUTO: {}", names.join(", "));
                if full.chars().count() <= 36 {
                    return full;
                }
                if names.len() == 1 {
                    return full.chars().take(36).collect();
                }
                let suffix = format!(" +{}", names.len().saturating_sub(1));
                let available = 36usize
                    .saturating_sub("AUTO: ".len())
                    .saturating_sub(suffix.len());
                let first: String = names
                    .first()
                    .into_iter()
                    .flat_map(|name| name.chars())
                    .take(available)
                    .collect();
                format!("AUTO: {first}{suffix}")
            }
            ControllerChoice::Named(name) => self
                .controllers
                .iter()
                .find(|controller| controller.name == *name)
                .map_or_else(
                    || format!("{} (OFF)", name.to_uppercase()),
                    |controller| controller.label.clone(),
                ),
        }
    }

    #[must_use]
    pub fn microphone_label(&self) -> String {
        match &self.microphone {
            MicrophoneChoice::All => "ALL MICROPHONES".into(),
            MicrophoneChoice::Auto => "AUTO (SYSTEM DEFAULT)".into(),
            MicrophoneChoice::Named(name) => self
                .microphones
                .iter()
                .find(|microphone| microphone.name == *name)
                .map_or_else(
                    || format!("{} (OFF)", name.to_uppercase()),
                    |microphone| microphone.description.to_uppercase(),
                ),
        }
    }
}

/// The option after or before `current`; an unlisted current value counts as the first.
pub(super) fn cycle<T: PartialEq + Clone>(options: &[T], current: &T, forward: bool) -> Option<T> {
    let index = options.iter().position(|option| option == current).unwrap_or(0);
    let next = if forward {
        index.saturating_add(1).checked_rem(options.len())?
    } else {
        index
            .checked_sub(1)
            .unwrap_or_else(|| options.len().saturating_sub(1))
    };
    options.get(next).cloned()
}
