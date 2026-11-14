//! Destination discovery, launcher views, invitation state and launched children.
use crate::destinations::{Destination, Route};
use std::time::Instant;
use tehuti_core::unit_verbs::DestinationKind;

#[must_use]
pub fn destinations(
    entries: &[Destination],
    kind: DestinationKind,
    focused: Option<&str>,
) -> (Vec<tehuti_core::launcher::Destination>, Vec<Route>) {
    use crate::destinations::{Source, Target};
    let mut destinations = Vec::new();
    let mut routes = Vec::new();
    for entry in entries {
        if kind == DestinationKind::Creation
            || (kind == DestinationKind::Project && entry.source != Source::Project)
        {
            continue;
        }
        for (index, route) in entry.routes.iter().enumerate() {
            if entry.source == Source::Desktop && index > 0 {
                continue;
            }
            if duplicates_desktop_route(route, entries) {
                continue;
            }
            let open = route
                .steps
                .iter()
                .any(|step| matches!(step.target, Target::Window(_)));
            let description = describe(route, &entry.name);
            destinations.push(tehuti_core::launcher::Destination {
                name: entry.name.clone(),
                extra_names: entry.extra_names.clone(),
                route: description,
                open,
                focused: route
                    .context
                    .as_deref()
                    .is_some_and(|context| Some(context) == focused),
                safety: if route
                    .steps
                    .iter()
                    .any(|step| matches!(step.target, Target::Action(_) | Target::Menu(_)))
                {
                    tehuti_core::launcher::Safety::Changes
                } else if open {
                    tehuti_core::launcher::Safety::Focus
                } else {
                    tehuti_core::launcher::Safety::Launch
                },
            });
            routes.push(route.clone());
        }
    }
    (destinations, routes)
}

fn describe(route: &Route, name: &str) -> String {
    use crate::destinations::Target;
    route
        .steps
        .iter()
        .map(|step| match &step.target {
            Target::Desktop(value)
            | Target::Url(value)
            | Target::Application(value)
            | Target::Menu(value) => format!("{}: {value}", step.icon),
            Target::Workspace(_) => format!("{}: workspace", step.icon),
            Target::Window(_) => format!("{}: open window", step.icon),
            Target::Action(_) => format!("{}: {name}", step.icon),
        })
        .collect::<Vec<_>>()
        .join(" → ")
}

fn duplicates_desktop_route(route: &Route, entries: &[Destination]) -> bool {
    use crate::destinations::{Source, Target};
    let [step] = route.steps.as_slice() else {
        return false;
    };
    let Target::Application(application) = &step.target else {
        return false;
    };
    entries.iter().any(|entry| {
        entry.source == Source::Desktop
            && entry.routes.iter().any(|desktop_route| {
                desktop_route.steps.iter().any(|desktop_step| {
                    matches!(&desktop_step.target,
                        Target::Desktop(desktop_id)
                            if desktop_id.strip_suffix(".desktop") == Some(application.as_str()))
                })
            })
    })
}

/// The session-start invitation's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Invitation {
    /// Embedders and tests that never asked for it.
    Off,
    Ready,
    /// A pick from the invitation is starting; its window has a grace period to appear.
    Launching(Instant),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InvitationView {
    Hidden,
    Close,
    Show,
}

#[derive(Debug)]
pub(crate) struct Session {
    children: Children,
    desktop_entries: crate::desktop::EntryCache,
    paths: crate::destinations::Paths,
    table: crate::destinations::Table,
    routes: Vec<Route>,
    invitation: Invitation,
    windows: Vec<crate::destinations::Window>,
}

impl Session {
    pub(crate) fn new() -> std::io::Result<Self> {
        Ok(Self {
            children: Children::default(),
            desktop_entries: crate::desktop::EntryCache::new()?,
            paths: crate::destinations::Paths::default(),
            table: crate::destinations::Table::default(),
            routes: Vec::new(),
            invitation: Invitation::Off,
            windows: Vec::new(),
        })
    }

    pub(crate) const fn paths(&self) -> &crate::destinations::Paths {
        &self.paths
    }
    pub(crate) const fn table(&self) -> &crate::destinations::Table {
        &self.table
    }
    pub(crate) fn windows(&self) -> &[crate::destinations::Window] {
        &self.windows
    }
    pub(crate) fn configure(&mut self, paths: crate::destinations::Paths) {
        self.paths = paths;
    }

