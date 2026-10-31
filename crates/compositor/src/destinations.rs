//! Desktop shell destinations and their alternative routes.
use serde::{Deserialize, Serialize};
use std::{
    io::{Read as _, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
};
use tehuti_protocol::{Action, Context};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Desktop,
    Project,
    Window,
    Custom,
    Menu,
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Target {
    Desktop(String),
    Url(String),
    Action(Action),
    Window(u64),
    Application(String),
    Menu(String),
    Workspace(u64),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub icon: String,
    pub target: Target,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub name: String,
    pub extra_names: Vec<String>,
    pub routes: Vec<Route>,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub identity: u64,
    pub app_id: String,
    pub title: String,
    pub chip: String,
}

#[derive(Debug, Default)]
pub struct Table {
    entries: Vec<Destination>,
}

impl Table {
    /// Builds a table from local discovery and custom configuration.
    ///
    /// # Errors
    /// Custom configuration cannot be read or parsed.
    pub fn read(
        config: &Path,
        projects: &Path,
        data: &[PathBuf],
        contexts: &[Context],
        windows: &[Window],
    ) -> Result<Self, String> {
        let mut table = Self::default();
        let mut seen = std::collections::HashSet::new();
        for directory in data {
            desktop_entries(&directory.join("applications"), "", 0, &mut seen, &mut table);
        }
        for name in crate::speech_vocabulary::directory_names(projects) {
            let routes: Vec<_> = contexts
                .iter()
                .filter_map(|context| {
                    context
                        .mapped_speech_action(&format!("Work on {name}"))
                        .map(|action| {
                            let mut route = route("folder", Target::Action(action));
                            route.context = Some(context.app_id().into());
                            route
                        })
                })
                .collect();
            if !routes.is_empty() {
                table.insert(Destination {
                    name,
                    extra_names: Vec::new(),
                    routes,
                    source: Source::Project,
                });
            }
        }
        table.update_windows(windows);
        for custom in read_custom(config)?.destinations {
            custom.validate()?;
            table
                .entries
                .retain(|entry| !entry.name.eq_ignore_ascii_case(&custom.name));
            table.entries.push(Destination {
                name: custom.name,
                extra_names: custom.extra_names,
                routes: custom.routes,
                source: Source::Custom,
            });
        }
        Ok(table)
    }

    #[must_use]
    pub fn entries(&self) -> &[Destination] {
        &self.entries
    }

    /// Captions for configured desktop hints, preserving favourite order.
    #[must_use]
    pub fn desktop_signposts(&self, contexts: &[Context]) -> Vec<(String, String)> {
        crate::desktop_hints::launch_phrases(contexts)
            .into_iter()
            .filter_map(|(entry, _)| {
                let desktop = if entry.ends_with(".desktop") {
                    entry.clone()
                } else {
                    format!("{entry}.desktop")
                };
                self.name_for(&Target::Desktop(desktop))
                    .map(|name| (entry, name.into()))
            })
            .collect()
    }

    pub(crate) fn update_workspaces(&mut self, snapshot: &tehuti_core::layout::Snapshot) {
        self.entries.retain(|entry| entry.source != Source::Workspace);
        for (index, workspace) in snapshot
            .outputs
            .iter()
            .flat_map(|output| &output.workspaces)
            .enumerate()
        {
            self.insert(Destination {
                name: workspace
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Workspace {}", index.saturating_add(1))),
                extra_names: Vec::new(),
                routes: vec![route("desktop", Target::Workspace(workspace.workspace.0))],
                source: Source::Workspace,
            });
        }
    }

    #[must_use]
    pub fn name_for(&self, target: &Target) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| {
                entry
                    .routes
                    .iter()
                    .any(|route| route.steps.iter().any(|step| &step.target == target))
            })
            .map(|entry| entry.name.as_str())
    }

    pub(crate) fn add_menu(&mut self) {
        for command in tehuti_core::menu::ITEMS {
            self.insert(Destination {
                name: command.label().into(),
                extra_names: Vec::new(),
                routes: vec![route("system", Target::Menu(command.key().into()))],
                source: Source::Menu,
            });
        }
    }

    fn insert(&mut self, destination: Destination) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| {
            entry.source == destination.source && entry.name.eq_ignore_ascii_case(&destination.name)
        }) {
            entry.routes.extend(destination.routes);
            entry.extra_names.extend(destination.extra_names);
        } else {
            self.entries.push(destination);
        }
    }

    /// Replaces live window routes while preserving custom overrides.
    pub fn update_windows(&mut self, windows: &[Window]) {
        self.entries.retain(|entry| entry.source != Source::Window);
        for window in windows {
            let base = if window.chip.is_empty() {
                &window.title
            } else {
                &window.chip
            };
            let duplicate = windows.iter().filter(|other| other.chip == window.chip).count() > 1;
            let name = if duplicate {
                if window.title.is_empty()
                    || windows
                        .iter()
                        .filter(|other| other.chip == window.chip && other.title == window.title)
                        .count()
                        > 1
                {
                    format!("{base}: {}", window.identity)
                } else {
                    format!("{base}: {}", window.title)
                }
            } else {
                base.clone()
            };
            if self
                .entries
                .iter()
                .any(|entry| entry.source == Source::Custom && entry.name.eq_ignore_ascii_case(&name))
            {
                continue;
            }
            self.insert(Destination {
                name,
                extra_names: vec![window.title.clone()],
                routes: vec![route("application-x-executable", Target::Window(window.identity))],
                source: Source::Window,
            });
        }
    }
}

