//! Plugin processes and their non-blocking stdio sources.

use crate::state::Compositor;
use core::{cell::RefCell, time::Duration};
use smithay::reexports::calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    process::{Child, ChildStdin, Stdio},
    rc::Rc,
    time::Instant,
};
use tehuti_protocol::{
    Action, Call, Event, Initialize, MAX_MESSAGE_BYTES, Manifest, Message, Method, Permission, ResultValue,
    RpcVersion, StartPolicy, Subscription,
};

const RESTART_BACKOFF_BASE: Duration = Duration::from_millis(100);
const RESTART_BACKOFF_CAP: Duration = Duration::from_secs(30);
const RESTART_BACKOFF_RESET: Duration = Duration::from_secs(30);
const SUPERVISION_INTERVAL: Duration = Duration::from_millis(10);
const HANG_TIMEOUT: Duration = Duration::from_secs(2);
const INVALID_MESSAGE_LIMIT: usize = 16;
const READ_ATTEMPTS: usize = 16;
const FLOOD_WINDOW: Duration = Duration::from_secs(1);
const FLOOD_BYTES: usize = 262_144;

enum Delivery {
    Reply {
        key: tehuti_core::plugin::RequestKey,
        result: Result<Vec<Action>, String>,
        submission_id: Option<u64>,
    },
    Notice(Vec<Action>),
}

#[derive(Debug)]
struct Process {
    child: Option<Child>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Starting,
    Ready,
    Refused,
    Failed,
}

#[derive(Debug)]
struct Session {
    child: Process,
    stdin: ChildStdin,
    output: VecDeque<u8>,
    input: Vec<u8>,
    dropping_line: bool,
    status: Status,
    initialize_deadline: Instant,
    latest_focus: Option<(Option<String>, String)>,
    events: Vec<Subscription>,
    manifest: Manifest,
    next_id: u64,
    plugin: usize,
    invalid_messages: usize,
    missed_decisions: usize,
    healthy_since: Option<Instant>,
    pending: BTreeMap<u64, Instant>,
    read_window: Instant,
    read_bytes: usize,
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            crate::shutdown::terminate(&mut child, false);
        }
    }
}

impl Session {
    fn queue_initialize(&mut self) -> io::Result<()> {
        self.queue(&Call {
            jsonrpc: RpcVersion::Two,
            request_id: Some(0),
            method: Method::Initialize(Initialize {
                protocol_version: 0,
                encoding: "json".into(),
                core_version: env!("CARGO_PKG_VERSION").into(),
                options: Some(
                    core::iter::once(("action_submission".into(), serde_json::json!("submit_actions")))
                        .collect(),
                ),
            }),
        })
    }

    fn accept_bytes(&mut self, count: usize) -> bool {
        let now = Instant::now();
        if now.duration_since(self.read_window) >= FLOOD_WINDOW {
            self.read_window = now;
            self.read_bytes = 0;
        }
        self.read_bytes = self.read_bytes.saturating_add(count);
        if self.read_bytes > FLOOD_BYTES {
            self.fail("plugin stdout flood");
            return false;
        }
        true
    }

    fn fail(&mut self, reason: &str) {
        if self.status != Status::Failed {
            tracing::warn!(plugin = self.plugin, executable = %self.manifest.executable, reason, "plugin restart scheduled");
            self.status = Status::Failed;
        }
    }

    fn invalid(&mut self) {
        self.invalid_messages = self.invalid_messages.saturating_add(1);
        self.healthy_since = (self.status == Status::Ready).then(Instant::now);
        if self.invalid_messages >= INVALID_MESSAGE_LIMIT {
            self.fail("repeated invalid messages");
        }
    }

    fn supervise(&mut self, now: Instant) {
        self.expire_initialize();
        let expired = self.pending.values().filter(|deadline| now >= **deadline).count();
        self.pending.retain(|_, deadline| now < *deadline);
        self.missed_decisions = self.missed_decisions.saturating_add(usize::from(expired > 0));
        if self.missed_decisions >= 3 {
            self.fail("three consecutive plugin decisions missed");
        }
        if let Some(child) = &mut self.child.child {
            match child.try_wait() {
                Ok(Some(status)) => self.fail(&format!("plugin exited: {status}")),
                Ok(None) => {}
                Err(error) => self.fail(&format!("plugin status failed: {error}")),
            }
        }
    }