    pub(crate) fn rebuild(
        &mut self,
        contexts: &[tehuti_protocol::Context],
        hidden: &[tehuti_core::menu::Command],
        snapshot: &tehuti_core::layout::Snapshot,
    ) -> Result<Vec<(String, String)>, String> {
        let mut table = crate::destinations::Table::read(
            &self.paths.config,
            &self.paths.projects,
            &self.paths.data,
            contexts,
            &self.windows,
        )?;
        table.add_menu(hidden);
        table.update_workspaces(snapshot);
        self.table = table;
        Ok(self.table.app_names())
    }

    pub(crate) fn update_workspaces(&mut self, snapshot: &tehuti_core::layout::Snapshot) {
        self.table.update_workspaces(snapshot);
    }

    pub(crate) fn update_windows(&mut self, windows: Vec<crate::destinations::Window>) -> bool {
        if windows == self.windows {
            return false;
        }
        self.table.update_windows(&windows);
        self.windows = windows;
        true
    }

    /// Builds the launcher view. It rewrites the routes through `rows`, so
    /// indices from an earlier view no longer apply.
    pub(crate) fn show(
        &mut self,
        request: &tehuti_core::unit_verbs::LauncherRequest,
        typed: bool,
        focused: Option<&str>,
        title: &str,
        contexts: &[tehuti_protocol::Context],
        level: tehuti_core::Level,
    ) -> tehuti_core::launcher::Launcher {
        let destinations = self.rows(request.kind, focused);
        let mut launcher = if typed {
            tehuti_core::launcher::Launcher::typed(&request.query, destinations)
        } else {
            tehuti_core::launcher::Launcher::new(&request.query, destinations, None)
        };
        let active = focused
            .and_then(|app_id| tehuti_core::own_context(contexts, app_id, title))
            .map(tehuti_protocol::Context::launcher);
        let global = contexts
            .iter()
            .find(|context| context.app_id() == "*")
            .map(tehuti_protocol::Context::launcher);
        let enabled = active
            .and_then(|settings| settings.auto_pick)
            .or_else(|| global.and_then(|settings| settings.auto_pick))
            .unwrap_or(true);
        let delay = active
            .and_then(|settings| settings.delay_ms)
            .or_else(|| global.and_then(|settings| settings.delay_ms))
            .map_or(5000, core::num::NonZeroU32::get);
        launcher.configure(enabled, core::time::Duration::from_millis(u64::from(delay)));
        let verb = match request.kind {
            tehuti_core::unit_verbs::DestinationKind::Application => "Open",
            tehuti_core::unit_verbs::DestinationKind::Project => "Work on",
            tehuti_core::unit_verbs::DestinationKind::Creation => "New",
        };
        let level = match level {
            tehuti_core::Level::App => "App",
            tehuti_core::Level::Columns => "Columns",
        };
        launcher.set_scope(verb, level, focused);
        tracing::info!(query = request.query, rows = ?launcher.list().candidates(), "launcher local ranking");
        let jev = active
            .and_then(|settings| settings.jev)
            .or_else(|| global.and_then(|settings| settings.jev))
            .unwrap_or(false);
        let key_present = std::env::var_os("TYPESAFE_API_KEY").is_some_and(|key| !key.is_empty());
        if let Some(jev_request) = launcher.jev_request(jev, key_present, level, focused) {
            // The network client is not wired yet; the local ranking stands.
            tracing::info!(?jev_request, "launcher Jev request skipped: no client");
        }
        launcher
    }

    /// Rewrites the routes that `choose` and `route` index into; call it
    /// whenever the rows shown change.
    pub(crate) fn rows(
        &mut self,
        kind: DestinationKind,
        focused: Option<&str>,
    ) -> Vec<tehuti_core::launcher::Destination> {
        let (rows, routes) = destinations(self.table.entries(), kind, focused);
        self.routes = routes;
        rows
    }

    pub(crate) fn route(&self, index: usize) -> Option<&Route> {
        self.routes.get(index)
    }

    pub(crate) fn choose(&mut self, index: usize, empty: bool, now: Instant) -> Option<Route> {
        let route = self.routes.get(index)?.clone();
        if self.invitation != Invitation::Off && empty {
            self.invitation = Invitation::Launching(now);
        }
        Some(route)
    }

