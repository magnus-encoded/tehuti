//! Plugin processes and their non-blocking stdio sources.

use crate::{
    plugin_session::{Backoff, Delivery, Session, Status},
    state::Compositor,
};
use core::{
    cell::{Cell, RefCell},
    time::Duration,
};
use smithay::reexports::calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic};
use std::{
    io::{self, Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Stdio},
    rc::Rc,
    time::Instant,
};
use tehuti_protocol::{
    Action, Call, Event, Manifest, Method, Permission, PluginKind, RpcVersion, StartPolicy, Subscription,
};

const SUPERVISION_INTERVAL: Duration = Duration::from_millis(10);
const READ_ATTEMPTS: usize = 16;

#[derive(Debug)]
struct Process {
    child: Option<Child>,
}

impl Process {
    /// Why the child has stopped, if it has.
    fn exit_reason(&mut self) -> Option<String> {
        match self.child.as_mut()?.try_wait() {
            Ok(Some(status)) => Some(format!("plugin exited: {status}")),
            Ok(None) => None,
            Err(error) => Some(format!("plugin status failed: {error}")),
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            crate::shutdown::terminate(&mut child, false);
        }
    }
}

/// A started plugin: its protocol session and the process-side handles.
struct Running {
    session: Rc<RefCell<Session>>,
    /// The child process, when there is one; it ends with the running plugin.
    process: Option<Process>,
    wake: Box<dyn Fn()>,
    cleanup: Box<dyn Fn()>,
}

type Start = Box<dyn Fn(&Manifest) -> io::Result<Running>>;
struct Entry {
    manifest: Manifest,
    plugin: usize,
    latest_focus: Option<(Option<String>, String)>,
    started: bool,
    restart: Backoff,
    next_id: u64,
    cleanup: Option<Box<dyn Fn()>>,
    session: Option<Rc<RefCell<Session>>>,
    process: Option<Process>,
    wake: Option<Box<dyn Fn()>>,
}

impl Entry {
    fn new(manifest: Manifest, plugin: usize) -> Self {
        Self {
            manifest,
            plugin,
            latest_focus: None,
            started: false,
            restart: Backoff::new(),
            next_id: 1,
            cleanup: None,
            session: None,
            process: None,
            wake: None,
        }
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
    }
}

pub struct Plugins {
    entries: Vec<Entry>,
    start: Start,
    focus: Option<String>,
    pad: tehuti_core::vpad::Filter,
    cleanup_timer: Box<dyn Fn()>,
    /// The verb route `active` found for the utterance `dispatch` sends next.
    routed_verb: Option<(Event, tehuti_core::verbs::VerbMatch)>,
}
impl Drop for Plugins {
    fn drop(&mut self) {
        (self.cleanup_timer)();
    }
}

impl core::fmt::Debug for Plugins {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Plugins")
            .field("count", &self.entries.len())
            .finish_non_exhaustive()
    }
}