    fn observe(&mut self, event: &Event) {
        if let Event::Focus { app_id, title } = event {
            self.latest_focus = Some((app_id.clone(), title.clone()));
        }
    }

    fn request(&mut self) -> Option<u64> {
        let request_id = self.next_id;
        self.next_id = self.next_id.checked_add(1)?;
        Some(request_id)
    }

    fn queue_event(&mut self, call: &Call) -> io::Result<()> {
        if call.request_id.is_some() && self.pending.len() >= 256 {
            return Err(io::Error::other("plugin pending queue full"));
        }
        self.queue(call)?;
        let budget = if matches!(call.method, Method::Button { .. }) {
            tehuti_core::plugin::BUTTON_BUDGET
        } else {
            tehuti_core::plugin::SPEECH_BUDGET
        };
        if let Some(request_id) = call.request_id
            && let Some(deadline) = Instant::now().checked_add(budget)
        {
            let _deadline = self.pending.insert(request_id, deadline);
        }
        Ok(())
    }

    fn queue(&mut self, call: &Call) -> io::Result<()> {
        self.queue_bytes(serde_json::to_vec(call)?)
    }

    fn queue_bytes(&mut self, bytes: Vec<u8>) -> io::Result<()> {
        if self.output.len().saturating_add(bytes.len()).saturating_add(1) > MAX_MESSAGE_BYTES {
            return Err(io::Error::other("plugin output queue full"));
        }
        self.output.extend(bytes);
        self.output.push_back(b'\n');
        Ok(())
    }