    pub(crate) const fn enable_invitation(&mut self) {
        self.invitation = Invitation::Ready;
    }

    pub(crate) fn invitation(&mut self, occupied: bool, blocked: bool, now: Instant) -> InvitationView {
        if self.invitation == Invitation::Off {
            return InvitationView::Hidden;
        }
        if occupied {
            self.invitation = Invitation::Ready;
            return InvitationView::Close;
        }
        let waiting = matches!(self.invitation, Invitation::Launching(since) if now.duration_since(since) < core::time::Duration::from_secs(10));
        if waiting || blocked {
            return InvitationView::Hidden;
        }
        self.routes.clear();
        InvitationView::Show
    }

    pub(crate) fn launch_failed(&mut self) {
        if self.invitation != Invitation::Off {
            self.invitation = Invitation::Ready;
        }
    }

    pub(crate) fn installed(&self) -> Vec<String> {
        self.desktop_entries.entries().to_vec()
    }

    pub(crate) fn launch_application(&mut self, entry: &str, socket: Option<&std::ffi::OsStr>) -> bool {
        let result = socket
            .ok_or_else(|| "no compositor socket".to_owned())
            .and_then(|socket| {
                if self.children.is_full() {
                    return Err("too many pending launches".into());
                }
                crate::desktop::launch(entry, socket)
            });
        match result {
            Ok(child) => {
                self.children.track(child, entry, Instant::now());
                true
            }
            Err(error) => {
                tracing::warn!(%error, "desktop launch failed");
                false
            }
        }
    }

    pub(crate) fn launch_url(&mut self, url: &str, socket: Option<&std::ffi::OsStr>) -> bool {
        let Some(socket) = socket else {
            return false;
        };
        if self.children.is_full() {
            return false;
        }
        match crate::client_env::command(std::ffi::OsStr::new("xdg-open"), socket)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => {
                self.children.track(child, "Link", Instant::now());
                true
            }
            Err(error) => {
                tracing::warn!(%error, "launcher URL failed");
                false
            }
        }
    }

    pub(crate) fn shutdown(&mut self) {
        self.children = Children::default();
    }

    pub(crate) fn poll(&mut self, now: Instant) -> Vec<Failure> {
        let failed = self.children.poll(now);
        self.desktop_entries.poll();
        failed
    }
}

/// How long after launch a non-zero exit counts as "closed right away".
const EARLY_EXIT: core::time::Duration = core::time::Duration::from_secs(5);

/// A launched child that has exited unsuccessfully.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    pub(crate) label: String,
    pub(crate) code: Option<i32>,
    /// Exited within a few seconds of the launch.
    pub(crate) early: bool,
}

impl Failure {
    /// The bar notice for an early exit.
    pub(crate) fn notice(&self) -> String {
        let exit = self.code.map_or_default(|code| format!(" (exit {code})"));
        format!("{} closed right away{exit}", self.label)
    }
}

#[derive(Debug)]
struct Child {
    process: std::process::Child,
    label: String,
    started: Instant,
}

#[derive(Debug, Default)]
struct Children(Vec<Child>);

impl Children {
    const fn is_full(&self) -> bool {
        self.0.len() >= 16
    }
    fn track(&mut self, process: std::process::Child, label: &str, now: Instant) {
        self.0.push(Child {
            process,
            label: label.to_owned(),
            started: now,
        });
    }
    fn poll(&mut self, now: Instant) -> Vec<Failure> {
        let mut failures = Vec::new();
        self.0.retain_mut(|child| match child.process.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                if !status.success() {
                    failures.push(Failure {
                        label: child.label.clone(),
                        code: status.code(),
                        early: now.saturating_duration_since(child.started) < EARLY_EXIT,
                    });
                }
                false
            }
            Err(error) => {
                tracing::warn!(%error, "desktop launcher wait failed");
                failures.push(Failure {
                    label: child.label.clone(),
                    code: None,
                    early: false,
                });
                let _killed = child.process.kill();
                let _reaped = child.process.wait();
                false
            }
        });
        failures
    }
}

impl Drop for Children {
    fn drop(&mut self) {
        for mut child in self.0.drain(..) {
            crate::shutdown::terminate(&mut child.process, false);
        }
    }
}

#[cfg(test)]
mod tests;