fn route(icon: &str, target: Target) -> Route {
    Route {
        context: None,
        steps: vec![Step {
            icon: icon.into(),
            target,
        }],
    }
}

fn desktop_entries(
    directory: &Path,
    prefix: &str,
    depth: usize,
    seen: &mut std::collections::HashSet<String>,
    table: &mut Table,
) {
    if depth >= 16 {
        return;
    }
    let mut children: Vec<_> = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .collect();
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let Ok(kind) = child.file_type() else {
            continue;
        };
        let Some(filename) = child.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let entry_id = format!("{prefix}{filename}");
        if kind.is_dir() {
            desktop_entries(
                &child.path(),
                &format!("{entry_id}-"),
                depth.saturating_add(1),
                seen,
                table,
            );
        } else if filename.ends_with(".desktop")
            && seen.insert(entry_id.clone())
            && let Some(destination) = desktop_destination(&child.path(), entry_id)
        {
            table.insert(destination);
        }
    }
}

fn desktop_destination(path: &Path, entry_id: String) -> Option<Destination> {
    let text = crate::speech_vocabulary::read_text(path);
    let mut fields = std::collections::HashMap::new();
    let mut in_entry = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry && let Some((key, value)) = line.split_once('=') {
            let _previous = fields.insert(key, value);
        }
    }
    if fields.get("Type") != Some(&"Application")
        || fields.get("Hidden") == Some(&"true")
        || fields.get("NoDisplay") == Some(&"true")
    {
        return None;
    }
    let name = fields.get("Name").filter(|name| !name.trim().is_empty())?;
    Some(Destination {
        name: (*name).into(),
        extra_names: fields
            .get("GenericName")
            .map(|generic| vec![(*generic).into()])
            .unwrap_or_default(),
        routes: vec![route(
            fields
                .get("Icon")
                .copied()
                .filter(|icon| !icon.trim().is_empty())
                .unwrap_or("application-x-executable"),
            Target::Desktop(entry_id),
        )],
        source: Source::Desktop,
    })
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CustomFile {
    #[serde(default)]
    destinations: Vec<Custom>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Custom {
    name: String,
    #[serde(default)]
    extra_names: Vec<String>,
    routes: Vec<Route>,
}

impl Custom {
    fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty()
            || self.name.contains('\0')
            || self.name.len() > 256
            || self
                .extra_names
                .iter()
                .any(|name| name.trim().is_empty() || name.contains('\0') || name.len() > 256)
            || self.routes.is_empty()
            || self.routes.iter().any(|route| route.steps.is_empty())
        {
            return Err("destination requires a name and nonempty routes".into());
        }
        for step in self.routes.iter().flat_map(|route| &route.steps) {
            let valid = match &step.target {
                Target::Desktop(entry) => {
                    !entry.trim().is_empty() && !entry.starts_with('-') && !entry.contains(['/', '\0'])
                }
                Target::Url(url) => {
                    (url.starts_with("https://") || url.starts_with("http://"))
                        && !url.contains(['\0', '\n', '\r', ' '])
                        && url.split_once("://").is_some_and(|(_, host)| !host.is_empty())
                }
                Target::Application(app_id) => !app_id.trim().is_empty() && !app_id.contains('\0'),
                Target::Action(_) => true,
                Target::Window(_) | Target::Menu(_) | Target::Workspace(_) => false,
            };
            if !valid || step.icon.trim().is_empty() || step.icon.contains('\0') {
                return Err("invalid destination route step".into());
            }
        }
        Ok(())
    }
}