    fn write_ready(&mut self) -> io::Result<PostAction> {
        while !self.output.is_empty() {
            let (first, _) = self.output.as_slices();
            match self.stdin.write(first) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "plugin stdin closed")),
                Ok(count) => {
                    let _written = self.output.drain(..count);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(PostAction::Continue),
                Err(error) => return Err(error),
            }
        }
        Ok(PostAction::Disable)
    }

    fn receive(&mut self, bytes: &[u8]) -> Vec<Delivery> {
        let mut actions = Vec::new();
        for byte in bytes {
            if *byte == b'\n' {
                if !self.dropping_line {
                    let line = core::mem::take(&mut self.input);
                    match tehuti_protocol::parse_message(&line) {
                        Ok(message) => actions.extend(self.message(message)),
                        Err(error) => {
                            tracing::warn!(%error, "plugin message dropped");
                            actions.extend(self.invalid_message(&line, error));
                            self.invalid();
                        }
                    }
                }
                self.dropping_line = false;
            } else if !self.dropping_line {
                if self.input.len() == MAX_MESSAGE_BYTES {
                    self.input.clear();
                    self.dropping_line = true;
                    tracing::warn!("overlong plugin line dropped");
                    self.invalid();
                } else {
                    self.input.push(*byte);
                }
            }
        }
        actions
    }

    fn invalid_message(&mut self, line: &[u8], reason: String) -> Vec<Delivery> {
        if self.status != Status::Ready {
            return Vec::new();
        }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            return Vec::new();
        };
        let request_id = value.get("id").and_then(serde_json::Value::as_u64);
        let method = value.get("method").and_then(serde_json::Value::as_str);
        let event_id = if method == Some("submit_actions") {
            value
                .get("params")
                .and_then(|params| params.get("event_id"))
                .and_then(serde_json::Value::as_u64)
        } else if value.get("result").is_some() {
            request_id
        } else {
            None
        };
        if let Some(event_id) = event_id {
            let _deadline = self.pending.remove(&event_id);
            return vec![Delivery::Reply {
                key: tehuti_core::plugin::RequestKey {
                    plugin: self.plugin,
                    request_id: event_id,
                },
                result: Err(reason),
                submission_id: if method == Some("submit_actions") {
                    request_id
                } else {
                    None
                },
            }];
        }
        if method == Some("actions")
            && let Err(error) = self.reject(None, &reason)
        {
            tracing::warn!(%error, "plugin rejection dropped");
        }
        Vec::new()
    }

    fn expire_initialize(&mut self) {
        if self.status == Status::Starting && Instant::now() >= self.initialize_deadline {
            self.fail("plugin initialize timed out");
        }
    }

    fn initialize_reply(&mut self, message: Message) {
        self.expire_initialize();
        if matches!(self.status, Status::Refused | Status::Failed) {
            return;
        }
        if let Message::Response(response) = message
            && response.request_id == 0
            && let ResultValue::Initialized(result) = response.result
        {
            if result.protocol_version != 0 || result.encoding != "json" {
                self.status = Status::Refused;
                tracing::warn!(
                    version = result.protocol_version,
                    encoding = result.encoding,
                    "plugin handshake refused"
                );
            } else {
                self.events = result
                    .events
                    .into_iter()
                    .filter(|event| self.manifest.events.contains(event))
                    .collect();
                self.status = Status::Ready;
                self.healthy_since = Some(Instant::now());
                if self.events.contains(&Subscription::Focus)
                    && let Some((app_id, title)) = self.latest_focus.clone()
                    && let Err(error) = self.queue(&Call {
                        jsonrpc: RpcVersion::Two,
                        request_id: None,
                        method: Method::Focus { app_id, title },
                    })
                {
                    tracing::warn!(%error, "plugin focus dropped");
                }
            }
        }
    }

    fn notice_actions(&mut self, actions: Vec<Action>) -> Vec<Delivery> {
        match tehuti_core::plugin::validate_actions(actions, &self.manifest.permissions) {
            Ok(actions) => vec![Delivery::Notice(actions)],
            Err(reason) => {
                self.invalid();
                tracing::warn!(%reason, "plugin action rejected");
                if let Err(error) = self.reject(None, &reason) {
                    tracing::warn!(%error, "plugin rejection dropped");
                }
                Vec::new()
            }
        }
    }

    fn complete_decision(&mut self, message: &Message) {
        self.supervise(Instant::now());
        let event_id = match message {
            Message::Response(response) => Some(response.request_id),
            Message::Error(response) => Some(response.request_id),
            Message::Call(Call {
                method: Method::SubmitActions { event_id, .. },
                ..
            }) => Some(*event_id),
            Message::Call(_) => None,
        };
        if let Some(event_id) = event_id
            && self.pending.remove(&event_id).is_some()
        {
            self.missed_decisions = 0;
        }
    }

    fn message(&mut self, message: Message) -> Vec<Delivery> {
        if let Message::Error(response) = &message {
            tracing::warn!(code = response.error.code, message = %response.error.message, "plugin request declined");
        }
        if matches!(self.status, Status::Refused | Status::Failed) {
            return Vec::new();
        }
        if self.status != Status::Ready {
            self.initialize_reply(message);
            return Vec::new();
        }
        self.complete_decision(&message);
        match message {
            Message::Response(response) => vec![Delivery::Reply {
                key: tehuti_core::plugin::RequestKey {
                    plugin: self.plugin,
                    request_id: response.request_id,
                },
                result: match response.result {
                    ResultValue::Actions(actions) => Ok(actions),
                    ResultValue::Initialized(_) => Err("result must be an action list".into()),
                },
                submission_id: None,
            }],
            Message::Error(response) => vec![Delivery::Reply {
                key: tehuti_core::plugin::RequestKey {
                    plugin: self.plugin,
                    request_id: response.request_id,
                },
                result: Ok(Vec::new()),
                submission_id: None,
            }],
            Message::Call(Call {
                request_id: Some(submission_id),
                method: Method::SubmitActions { event_id, actions },
                ..
            }) => vec![Delivery::Reply {
                key: tehuti_core::plugin::RequestKey {
                    plugin: self.plugin,
                    request_id: event_id,
                },
                result: Ok(actions),
                submission_id: Some(submission_id),
            }],
            Message::Call(Call {
                method: Method::Actions { actions },
                ..
            }) => self.notice_actions(actions),
            Message::Call(_) => Vec::new(),
        }
    }

    fn call_for_event(&mut self, event: &Event, selected: bool) -> Option<Call> {
        let (request_id, method) = match event {
            Event::Button { name, state } if selected && self.events.contains(&Subscription::Button) => {
                let request_id = self.request()?;
                (
                    Some(request_id),
                    Method::Button {
                        name: *name,
                        state: *state,
                    },
                )
            }
            Event::Speech { text } if selected && self.events.contains(&Subscription::Speech) => {
                let request_id = self.request()?;
                (Some(request_id), Method::Speech { text: text.clone() })
            }
            Event::Focus { app_id, title } if self.events.contains(&Subscription::Focus) => (
                None,
                Method::Focus {
                    app_id: app_id.clone(),
                    title: title.clone(),
                },
            ),
            Event::Button { .. } | Event::Focus { .. } | Event::Tick { .. } | Event::Speech { .. } => {
                return None;
            }
        };
        Some(Call {
            jsonrpc: RpcVersion::Two,
            request_id,
            method,
        })
    }

    fn reject(&mut self, request_id: Option<u64>, reason: &str) -> io::Result<()> {
        self.queue_bytes(serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0", "method": "action_rejected",
            "params": { "event_id": request_id, "error": { "code": -32602, "message": reason } }
        }))?)
    }
}