impl Plugins {
    /// Initialized plugins that match the focused application, in manifest order.
    fn focused_ready(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|entry| {
            self.focus
                .as_ref()
                .is_some_and(|app_id| entry.manifest.app_ids.contains(app_id))
                && entry
                    .session
                    .as_ref()
                    .is_some_and(|session| session.borrow().status == Status::Ready)
        })
    }

    /// The powerline status of the first focused plugin that set one.
    pub(crate) fn status(&self) -> Option<String> {
        self.focused_ready()
            .find_map(|entry| entry.session.as_ref()?.borrow().status_text.clone())
    }

    /// Invoke words of the initialized application plugins that cover `app_id`, in manifest order.
    pub(crate) fn invoke_words(&self, app_id: &str) -> Vec<String> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.manifest.kind == PluginKind::Application
                    && entry.manifest.app_ids.iter().any(|name| name == app_id)
            })
            .filter_map(|entry| entry.session.as_ref())
            .filter(|session| session.borrow().status == Status::Ready)
            .flat_map(|session| session.borrow().invoke_words.clone())
            .collect()
    }

    /// Hotwords of the initialized application plugins that cover `app_id`, in manifest order.
    pub(crate) fn speech_hints(&self, app_id: &str) -> Vec<String> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.manifest.kind == PluginKind::Application
                    && entry.manifest.app_ids.iter().any(|name| name == app_id)
            })
            .filter_map(|entry| entry.session.as_ref())
            .filter(|session| session.borrow().status == Status::Ready)
            .flat_map(|session| session.borrow().hints.clone())
            .collect()
    }

    pub(crate) fn set_pad_overrides(&mut self, plugin: usize, overrides: Vec<tehuti_protocol::PadOverride>) {
        self.pad.set(plugin, overrides);
    }

    pub(crate) fn reset_pad(&mut self) {
        self.pad.reset_held();
    }

    /// Whether a plugin consumed the held `button`; reads only.
    pub(crate) fn pad_consumed(&self, button: tehuti_protocol::ButtonName) -> bool {
        self.pad.consumed(button)
    }

    /// Applies virtual pad overrides to one physical button; true keeps it from the application.
    pub(crate) fn pad_button(
        &mut self,
        button: tehuti_protocol::ButtonName,
        state: tehuti_protocol::ButtonState,
        push_to_talk: &[tehuti_protocol::ButtonName],
    ) -> bool {
        let eligible: Vec<usize> = self.focused_ready().map(|entry| entry.plugin).collect();
        let tehuti_core::vpad::Verdict::Plugin(plugin) =
            self.pad.step(button, state, &eligible, push_to_talk)
        else {
            return false;
        };
        if let Some(entry) = self.entries.iter().find(|entry| entry.plugin == plugin)
            && let Some(session) = &entry.session
        {
            if let Err(error) = session.borrow_mut().queue(&Call {
                jsonrpc: RpcVersion::Two,
                request_id: None,
                method: Method::PadInput { button, state },
            }) {
                tracing::warn!(%error, "plugin pad input dropped");
            }
            if let Some(wake) = &entry.wake {
                wake();
            }
        }
        true
    }

    pub(crate) fn adjusters(&self) -> Vec<(usize, tehuti_protocol::Adjuster)> {
        self.entries
            .iter()
            .filter(|entry| {
                self.focus
                    .as_ref()
                    .is_some_and(|app_id| entry.manifest.app_ids.contains(app_id))
                    && entry
                        .session
                        .as_ref()
                        .is_some_and(|session| session.borrow().status == Status::Ready)
            })
            .flat_map(|entry| {
                entry
                    .manifest
                    .adjusters
                    .iter()
                    .cloned()
                    .map(|spec| (entry.plugin, spec))
            })
            .collect()
    }

    pub(crate) fn adjuster_value(&mut self, plugin: usize, name: String, value: i32, finished: bool) {
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.plugin == plugin) else {
            return;
        };
        let Some(spec) = entry
            .manifest
            .adjusters
            .iter_mut()
            .find(|spec| spec.name() == name)
        else {
            return;
        };
        spec.set_value(value);
        if let Some(session) = &entry.session {
            let mut session = session.borrow_mut();
            if session.status == Status::Ready {
                if let Err(error) = session.queue(&Call {
                    jsonrpc: RpcVersion::Two,
                    request_id: None,
                    method: Method::Adjuster {
                        name,
                        value,
                        finished,
                    },
                }) {
                    tracing::warn!(%error, "plugin adjuster update dropped");
                }
                if let Some(wake) = &entry.wake {
                    wake();
                }
            }
        }
    }

    #[must_use]
    pub fn initialized_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| {
                entry.session.as_ref().is_some_and(|session| {
                    let session = session.borrow();
                    session.status == Status::Ready
                })
            })
            .count()
    }

    fn start_entry(start: &Start, entry: &mut Entry, now: Instant) {
        if entry.session.is_none() && entry.restart.due(now) {
            entry.started = true;
            entry.restart.started();
            match start(&entry.manifest) {
                Ok(Running {
                    session,
                    process,
                    wake,
                    cleanup,
                }) => {
                    {
                        let mut session = session.borrow_mut();
                        session.next_id = entry.next_id;
                        session.plugin = entry.plugin;
                        session.latest_focus.clone_from(&entry.latest_focus);
                    }
                    entry.cleanup = Some(cleanup);
                    entry.session = Some(session);
                    entry.process = process;
                    entry.wake = Some(wake);
                }
                Err(error) => {
                    tracing::warn!(%error, "plugin start failed");
                    Self::schedule_restart(entry, now);
                }
            }
        }
    }

    fn schedule_restart(entry: &mut Entry, now: Instant) {
        tracing::warn!(executable = %entry.manifest.executable, delay_ms = entry.restart.delay.as_millis(), "plugin restart backoff");
        entry.restart.schedule(now);
    }

    pub(crate) fn supervise(&mut self) -> Vec<usize> {
        self.supervise_at(Instant::now())
    }

    fn supervise_at(&mut self, now: Instant) -> Vec<usize> {
        let mut exited = Vec::new();
        for entry in &mut self.entries {
            let failed = entry.session.as_ref().is_some_and(|session| {
                let mut session = session.borrow_mut();
                let exit = entry.process.as_mut().and_then(Process::exit_reason);
                let failed = session.supervise_child(now, exit.as_deref(), &mut entry.restart);
                entry.next_id = session.next_id;
                failed
            });
            if failed {
                exited.push(entry.plugin);
                self.pad.clear(entry.plugin);
                if let Some(cleanup) = entry.cleanup.take() {
                    cleanup();
                }
                entry.wake = None;
                entry.session = None;
                entry.process = None;
                Self::schedule_restart(entry, now);
            }
            if entry.started {
                Self::start_entry(&self.start, entry, now);
            }
        }
        exited
    }

    pub(crate) fn active(&mut self, event: &Event) -> bool {
        if let Event::Focus { app_id, .. } = event {
            self.focus.clone_from(app_id);
        }
        for entry in &mut self.entries {
            if let Event::Focus { app_id, title } = event {
                entry.latest_focus = Some((app_id.clone(), title.clone()));
            }
            if !entry.started
                && (entry.manifest.start == StartPolicy::Always
                    || self
                        .focus
                        .as_ref()
                        .is_some_and(|app_id| entry.manifest.app_ids.contains(app_id)))
            {
                Self::start_entry(&self.start, entry, Instant::now());
            }
        }
        for entry in &self.entries {
            if let Some(session) = &entry.session {
                session.borrow_mut().expire_initialize(Instant::now());
            }
        }
        self.routed_verb = self.verb_route(event).map(|matched| (event.clone(), matched));
        self.routed_verb.is_some() || self.entries.iter().any(|entry| self.matches(entry, event))
    }

    /// The ready verb plugin whose verb begins this speech, checked before application plugins.
    fn verb_route(&self, event: &Event) -> Option<tehuti_core::verbs::VerbMatch> {
        let Event::Speech { text } = event else {
            return None;
        };
        let owners: Vec<(usize, Vec<String>)> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.manifest.kind == PluginKind::Verb)
            .filter_map(|(index, entry)| {
                let session = entry.session.as_ref()?.borrow();
                (session.status == Status::Ready && session.events.contains(&Subscription::Speech))
                    .then(|| (index, session.verbs.clone()))
            })
            .collect();
        tehuti_core::verbs::route(
            owners.iter().map(|(index, verbs)| (*index, verbs.as_slice())),
            text.as_str(),
        )
    }

    /// Sends a verb request to its owner alone.
    fn dispatch_verb(
        &self,
        matched: tehuti_core::verbs::VerbMatch,
    ) -> Option<(tehuti_core::plugin::RequestKey, Vec<Permission>)> {
        let entry = self.entries.get(matched.plugin)?;
        let mut session = entry.session.as_ref()?.borrow_mut();
        session.plugin = matched.plugin;
        let request_id = session.request()?;
        let call = Call {
            jsonrpc: RpcVersion::Two,
            request_id: Some(request_id),
            method: Method::Verb {
                verb: matched.verb,
                text: matched.text,
            },
        };
        if let Err(error) = session.queue_event(&call, Instant::now()) {
            tracing::warn!(%error, "plugin verb dropped");
            return None;
        }
        let permissions = session.manifest.permissions.clone();
        drop(session);
        if let Some(wake) = &entry.wake {
            wake();
        }
        Some((
            tehuti_core::plugin::RequestKey {
                plugin: matched.plugin,
                request_id,
            },
            permissions,
        ))
    }

    /// Whether an application plugin serves the focused application for this event.
    fn matches(&self, entry: &Entry, event: &Event) -> bool {
        entry.manifest.kind == PluginKind::Application
            && self
                .focus
                .as_ref()
                .is_some_and(|app_id| entry.manifest.app_ids.contains(app_id))
            && entry.session.as_ref().is_some_and(|session| {
                let session = session.borrow();
                session.status == Status::Ready
                    && session.events.contains(&match event {
                        Event::Speech { .. } => Subscription::Speech,
                        Event::Button { .. } | Event::Focus { .. } | Event::Tick { .. } => {
                            Subscription::Button
                        }
                    })
            })
    }

    pub(crate) fn dispatch(
        &mut self,
        event: &Event,
        route_button: bool,
    ) -> Option<(tehuti_core::plugin::RequestKey, Vec<Permission>)> {
        let routed = self
            .routed_verb
            .take()
            .filter(|(seen, _)| seen == event)
            .map(|(_, matched)| matched);
        if route_button && let Some(matched) = routed {
            return self.dispatch_verb(matched);
        }
        let mut request = None;
        let selected = self.entries.iter().position(|entry| self.matches(entry, event));
        for (index, entry) in self.entries.iter_mut().enumerate() {
            let Some(session) = &entry.session else {
                continue;
            };
            let mut session = session.borrow_mut();
            session.plugin = index;
            session.observe(event);
            if session.status != Status::Ready {
                continue;
            }
            let Some(call) = session.call_for_event(event, route_button && selected == Some(index)) else {
                continue;
            };
            let request_id = call.request_id;
            if let Err(error) = session.queue_event(&call, Instant::now()) {
                tracing::warn!(%error, "plugin event dropped");
                continue;
            }
            if let Some(request_id) = request_id {
                request = Some((
                    tehuti_core::plugin::RequestKey {
                        plugin: index,
                        request_id,
                    },
                    session.manifest.permissions.clone(),
                ));
            }
            if let Some(wake) = &entry.wake {
                wake();
            }
        }
        request
    }
}