fn read_custom(config: &Path) -> Result<CustomFile, String> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(config.join("destinations.toml"))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(CustomFile::default()),
        Err(error) => return Err(error.to_string()),
    };
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > 65536 {
        return Err("destination configuration must be a regular file within 65536 bytes".into());
    }
    let mut text = String::new();
    let _bytes = file
        .take(65537)
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    if text.len() > 65536 {
        return Err("destination configuration exceeds 65536 bytes".into());
    }
    let parsed: CustomFile = toml::from_str(&text).map_err(|error| error.to_string())?;
    for custom in &parsed.destinations {
        custom.validate()?;
    }
    Ok(parsed)
}

/// Saves an application route from a focused window, replacing the same custom name.
///
/// # Errors
/// The window has no application id or configuration cannot be read or written.
pub fn save(config: &Path, name: &str, window: &Window) -> Result<(), String> {
    if window.app_id.trim().is_empty() {
        return Err("focused window has no application id".into());
    }
    let custom = Custom {
        name: name.trim().into(),
        extra_names: Vec::new(),
        routes: vec![route(
            "application-x-executable",
            Target::Application(window.app_id.clone()),
        )],
    };
    custom.validate()?;
    let mut file = read_custom(config)?;
    file.destinations
        .retain(|entry| !entry.name.eq_ignore_ascii_case(&custom.name));
    file.destinations.push(custom);
    let text = toml::to_string(&file).map_err(|error| error.to_string())?;
    if text.len() > 65536 {
        return Err("destination configuration exceeds 65536 bytes".into());
    }
    std::fs::create_dir_all(config).map_err(|error| error.to_string())?;
    let temporary = config.join("destinations.toml.tmp");
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    if let Err(error) = output.write_all(text.as_bytes()) {
        let _cleanup = std::fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    std::fs::rename(temporary, config.join("destinations.toml")).map_err(|error| error.to_string())
}

impl Table {
    /// Names and extra names offered to the speech engine.
    #[must_use]
    pub fn vocabulary(&self) -> Vec<String> {
        Self::names(self.entries.iter())
    }

    /// Names whose routes are available in the active or global context.
    #[must_use]
    pub fn vocabulary_for(&self, context: Option<&str>) -> Vec<String> {
        Self::names(self.entries.iter().filter(|entry| {
            entry.routes.iter().any(|route| {
                route
                    .context
                    .as_deref()
                    .is_none_or(|scope| scope == "*" || Some(scope) == context)
            })
        }))
    }

    fn names<'table>(entries: impl Iterator<Item = &'table Destination>) -> Vec<String> {
        let mut entries: Vec<_> = entries.collect();
        entries.sort_by_key(|entry| match entry.source {
            Source::Custom => 0,
            Source::Project => 1,
            Source::Window => 2,
            Source::Desktop => 3,
            Source::Menu => 4,
            Source::Workspace => 5,
        });
        crate::speech_vocabulary::bounded(
            entries
                .into_iter()
                .flat_map(|entry| core::iter::once(entry.name.clone()).chain(entry.extra_names.clone()))
                .collect(),
        )
    }
}

/// Filesystem inputs for destination discovery.
#[derive(Debug)]
pub struct Paths {
    pub config: PathBuf,
    pub projects: PathBuf,
    pub data: Vec<PathBuf>,
}

impl Default for Paths {
    fn default() -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map_or_else(|| home.join(".config"), PathBuf::from)
            .join("tehuti");
        let mut data =
            vec![std::env::var_os("XDG_DATA_HOME").map_or_else(|| home.join(".local/share"), PathBuf::from)];
        data.extend(std::env::split_paths(
            &std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into()),
        ));
        Self {
            config,
            projects: home.join("Projects"),
            data,
        }
    }
}