type Running = (Rc<RefCell<Session>>, Box<dyn Fn()>, Box<dyn Fn()>);
type Start = Box<dyn Fn(&Manifest) -> io::Result<Running>>;
struct Entry {
    manifest: Manifest,
    plugin: usize,
    latest_focus: Option<(Option<String>, String)>,
    started: bool,
    restart_at: Option<Instant>,
    backoff: Duration,
    next_id: u64,
    cleanup: Option<Box<dyn Fn()>>,
    session: Option<Rc<RefCell<Session>>>,
    wake: Option<Box<dyn Fn()>>,
}

impl Entry {
    fn new(manifest: Manifest, plugin: usize) -> Self {
        Self {
            manifest,
            plugin,
            latest_focus: None,
            started: false,
            restart_at: None,
            backoff: RESTART_BACKOFF_BASE,
            next_id: 1,
            cleanup: None,
            session: None,
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
    cleanup_timer: Box<dyn Fn()>,
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
        if entry.session.is_none() && entry.restart_at.is_none_or(|deadline| now >= deadline) {
            entry.started = true;
            entry.restart_at = None;
            match start(&entry.manifest) {
                Ok((session, wake, cleanup)) => {
                    {
                        let mut session = session.borrow_mut();
                        session.next_id = entry.next_id;
                        session.plugin = entry.plugin;
                        session.latest_focus.clone_from(&entry.latest_focus);
                    }
                    entry.cleanup = Some(cleanup);
                    entry.session = Some(session);
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
        entry.restart_at = now.checked_add(entry.backoff);
        tracing::warn!(executable = %entry.manifest.executable, delay_ms = entry.backoff.as_millis(), "plugin restart backoff");
        entry.backoff = entry.backoff.saturating_mul(2).min(RESTART_BACKOFF_CAP);
    }

    pub(crate) fn supervise(&mut self) -> Vec<usize> {
        self.supervise_at(Instant::now())
    }

    fn supervise_at(&mut self, now: Instant) -> Vec<usize> {
        let mut exited = Vec::new();
        for entry in &mut self.entries {
            let failed = entry.session.as_ref().is_some_and(|session| {
                let mut session = session.borrow_mut();
                session.supervise(now);
                entry.next_id = session.next_id;
                if session.status != Status::Failed
                    && session
                        .healthy_since
                        .is_some_and(|since| now.duration_since(since) >= RESTART_BACKOFF_RESET)
                {
                    entry.backoff = RESTART_BACKOFF_BASE;
                    session.invalid_messages = 0;
                }
                session.status == Status::Failed
            });
            if failed {
                exited.push(entry.plugin);
                if let Some(cleanup) = entry.cleanup.take() {
                    cleanup();
                }
                entry.wake = None;
                entry.session = None;
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
                session.borrow_mut().expire_initialize();
            }
        }
        self.entries.iter().any(|entry| self.matches(entry, event))
    }

    fn matches(&self, entry: &Entry, event: &Event) -> bool {
        self.focus
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
            if let Err(error) = session.queue_event(&call) {
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
        cleanup_timer: Box::new(move || handle.remove(timer_token)),
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

fn create_session(manifest: &Manifest) -> io::Result<(Rc<RefCell<Session>>, std::process::ChildStdout)> {
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
    let session = Rc::new(RefCell::new(Session {
        child: process,
        stdin,
        output: VecDeque::new(),
        input: Vec::new(),
        dropping_line: false,
        status: Status::Starting,
        initialize_deadline: Instant::now()
            .checked_add(core::time::Duration::from_secs(2))
            .unwrap_or_else(Instant::now),
        latest_focus: None,
        events: Vec::new(),
        manifest: manifest.clone(),
        next_id: 1,
        plugin: usize::MAX,
        invalid_messages: 0,
        missed_decisions: 0,
        healthy_since: None,
        pending: BTreeMap::new(),
        read_window: Instant::now(),
        read_bytes: 0,
    }));
    session.borrow_mut().queue_initialize()?;
    Ok((session, stdout))
}

fn start_process<Data: 'static>(
    handle: &smithay::reexports::calloop::LoopHandle<'static, Data>,
    manifest: &Manifest,
    compositor: fn(&mut Data) -> &mut Compositor,
) -> io::Result<Running> {
    let (session, stdout) = create_session(manifest)?;
    let writer = session.clone();
    let descriptor = rustix::io::dup(&session.borrow().stdin)?;
    let token = handle
        .insert_source(
            Generic::new(descriptor, Interest::WRITE, Mode::Level),
            move |_, _, data| {
                let result = writer.borrow_mut().write_ready();
                Ok(result.unwrap_or_else(|error| {
                    tracing::warn!(%error, "plugin write failed");
                    writer.borrow_mut().fail("plugin pipe write failed");
                    compositor(data).plugin_exited(writer.borrow().plugin);
                    PostAction::Remove
                }))
            },
        )
        .map_err(|error| io::Error::other(error.to_string()))?;
    let reader_token = match insert_reader(handle, stdout, session.clone(), token, compositor) {
        Ok(reader_token) => reader_token,
        Err(error) => {
            handle.remove(token);
            return Err(error);
        }
    };
    let wake_handle = handle.clone();
    let cleanup_handle = handle.clone();
    Ok((
        session,
        Box::new(move || {
            if let Err(error) = wake_handle.enable(&token) {
                tracing::warn!(%error, "plugin writer unavailable");
            }
        }),
        Box::new(move || {
            cleanup_handle.remove(token);
            cleanup_handle.remove(reader_token);
        }),
    ))
}

fn insert_reader<Data: 'static>(
    handle: &smithay::reexports::calloop::LoopHandle<'static, Data>,
    stdout: std::process::ChildStdout,
    session: Rc<RefCell<Session>>,
    writer_token: smithay::reexports::calloop::RegistrationToken,
    compositor: fn(&mut Data) -> &mut Compositor,
) -> io::Result<smithay::reexports::calloop::RegistrationToken> {
    let reader = session;
    let writer_handle = handle.downgrade();
    let reader_token = handle
        .insert_source(
            Generic::new(
                std::fs::File::from(std::os::fd::OwnedFd::from(stdout)),
                Interest::READ,
                Mode::Level,
            ),
            move |_, source, data| {
                let action = read_ready(&reader, source, compositor(data));
                if !reader.borrow().output.is_empty()
                    && let Some(writer_handle) = writer_handle.upgrade()
                    && let Err(error) = writer_handle.enable(&writer_token)
                {
                    tracing::warn!(%error, "plugin writer unavailable");
                }
                Ok(action)
            },
        )
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(reader_token)
}

fn read_ready(
    reader: &Rc<RefCell<Session>>,
    mut source: &std::fs::File,
    compositor: &mut Compositor,
) -> PostAction {
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
                if !reader.borrow_mut().accept_bytes(count) {
                    compositor.plugin_exited(reader.borrow().plugin);
                    return PostAction::Remove;
                }
                let deliveries = reader
                    .borrow_mut()
                    .receive(bytes.get(..count).unwrap_or_default());
                compositor.advance_time();
                deliver(reader, compositor, deliveries);
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

fn deliver(reader: &Rc<RefCell<Session>>, compositor: &mut Compositor, deliveries: Vec<Delivery>) {
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
                        reader.borrow_mut().invalid();
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
            Delivery::Notice(actions) => {
                for action in actions {
                    compositor.apply_plugin_action(&action);
                }
            }
        }
    }
}

thread_local! {
    static FUZZ_SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

fn fuzz_session() -> io::Result<Session> {
    let manifest = Manifest {
        plugin_id: None,
        author: None,
        security_contact: None,
        executable: "sleep".into(),
        app_ids: vec!["fuzz".into()],
        events: vec![Subscription::Button, Subscription::Speech, Subscription::Focus],
        start: StartPolicy::Always,
        permissions: vec![Permission::Key, Permission::Text],
    };
    let mut child = crate::child_process::command("sleep")
        .arg("infinity")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("stdin missing"))?;
    let now = Instant::now();
    Ok(Session {
        child: Process { child: Some(child) },
        stdin,
        output: VecDeque::new(),
        input: Vec::new(),
        dropping_line: false,
        status: Status::Starting,
        initialize_deadline: now,
        latest_focus: None,
        events: Vec::new(),
        manifest,
        next_id: 1,
        plugin: 0,
        invalid_messages: 0,
        missed_decisions: 0,
        healthy_since: None,
        pending: BTreeMap::new(),
        read_window: now,
        read_bytes: 0,
    })
}

/// Feeds arbitrary bytes to one plugin session as if they were its stdout and routes the replies
/// through the core. Fuzzing seam only; the core must never panic on any input.
///
/// The first byte picks the session state and the read chunk size.
///
/// # Errors
/// The stand-in child process cannot be spawned.
#[doc(hidden)]
pub fn fuzz_stdout(bytes: &[u8]) -> io::Result<()> {
    let Some((&control, stdout)) = bytes.split_first() else {
        return Ok(());
    };
    FUZZ_SESSION.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(fuzz_session()?);
        }
        let Some(session) = slot.as_mut() else {
            return Ok(());
        };
        let now = Instant::now();
        session.output.clear();
        session.input.clear();
        session.dropping_line = false;
        session.invalid_messages = 0;
        session.pending.clear();
        session.read_window = now;
        session.read_bytes = 0;
        session.initialize_deadline = now.checked_add(HANG_TIMEOUT).unwrap_or(now);
        session.status = if control.is_multiple_of(2) {
            Status::Starting
        } else {
            Status::Ready
        };
        session.events = session.manifest.events.clone();
        let chunk = usize::from(control.checked_shr(1).unwrap_or(0)).max(1);
        let mut core = tehuti_core::State::new();
        for part in stdout.chunks(chunk) {
            if !session.accept_bytes(part.len()) {
                break;
            }
            for delivery in session.receive(part) {
                if let Delivery::Reply { key, result, .. } = delivery {
                    let (next, _actions, _rejection) =
                        tehuti_core::plugin::step(core, tehuti_core::plugin::Input::Reply { key, result });
                    core = next;
                }
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_cannot_request_secret_delivery() {
        let manifest = Manifest {
            plugin_id: None,
            author: None,
            security_contact: None,
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
                plugin_id: None,
                author: None,
                security_contact: None,
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

        /// A ready session backed by a live child that never writes.
        fn live_session(manifest: &Manifest) -> io::Result<Rc<RefCell<Session>>> {
            let mut child = crate::child_process::command("sleep")
                .arg("3600")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()?;
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("stdin missing"))?;
            let now = Instant::now();
            Ok(Rc::new(RefCell::new(Session {
                child: Process { child: Some(child) },
                stdin,
                output: VecDeque::new(),
                input: Vec::new(),
                dropping_line: false,
                status: Status::Ready,
                initialize_deadline: now,
                latest_focus: None,
                events: Vec::new(),
                manifest: manifest.clone(),
                next_id: 1,
                plugin: usize::MAX,
                invalid_messages: 0,
                missed_decisions: 0,
                healthy_since: None,
                pending: BTreeMap::new(),
                read_window: now,
                read_bytes: 0,
            })))
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
                let running: Running = (live_session(manifest)?, Box::new(|| {}), Box::new(|| {}));
                Ok(running)
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
                cleanup_timer: Box::new(|| {}),
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

        fn check(plugins: &Plugins, model: &[ModelEntry], starts: &[usize]) -> Result<(), TestCaseError> {
            for (index, (entry, expected)) in plugins.entries.iter().zip(model).enumerate() {
                prop_assert_eq!(
                    entry.session.is_some(),
                    expected.running,
                    "plugin {} running",
                    index
                );
                prop_assert_eq!(entry.backoff, expected.backoff, "plugin {} backoff", index);
                prop_assert_eq!(
                    entry.restart_at,
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
                    }).unwrap();
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
                            for delivery in old.borrow_mut().receive(reply.as_bytes()) {
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
                drop(session.borrow_mut().receive(reply.as_bytes()));
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
                let restart_at = plugins.entries[0].restart_at.unwrap();
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
                now = plugins.entries[0].restart_at.unwrap();
            }
            succeed.borrow_mut()[0] = true;
            drop(plugins.supervise_at(now));
            mark_healthy(&plugins, now);
            let young = Duration::from_millis(3200);
            assert_eq!(plugins.entries[0].backoff, young, "backoff kept while young");
            drop(plugins.supervise_at(later(now, 29_999)));
            assert_eq!(plugins.entries[0].backoff, young, "not reset before 30 s");
            drop(plugins.supervise_at(later(now, 30_000)));
            assert_eq!(
                plugins.entries[0].backoff, RESTART_BACKOFF_BASE,
                "reset after 30 s healthy"
            );
        }
    }
}