/// Installs plugin manifests and non-blocking pipe sources on the compositor loop.
///
/// # Errors
/// The manifest directory cannot be read.
pub fn install<Data: 'static>(
    event_loop: &EventLoop<'static, Data>,
    directory: &Path,
    compositor: fn(&mut Data) -> &mut Compositor,
) -> io::Result<Plugins> {
    let handle = event_loop.handle();
    let start_handle = handle.clone();
    let start: Start = Box::new(move |manifest| start_process(&start_handle, manifest, compositor));
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        paths.push(entry?.path());
    }
    paths.sort();
    let mut entries = Vec::new();
    for path in paths {
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        match read_manifest(&path) {
            Ok(mut manifest) => {
                if Path::new(&manifest.executable).is_relative() {
                    manifest.executable = directory
                        .join(&manifest.executable)
                        .to_string_lossy()
                        .into_owned();
                }
                let mut entry = Entry::new(manifest, entries.len());
                if entry.manifest.start == StartPolicy::Always {
                    Plugins::start_entry(&start, &mut entry, Instant::now());
                }
                entries.push(entry);
            }
            Err(error) => tracing::warn!(?path, %error, "plugin manifest skipped"),
        }
    }
    let timer_token = handle
        .insert_source(
            smithay::reexports::calloop::timer::Timer::from_duration(SUPERVISION_INTERVAL),
            move |_, (), data| {
                compositor(data).supervise_plugins();
                smithay::reexports::calloop::timer::TimeoutAction::ToDuration(SUPERVISION_INTERVAL)
            },
        )
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(Plugins {
        entries,
        start,
        focus: None,
        pad: tehuti_core::vpad::Filter::default(),
        cleanup_timer: Box::new(move || handle.remove(timer_token)),
        routed_verb: None,
    })
}

fn read_manifest(path: &Path) -> io::Result<Manifest> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("manifest must be a regular file"));
    }
    let mut bytes = Vec::new();
    let _count = file.take(65_537).read_to_end(&mut bytes)?;
    tehuti_protocol::parse_manifest(&bytes).map_err(io::Error::other)
}

fn create_session(
    manifest: &Manifest,
) -> io::Result<(Rc<RefCell<Session>>, Process, ChildStdin, ChildStdout)> {
    let child = crate::child_process::command(&manifest.executable)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut process = Process { child: Some(child) };
    let running_child = process
        .child
        .as_mut()
        .ok_or_else(|| io::Error::other("plugin process missing"))?;
    let stdin = running_child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("plugin stdin missing"))?;
    let stdout = running_child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("plugin stdout missing"))?;
    rustix::fs::fcntl_setfl(&stdin, rustix::fs::OFlags::NONBLOCK)?;
    rustix::fs::fcntl_setfl(&stdout, rustix::fs::OFlags::NONBLOCK)?;
    let session = Rc::new(RefCell::new(Session::new(manifest, Instant::now())?));
    Ok((session, process, stdin, stdout))
}

/// Writes what the session has queued to the plugin's stdin until the pipe is full.
fn write_ready(
    session: &mut Session,
    stdin: &mut ChildStdin,
    writer_disabled: &Cell<bool>,
) -> io::Result<PostAction> {
    while !session.output.is_empty() {
        match stdin.write(session.output_slice()) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "plugin stdin closed")),
            Ok(count) => session.written(count),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(PostAction::Continue),
            Err(error) => return Err(error),
        }
    }
    writer_disabled.set(true);
    Ok(PostAction::Disable)
}

fn start_process<Data: 'static>(
    handle: &smithay::reexports::calloop::LoopHandle<'static, Data>,
    manifest: &Manifest,
    compositor: fn(&mut Data) -> &mut Compositor,
) -> io::Result<Running> {
    let (session, process, mut stdin, stdout) = create_session(manifest)?;
    let writer = session.clone();
    let writer_disabled = Rc::new(Cell::new(false));
    let writer_flag = writer_disabled.clone();
    let descriptor = rustix::io::dup(&stdin)?;
    let token = handle
        .insert_source(
            Generic::new(descriptor, Interest::WRITE, Mode::Level),
            move |_, _, data| {
                let result = write_ready(&mut writer.borrow_mut(), &mut stdin, &writer_flag);
                Ok(result.unwrap_or_else(|error| {
                    tracing::warn!(%error, "plugin write failed");
                    writer.borrow_mut().fail("plugin pipe write failed");
                    compositor(data).plugin_exited(writer.borrow().plugin);
                    PostAction::Remove
                }))
            },
        )
        .map_err(|error| io::Error::other(error.to_string()))?;
    let reader_token = match insert_reader(
        handle,
        stdout,
        session.clone(),
        writer_disabled.clone(),
        token,
        compositor,
    ) {
        Ok(reader_token) => reader_token,
        Err(error) => {
            handle.remove(token);
            return Err(error);
        }
    };
    let wake_handle = handle.clone();
    let cleanup_handle = handle.clone();
    Ok(Running {
        session,
        process: Some(process),
        wake: Box::new(move || {
            if writer_disabled.get() {
                match wake_handle.enable(&token) {
                    Ok(()) => writer_disabled.set(false),
                    Err(error) => tracing::warn!(%error, "plugin writer unavailable"),
                }
            }
        }),
        cleanup: Box::new(move || {
            cleanup_handle.remove(token);
            cleanup_handle.remove(reader_token);
        }),
    })
}

fn insert_reader<Data: 'static>(
    handle: &smithay::reexports::calloop::LoopHandle<'static, Data>,
    stdout: ChildStdout,
    session: Rc<RefCell<Session>>,
    writer_disabled: Rc<Cell<bool>>,
    writer_token: smithay::reexports::calloop::RegistrationToken,
    compositor: fn(&mut Data) -> &mut Compositor,
) -> io::Result<smithay::reexports::calloop::RegistrationToken> {
    let reader = session;
    let writer_handle = handle.downgrade();
    let timer_handle = handle.clone();
    let reader_token = handle
        .insert_source(
            Generic::new(
                std::fs::File::from(std::os::fd::OwnedFd::from(stdout)),
                Interest::READ,
                Mode::Level,
            ),
            move |_, source, data| {
                let action = read_ready(&reader, source, data, compositor, &timer_handle);
                let reader_session = reader.borrow();
                if !reader_session.output.is_empty()
                    && writer_disabled.get()
                    && let Some(writer_handle) = writer_handle.upgrade()
                {
                    match writer_handle.enable(&writer_token) {
                        Ok(()) => writer_disabled.set(false),
                        Err(error) => tracing::warn!(%error, "plugin writer unavailable"),
                    }
                }
                Ok(action)
            },
        )
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(reader_token)
}

fn read_ready<Data: 'static>(
    reader: &Rc<RefCell<Session>>,
    mut source: &std::fs::File,
    data: &mut Data,
    compositor_of: fn(&mut Data) -> &mut Compositor,
    handle: &smithay::reexports::calloop::LoopHandle<'static, Data>,
) -> PostAction {
    let compositor = compositor_of(data);
    let mut bytes = [0; 4096];
    for _attempt in 0..READ_ATTEMPTS {
        if reader.borrow().status == Status::Failed {
            compositor.plugin_exited(reader.borrow().plugin);
            return PostAction::Remove;
        }
        match source.read(&mut bytes) {
            Ok(0) => {
                reader.borrow_mut().fail("plugin stdout closed");
                compositor.plugin_exited(reader.borrow().plugin);
                return PostAction::Remove;
            }
            Ok(count) => {
                if !reader.borrow_mut().accept_bytes(count, Instant::now()) {
                    compositor.plugin_exited(reader.borrow().plugin);
                    return PostAction::Remove;
                }
                let deliveries = reader
                    .borrow_mut()
                    .receive(bytes.get(..count).unwrap_or_default(), Instant::now());
                compositor.advance_time();
                deliver(reader, compositor, deliveries, &mut |rest| {
                    schedule_rest(handle, compositor_of, rest);
                });
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                return PostAction::Continue;
            }
            Err(error) => {
                tracing::warn!(%error, "plugin read failed");
                reader.borrow_mut().fail("plugin pipe read failed");
                compositor.plugin_exited(reader.borrow().plugin);
                return PostAction::Remove;
            }
        }
    }
    PostAction::Continue
}

/// How long a key action waits before text that follows it in one list, so the
/// client can react to the key (for example, open a chat box) first.
const KEY_TEXT_GAP: Duration = Duration::from_millis(50);

/// Splits `actions` after the first key action directly followed by a text
/// action: the first part applies now, the rest after [`KEY_TEXT_GAP`].
fn split_at_key_text(mut actions: Vec<Action>) -> (Vec<Action>, Vec<Action>) {
    let split = actions
        .windows(2)
        .position(|pair| matches!(pair, [Action::Key { .. }, Action::Text { .. }]));
    match split {
        Some(index) => {
            let rest = actions.split_off(index.saturating_add(1));
            (actions, rest)
        }
        None => (actions, Vec::new()),
    }
}

/// Applies the part of `actions` due now and hands any delayed rest to `later`.
fn apply_notice(compositor: &mut Compositor, actions: Vec<Action>, later: &mut dyn FnMut(Vec<Action>)) {
    let (now, rest) = split_at_key_text(actions);
    for action in now {
        compositor.apply_plugin_action(&action);
    }
    if !rest.is_empty() {
        later(rest);
    }
}

fn schedule_rest<Data: 'static>(
    handle: &smithay::reexports::calloop::LoopHandle<'static, Data>,
    compositor_of: fn(&mut Data) -> &mut Compositor,
    rest: Vec<Action>,
) {
    let next = handle.clone();
    let mut pending = Some(rest);
    let inserted = handle.insert_source(
        smithay::reexports::calloop::timer::Timer::from_duration(KEY_TEXT_GAP),
        move |_, (), data| {
            if let Some(actions) = pending.take() {
                apply_notice(compositor_of(data), actions, &mut |later| {
                    schedule_rest(&next, compositor_of, later);
                });
            }
            smithay::reexports::calloop::timer::TimeoutAction::Drop
        },
    );
    if let Err(error) = inserted {
        tracing::warn!(%error, "plugin delayed actions dropped");
    }
}

fn deliver(
    reader: &Rc<RefCell<Session>>,
    compositor: &mut Compositor,
    deliveries: Vec<Delivery>,
    later: &mut dyn FnMut(Vec<Action>),
) {
    for delivery in deliveries {
        match delivery {
            Delivery::Reply {
                key,
                result,
                submission_id,
            } => {
                let already_invalid = result.is_err();
                let rejection = compositor.plugin_reply(key, result);
                if rejection.is_some() {
                    tracing::warn!("plugin action rejected");
                    if !already_invalid {
                        reader.borrow_mut().invalid(Instant::now());
                    }
                }
                let response = if let Some(request_id) = submission_id {
                    let value = rejection.map_or_else(
                                    || serde_json::json!({"jsonrpc":"2.0", "id":request_id, "result":[]}),
                                    |rejected| serde_json::json!({"jsonrpc":"2.0", "id":request_id, "error":{"code":-32602, "message":rejected.reason}}));
                    serde_json::to_vec(&value)
                        .map_err(io::Error::from)
                        .and_then(|response_bytes| reader.borrow_mut().queue_bytes(response_bytes))
                } else if let Some(rejected) = rejection {
                    reader.borrow_mut().reject(Some(key.request_id), &rejected.reason)
                } else {
                    Ok(())
                };
                if let Err(error) = response {
                    tracing::warn!(%error, "plugin rejection dropped");
                }
            }
            Delivery::Handled(key) => compositor.plugin_handled(key),
            Delivery::PadOverrides(overrides) => {
                let plugin = reader.borrow().plugin;
                compositor.set_pad_overrides(plugin, overrides);
            }
            Delivery::SpeechHints => compositor.refresh_speech_vocabulary(),
            Delivery::Notice(actions) => apply_notice(compositor, actions, later),
        }
    }
}

fn fuzz_manifest() -> Manifest {
    Manifest {
        adjusters: Vec::new(),
        plugin_id: None,
        author: None,
        security_contact: None,
        kind: PluginKind::Application,
        verbs: Vec::new(),
        executable: "plugin".into(),
        app_ids: vec!["fuzz".into()],
        events: vec![Subscription::Button, Subscription::Speech, Subscription::Focus],
        start: StartPolicy::Always,
        permissions: vec![Permission::Key, Permission::Text],
    }
}

/// Feeds arbitrary bytes to one plugin session as if they were its stdout and routes the replies
/// through the core. Fuzzing seam only; the core must never panic on any input.
///
/// The first byte picks the session state and the read chunk size.
#[doc(hidden)]
pub fn fuzz_stdout(bytes: &[u8]) {
    let Some((&control, stdout)) = bytes.split_first() else {
        return;
    };
    let now = Instant::now();
    let manifest = fuzz_manifest();
    let Ok(mut session) = Session::new(&manifest, now) else {
        return;
    };
    session.status = if control.is_multiple_of(2) {
        Status::Starting
    } else {
        Status::Ready
    };
    session.events.clone_from(&manifest.events);
    let chunk = usize::from(control.checked_shr(1).unwrap_or(0)).max(1);
    let mut core = tehuti_core::State::new();
    for part in stdout.chunks(chunk) {
        if !session.accept_bytes(part.len(), now) {
            break;
        }
        for delivery in session.receive(part, now) {
            if let Delivery::Reply { key, result, .. } = delivery {
                let (next, _actions, _rejection) =
                    tehuti_core::plugin::step(core, tehuti_core::plugin::Input::Reply { key, result });
                core = next;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_session::RESTART_BACKOFF_BASE;
    use tehuti_protocol::Message;

    fn key(name: &str) -> Action {
        serde_json::from_value(serde_json::json!({"type":"key","key":name})).unwrap()
    }

    #[test]
    fn focus_notices_reach_only_plugins_that_list_the_app() {
        let mut session = Session::new(&fuzz_manifest(), Instant::now()).unwrap();
        session.events = vec![Subscription::Focus];
        let focus = |app_id: Option<&str>| Event::Focus {
            app_id: app_id.map(Into::into),
            title: "private title".into(),
        };
        assert!(
            session.call_for_event(&focus(Some("other")), true).is_none(),
            "another app"
        );
        assert!(session.call_for_event(&focus(None), true).is_none(), "no app");
        assert!(
            session.call_for_event(&focus(Some("fuzz")), true).is_some(),
            "its own app"
        );
    }

    #[test]
    fn the_handshake_replays_focus_only_for_a_listed_app() {
        for (app, replayed) in [("other", false), ("fuzz", true)] {
            let mut session = Session::new(&fuzz_manifest(), Instant::now()).unwrap();
            session.initialize_deadline = Instant::now() + Duration::from_secs(60);
            session.latest_focus = Some((Some(app.into()), "private title".into()));
            let reply = "{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{\"protocol_version\":0,\"encoding\":\"json\",\"events\":[\"focus\"]}}\n";
            drop(session.receive(reply.as_bytes(), Instant::now()));
            assert_eq!(session.status, Status::Ready);
            let sent = String::from_utf8(session.output.iter().copied().collect()).unwrap();
            assert_eq!(sent.contains("private title"), replayed, "{app}: {sent}");
        }
    }

    fn text(value: &str) -> Action {
        serde_json::from_value(serde_json::json!({"type":"text","text":value})).unwrap()
    }

    #[test]
    fn text_after_a_key_waits_for_the_gap_in_order() {
        let (now, rest) = split_at_key_text(vec![key("slash"), text("hello"), key("enter")]);
        assert_eq!(now, vec![key("slash")]);
        assert_eq!(rest, vec![text("hello"), key("enter")]);
        let (later, after) = split_at_key_text(rest);
        assert_eq!(later, vec![text("hello"), key("enter")]);
        assert_eq!(after, Vec::new());
    }

    #[test]
    fn lists_without_key_then_text_apply_at_once() {
        for actions in [
            vec![key("tab"), key("enter")],
            vec![text("hi"), key("enter")],
            vec![text("hi")],
            Vec::new(),
        ] {
            let (now, rest) = split_at_key_text(actions.clone());
            assert_eq!(now, actions);
            assert_eq!(rest, Vec::new());
        }
    }

    #[test]
    fn plugins_cannot_request_secret_delivery() {
        let manifest = Manifest {
            adjusters: Vec::new(),
            plugin_id: None,
            author: None,
            security_contact: None,
            kind: tehuti_protocol::PluginKind::Application,
            verbs: Vec::new(),
            executable: "plugin".into(),
            app_ids: vec!["test".into()],
            events: Vec::new(),
            start: StartPolicy::Always,
            permissions: vec![Permission::Key, Permission::Text, Permission::Quit],
        };
        let request = Action::SecretRequest {
            reference: tehuti_protocol::SecretReference { entry: "mail".into() },
        };

        assert!(tehuti_core::plugin::validate_actions(vec![request], &manifest.permissions).is_err());
    }

    /// Reference-model tests for restart supervision on a virtual clock.
    mod supervision {
        use super::*;
        use proptest::prelude::*;

        const PLUGINS: usize = 3;

        fn manifest(index: usize) -> Manifest {
            Manifest {
                adjusters: Vec::new(),
                plugin_id: None,
                author: None,
                security_contact: None,
                kind: tehuti_protocol::PluginKind::Application,
                verbs: Vec::new(),
                executable: format!("plugin-{index}"),
                app_ids: vec!["test".into()],
                events: Vec::new(),
                start: StartPolicy::Always,
                permissions: Vec::new(),
            }
        }

        fn later(now: Instant, millis: u64) -> Instant {
            now.checked_add(Duration::from_millis(millis)).unwrap()
        }

        /// A ready session with nothing behind it: the tests feed it bytes and a virtual clock.
        fn ready_session(manifest: &Manifest) -> io::Result<Rc<RefCell<Session>>> {
            let mut session = Session::new(manifest, Instant::now())?;
            session.status = Status::Ready;
            session.output.clear();
            Ok(Rc::new(RefCell::new(session)))
        }

        /// Plugins whose starts succeed when `succeed[index]` is set and are counted in `starts`.
        fn plugins(succeed: &Rc<RefCell<Vec<bool>>>, starts: &Rc<RefCell<Vec<usize>>>) -> Plugins {
            let succeed = Rc::clone(succeed);
            let starts = Rc::clone(starts);
            let start: Start = Box::new(move |manifest: &Manifest| {
                let index: usize = manifest
                    .executable
                    .trim_start_matches("plugin-")
                    .parse()
                    .map_err(io::Error::other)?;
                let count = starts.borrow()[index].saturating_add(1);
                starts.borrow_mut()[index] = count;
                if !succeed.borrow()[index] {
                    return Err(io::Error::other("start refused"));
                }
                Ok(Running {
                    session: ready_session(manifest)?,
                    process: None,
                    wake: Box::new(|| {}),
                    cleanup: Box::new(|| {}),
                })
            });
            let entries = (0..PLUGINS)
                .map(|index| {
                    let mut entry = Entry::new(manifest(index), index);
                    entry.started = true;
                    entry
                })
                .collect();
            Plugins {
                entries,
                start,
                focus: None,
                pad: tehuti_core::vpad::Filter::default(),
                cleanup_timer: Box::new(|| {}),
                routed_verb: None,
            }
        }

        /// Marks freshly started sessions healthy from the virtual `now`.
        fn mark_healthy(plugins: &Plugins, now: Instant) {
            for entry in &plugins.entries {
                if let Some(session) = &entry.session {
                    let mut session = session.borrow_mut();
                    if session.healthy_since.is_none() {
                        session.healthy_since = Some(now);
                    }
                }
            }
        }

        #[derive(Debug, Clone)]
        struct ModelEntry {
            running: bool,
            broken: bool,
            healthy_since: Option<Instant>,
            restart_at: Option<Instant>,
            backoff: Duration,
            starts: usize,
        }

        impl ModelEntry {
            const fn new() -> Self {
                Self {
                    running: false,
                    broken: false,
                    healthy_since: None,
                    restart_at: None,
                    backoff: Duration::from_millis(100),
                    starts: 0,
                }
            }

            fn schedule(&mut self, now: Instant) {
                self.restart_at = now.checked_add(self.backoff);
                self.backoff = self.backoff.saturating_mul(2).min(Duration::from_secs(30));
            }

            fn supervise(&mut self, now: Instant, succeed: bool) {
                if self.running {
                    if self.broken {
                        self.running = false;
                        self.broken = false;
                        self.schedule(now);
                    } else if self
                        .healthy_since
                        .is_some_and(|since| now.duration_since(since) >= Duration::from_secs(30))
                    {
                        self.backoff = Duration::from_millis(100);
                    }
                }
                if !self.running && self.restart_at.is_none_or(|deadline| now >= deadline) {
                    self.starts = self.starts.saturating_add(1);
                    self.restart_at = None;
                    if succeed {
                        self.running = true;
                        self.healthy_since = Some(now);
                    } else {
                        self.schedule(now);
                    }
                }
            }
        }

        #[derive(Debug, Clone)]
        enum Operation {
            Advance(u64),
            Crash(usize),
            Hang(usize),
            StartsSucceed(usize, bool),
        }

        fn operation() -> impl Strategy<Value = Operation> {
            prop_oneof![
                4 => (0_u64..40_000).prop_map(Operation::Advance),
                2 => (0..PLUGINS).prop_map(Operation::Crash),
                1 => (0..PLUGINS).prop_map(Operation::Hang),
                2 => (0..PLUGINS, any::<bool>()).prop_map(|(index, succeeds)| Operation::StartsSucceed(index, succeeds)),
            ]
        }

        fn break_session(plugins: &Plugins, index: usize, hang: bool, now: Instant) {
            if let Some(session) = &plugins.entries[index].session {
                let mut session = session.borrow_mut();
                if hang {
                    // A request whose deadline has already passed: the plugin never answered.
                    let _deadline = session.pending.insert(0, now);
                    session.missed_decisions = 2;
                } else {
                    session.fail("simulated crash");
                }
            }
        }

        fn verb_plugins(verbs: &[&str]) -> Plugins {
            let succeed = Rc::new(RefCell::new(vec![true; PLUGINS]));
            let starts = Rc::new(RefCell::new(vec![0; PLUGINS]));
            let mut plugins = plugins(&succeed, &starts);
            let first = &mut plugins.entries[0];
            first.manifest.kind = PluginKind::Verb;
            first.manifest.verbs = verbs.iter().map(|verb| (*verb).to_owned()).collect();
            let _exited = plugins.supervise_at(Instant::now());
            for entry in &plugins.entries {
                entry.session.as_ref().unwrap().borrow_mut().events = vec![Subscription::Speech];
            }
            plugins
        }

        fn speech(text: &str) -> Event {
            Event::Speech {
                text: tehuti_protocol::Text::new(text.into()).unwrap(),
            }
        }

        #[test]
        fn a_verbs_notice_between_query_and_dispatch_cannot_split_the_route() {
            let mut plugins = verb_plugins(&["ada"]);
            let heard = speech("ada sum up");
            assert!(plugins.active(&heard), "the verb makes a plugin active");
            plugins.entries[0]
                .session
                .as_ref()
                .unwrap()
                .borrow_mut()
                .set_verbs(vec!["orion".into()], Instant::now());
            let request = plugins.dispatch(&heard, true);
            assert_eq!(
                request.map(|(key, _)| key.plugin),
                Some(0),
                "dispatch uses the route the query found"
            );
            assert!(!plugins.active(&heard), "later utterances see the new verbs");
            assert_eq!(
                plugins.dispatch(&heard, true),
                None,
                "nothing routes the old verb"
            );
        }

        #[test]
        fn an_application_plugin_cannot_take_verbs_at_runtime() {
            let mut plugins = verb_plugins(&[]);
            let session = plugins.entries[1].session.clone().unwrap();
            let deliveries = session.borrow_mut().message(
                Message::Call(Call {
                    jsonrpc: RpcVersion::Two,
                    request_id: None,
                    method: Method::Verbs {
                        verbs: vec!["ada".into()],
                    },
                }),
                Instant::now(),
            );
            assert!(deliveries.is_empty(), "the notice delivers nothing");
            assert!(session.borrow().verbs.is_empty(), "the verbs are refused");
            assert_eq!(
                session.borrow().invalid_messages,
                1,
                "the refusal counts as invalid"
            );
            let output: Vec<u8> = session.borrow().output.iter().copied().collect();
            assert!(
                String::from_utf8(output).unwrap().contains("action_rejected"),
                "the plugin is told why"
            );
            assert!(
                !plugins.active(&speech("ada sum up")),
                "the refused verb never routes"
            );
        }

        #[test]
        fn an_application_plugins_invoke_words_replace_and_apply_to_its_applications() {
            let plugins = verb_plugins(&[]);
            let notice = |words: &[&str]| {
                Message::Call(Call {
                    jsonrpc: RpcVersion::Two,
                    request_id: None,
                    method: Method::InvokeWords {
                        words: words.iter().map(|word| (*word).to_owned()).collect(),
                    },
                })
            };
            let session = plugins.entries[1].session.clone().unwrap();
            let _first = session
                .borrow_mut()
                .message(notice(&["usage", "code-review"]), Instant::now());
            let deliveries = session.borrow_mut().message(notice(&["usage"]), Instant::now());
            assert!(
                matches!(deliveries.as_slice(), [Delivery::SpeechHints]),
                "a notice refreshes the focused window's words"
            );
            assert_eq!(
                plugins.invoke_words("test"),
                ["usage"],
                "the second list replaces"
            );
            assert!(
                plugins.invoke_words("other").is_empty(),
                "only covered applications"
            );
        }

        #[test]
        fn an_exited_plugin_loses_its_pad_overrides() {
            let succeed = Rc::new(RefCell::new(vec![true; PLUGINS]));
            let starts = Rc::new(RefCell::new(vec![0; PLUGINS]));
            let mut plugins = plugins(&succeed, &starts);
            let now = Instant::now();
            let _exited = plugins.supervise_at(now);
            plugins.set_pad_overrides(
                0,
                vec![tehuti_protocol::PadOverride {
                    button: tehuti_protocol::ButtonName::South,
                    held: Vec::new(),
                }],
            );
            let _matched = plugins.active(&Event::Focus {
                app_id: Some("test".into()),
                title: String::new(),
            });
            let (south, pressed, released) = (
                tehuti_protocol::ButtonName::South,
                tehuti_protocol::ButtonState::Pressed,
                tehuti_protocol::ButtonState::Released,
            );
            assert!(
                plugins.pad_button(south, pressed, &[]),
                "the override applies before the exit"
            );
            assert!(
                plugins.pad_button(south, released, &[]),
                "its release follows the press"
            );
            break_session(&plugins, 0, false, now);
            assert_eq!(plugins.supervise_at(now), [0], "the crashed plugin is reported");
            let _restarted = plugins.supervise_at(later(now, 1000));
            assert_eq!(
                plugins.initialized_count(),
                PLUGINS,
                "the plugin is running again"
            );
            assert!(
                !plugins.pad_button(south, pressed, &[]),
                "its overrides are cleared"
            );
        }

        fn check(plugins: &Plugins, model: &[ModelEntry], starts: &[usize]) -> Result<(), TestCaseError> {
            for (index, (entry, expected)) in plugins.entries.iter().zip(model).enumerate() {
                prop_assert_eq!(
                    entry.session.is_some(),
                    expected.running,
                    "plugin {} running",
                    index
                );
                prop_assert_eq!(entry.restart.delay, expected.backoff, "plugin {} backoff", index);
                prop_assert_eq!(
                    entry.restart.at,
                    expected.restart_at,
                    "plugin {} restart time",
                    index
                );
                prop_assert_eq!(starts[index], expected.starts, "plugin {} start count", index);
            }
            Ok(())
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(64))]

            #[test]
            fn unanswered_requests_restart_despite_focus_changes(
                changes in prop::collection::vec((0_u64..100, any::<String>()), 1..30),
                button in any::<bool>(),
            ) {
                let succeed = Rc::new(RefCell::new(vec![true; PLUGINS]));
                let starts = Rc::new(RefCell::new(vec![0; PLUGINS]));
                let mut plugins = plugins(&succeed, &starts);
                plugins.entries[0].manifest.start = StartPolicy::OnFocus;
                plugins.entries[0].started = false;
                let method = if button {
                    Method::Button {
                        name: tehuti_protocol::ButtonName::East,
                        state: tehuti_protocol::ButtonState::Pressed,
                    }
                } else {
                    Method::Speech { text: tehuti_protocol::Text::new("request".into()).unwrap() }
                };
                prop_assert!(!plugins.active(&Event::Focus { app_id: None, title: String::new() }), "unmatched focus");
                let _active = plugins.active(&Event::Focus { app_id: Some("test".into()), title: String::new() });
                let old = plugins.entries[0].session.as_ref().unwrap().clone();
                for miss in 1..=3 {
                    let request_id = old.borrow_mut().request().unwrap();
                    old.borrow_mut().queue_event(&Call {
                        jsonrpc: RpcVersion::Two, request_id: Some(request_id), method: method.clone(),
                    }, Instant::now()).unwrap();
                    let deadline = old.borrow().pending[&request_id];
                    let event = if button {
                        Event::Button { name: tehuti_protocol::ButtonName::East, state: tehuti_protocol::ButtonState::Pressed }
                    } else {
                        Event::Speech { text: tehuti_protocol::Text::new("request".into()).unwrap() }
                    };
                    let (core, _, _) = tehuti_core::plugin::step(tehuti_core::State::new(),
                        tehuti_core::plugin::Input::Request {
                            key: tehuti_core::plugin::RequestKey { plugin: 0, request_id },
                            event, permissions: vec![Permission::Quit],
                        });
                    let (mut core, fallbacks, _) = tehuti_core::plugin::step(core,
                        tehuti_core::plugin::Input::Tick { elapsed: Duration::from_secs(2) });
                    prop_assert_eq!(fallbacks.len(), usize::from(!button));
                    for (millis, title) in &changes {
                        let _unmatched = plugins.active(&Event::Focus { app_id: None, title: title.clone() });
                        drop(plugins.supervise_at(deadline.checked_sub(Duration::from_micros(millis.saturating_add(1))).unwrap()));
                        prop_assert_eq!(starts.borrow()[0], 1);
                    }
                    let exited = plugins.supervise_at(deadline);
                    if miss < 3 {
                        prop_assert!(exited.is_empty());
                        prop_assert_eq!(starts.borrow()[0], 1);
                        for reply_id in [request_id, request_id.saturating_add(1000)] {
                            let reply = format!("{{\"jsonrpc\":\"2.0\",\"id\":{reply_id},\"result\":[{{\"type\":\"quit\"}}]}}\n");
                            for delivery in old.borrow_mut().receive(reply.as_bytes(), Instant::now()) {
                                if let Delivery::Reply { key, result, .. } = delivery {
                                    let (next, actions, _) = tehuti_core::plugin::step(core,
                                        tehuti_core::plugin::Input::Reply { key, result });
                                    prop_assert!(actions.is_empty(), "late and mismatched live replies release nothing");
                                    core = next;
                                }
                            }
                            prop_assert_eq!(old.borrow().status, Status::Ready);
                            prop_assert_eq!(old.borrow().missed_decisions, miss);
                        }
                    } else {
                        prop_assert_eq!(exited, vec![0]);
                        drop(plugins.supervise_at(later(deadline, 100)));
                        prop_assert_eq!(starts.borrow()[0], 2);
                    }
                }
            }

            #[test]
            fn simultaneous_expiries_add_one_miss(count in 1_u64..257, misses in 0_usize..2) {
                let succeed = Rc::new(RefCell::new(vec![true; PLUGINS]));
                let starts = Rc::new(RefCell::new(vec![0; PLUGINS]));
                let mut plugins = plugins(&succeed, &starts);
                let now = Instant::now();
                drop(plugins.supervise_at(now));
                let session = plugins.entries[0].session.as_ref().unwrap().clone();
                session.borrow_mut().missed_decisions = misses;
                for request_id in 0..count {
                    let _deadline = session.borrow_mut().pending.insert(request_id, now);
                }
                prop_assert!(plugins.supervise_at(now).is_empty());
                prop_assert_eq!(session.borrow().missed_decisions, misses.saturating_add(1));
                prop_assert_eq!(starts.borrow()[0], 1);
                prop_assert!(plugins.supervise_at(later(now, 1)).is_empty());
                prop_assert_eq!(session.borrow().missed_decisions, misses.saturating_add(1));
            }

            #[test]
            fn a_timely_decline_breaks_the_consecutive_miss_streak(misses in 1_usize..3) {
                let succeed = Rc::new(RefCell::new(vec![true; PLUGINS]));
                let starts = Rc::new(RefCell::new(vec![0; PLUGINS]));
                let mut plugins = plugins(&succeed, &starts);
                drop(plugins.supervise_at(Instant::now()));
                let session = plugins.entries[0].session.as_ref().unwrap().clone();
                session.borrow_mut().events = vec![Subscription::Button];
                let _active = plugins.active(&Event::Focus { app_id: Some("test".into()), title: String::new() });
                let event = Event::Button { name: tehuti_protocol::ButtonName::East, state: tehuti_protocol::ButtonState::Pressed };
                for _miss in 0..misses {
                    let (key, _) = plugins.dispatch(&event, true).unwrap();
                    let deadline = session.borrow().pending[&key.request_id];
                    prop_assert!(plugins.supervise_at(deadline).is_empty());
                }
                let (answered, _) = plugins.dispatch(&event, true).unwrap();
                let reply = format!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":[]}}\n", answered.request_id);
                drop(session.borrow_mut().receive(reply.as_bytes(), Instant::now()));
                prop_assert_eq!(session.borrow().missed_decisions, 0);
                for miss in 1..=3 {
                    let (key, _) = plugins.dispatch(&event, true).unwrap();
                    let deadline = session.borrow().pending[&key.request_id];
                    let exited = plugins.supervise_at(deadline);
                    if miss < 3 {
                        prop_assert!(exited.is_empty(), "two fresh misses still leave the plugin live");
                    } else {
                        prop_assert_eq!(exited, vec![0]);
                    }
                }
            }

            #[test]
            fn supervision_matches_the_reference_model(ops in prop::collection::vec(operation(), 1..60)) {
                let succeed = Rc::new(RefCell::new(vec![true; PLUGINS]));
                let starts = Rc::new(RefCell::new(vec![0; PLUGINS]));
                let mut plugins = plugins(&succeed, &starts);
                let mut model = vec![ModelEntry::new(); PLUGINS];
                let mut now = Instant::now();
                for operation in ops {
                    match operation {
                        Operation::Advance(millis) => now = later(now, millis),
                        Operation::Crash(index) | Operation::Hang(index) => {
                            break_session(&plugins, index, matches!(operation, Operation::Hang(_)), now);
                            model[index].broken = model[index].running;
                        }
                        Operation::StartsSucceed(index, succeeds) => succeed.borrow_mut()[index] = succeeds,
                    }
                    // Every plugin is supervised in the same pass; a broken one must not disturb the rest.
                    drop(plugins.supervise_at(now));
                    mark_healthy(&plugins, now);
                    for (index, entry) in model.iter_mut().enumerate() {
                        entry.supervise(now, succeed.borrow()[index]);
                    }
                    check(&plugins, &model, &starts.borrow())?;
                }
            }
        }

        #[test]
        fn restart_delay_starts_at_100_ms_doubles_and_caps_at_30_s() {
            let succeed = Rc::new(RefCell::new(vec![false; PLUGINS]));
            let starts = Rc::new(RefCell::new(vec![0; PLUGINS]));
            let mut plugins = plugins(&succeed, &starts);
            let mut now = Instant::now();
            let mut delays = Vec::new();
            for _attempt in 0..12 {
                drop(plugins.supervise_at(now));
                let restart_at = plugins.entries[0].restart.at.unwrap();
                delays.push(restart_at.duration_since(now).as_millis());
                drop(plugins.supervise_at(restart_at.checked_sub(Duration::from_millis(1)).unwrap()));
                assert_eq!(
                    starts.borrow()[0],
                    delays.len(),
                    "no restart before the delay elapses"
                );
                now = restart_at;
            }
            assert_eq!(
                delays,
                vec![
                    100, 200, 400, 800, 1600, 3200, 6400, 12_800, 25_600, 30_000, 30_000, 30_000
                ],
                "backoff doubles from 100 ms and caps at 30 s"
            );
        }

        #[test]
        fn backoff_resets_after_30_s_healthy() {
            let succeed = Rc::new(RefCell::new(vec![false; PLUGINS]));
            let starts = Rc::new(RefCell::new(vec![0; PLUGINS]));
            let mut plugins = plugins(&succeed, &starts);
            let mut now = Instant::now();
            for _attempt in 0..5 {
                drop(plugins.supervise_at(now));
                now = plugins.entries[0].restart.at.unwrap();
            }
            succeed.borrow_mut()[0] = true;
            drop(plugins.supervise_at(now));
            mark_healthy(&plugins, now);
            let young = Duration::from_millis(3200);
            assert_eq!(plugins.entries[0].restart.delay, young, "backoff kept while young");
            drop(plugins.supervise_at(later(now, 29_999)));
            assert_eq!(plugins.entries[0].restart.delay, young, "not reset before 30 s");
            drop(plugins.supervise_at(later(now, 30_000)));
            assert_eq!(
                plugins.entries[0].restart.delay, RESTART_BACKOFF_BASE,
                "reset after 30 s healthy"
            );
        }
    }
}
