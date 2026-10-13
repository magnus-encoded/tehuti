//! Plugin processes and their non-blocking stdio sources.

use crate::state::Compositor;
use core::cell::RefCell;
use smithay::reexports::calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic};
use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    rc::Rc,
    time::Instant,
};
use tehuti_protocol::{
    Action, Call, Event, Initialize, MAX_MESSAGE_BYTES, Manifest, Message, Method, Permission, ResultValue,
    RpcVersion, StartPolicy, Subscription,
};

enum Delivery {
    Reply {
        key: tehuti_core::plugin::RequestKey,
        result: Result<Vec<Action>, String>,
        submission_id: Option<u64>,
    },
    Notice(Vec<Action>),
}

#[derive(Debug)]
struct Session {
    child: Child,
    stdin: ChildStdin,
    output: VecDeque<u8>,
    input: Vec<u8>,
    dropping_line: bool,
    ready: bool,
    initialize_deadline: Instant,
    latest_focus: Option<(Option<String>, String)>,
    refused: bool,
    events: Vec<Subscription>,
    manifest: Manifest,
    next_id: u64,
    plugin: usize,
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Err(error) = self.child.kill() {
            tracing::debug!(%error, "plugin kill failed");
        }
        if let Err(error) = self.child.wait() {
            tracing::debug!(%error, "plugin reap failed");
        }
    }
}

impl Session {
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
                        }
                    }
                }
                self.dropping_line = false;
            } else if !self.dropping_line {
                if self.input.len() == MAX_MESSAGE_BYTES {
                    self.input.clear();
                    self.dropping_line = true;
                    tracing::warn!("overlong plugin line dropped");
                } else {
                    self.input.push(*byte);
                }
            }
        }
        actions
    }

    fn invalid_message(&mut self, line: &[u8], reason: String) -> Vec<Delivery> {
        if !self.ready || self.refused {
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
        if !self.ready && !self.refused && Instant::now() >= self.initialize_deadline {
            self.refused = true;
            tracing::warn!("plugin initialize timed out");
        }
    }

    fn initialize_reply(&mut self, message: Message) {
        self.expire_initialize();
        if self.refused {
            return;
        }
        if let Message::Response(response) = message
            && response.request_id == 0
            && let ResultValue::Initialized(result) = response.result
        {
            if result.protocol_version != 0 || result.encoding != "json" {
                self.refused = true;
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
                self.ready = true;
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

    fn message(&mut self, message: Message) -> Vec<Delivery> {
        if let Message::Error(response) = &message {
            tracing::warn!(code = response.error.code, message = %response.error.message, "plugin request declined");
        }
        if self.refused {
            return Vec::new();
        }
        if !self.ready {
            self.initialize_reply(message);
            return Vec::new();
        }
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
            }) => match tehuti_core::plugin::validate_actions(actions, &self.manifest.permissions) {
                Ok(actions) => vec![Delivery::Notice(actions)],
                Err(reason) => {
                    if let Err(error) = self.reject(None, &reason) {
                        tracing::warn!(%error, "plugin rejection dropped");
                    }
                    Vec::new()
                }
            },
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

type Running = (Rc<RefCell<Session>>, Box<dyn Fn()>);
type Start = Box<dyn Fn(&Manifest) -> io::Result<Running>>;
struct Entry {
    manifest: Manifest,
    attempted: bool,
    session: Option<Rc<RefCell<Session>>>,
    wake: Option<Box<dyn Fn()>>,
}

pub struct Plugins {
    entries: Vec<Entry>,
    start: Start,
    focus: Option<String>,
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
                    session.ready && !session.refused
                })
            })
            .count()
    }

    fn start_entry(start: &Start, entry: &mut Entry) {
        if !entry.attempted {
            entry.attempted = true;
            match start(&entry.manifest) {
                Ok((session, wake)) => {
                    entry.session = Some(session);
                    entry.wake = Some(wake);
                }
                Err(error) => tracing::warn!(%error, "plugin start failed"),
            }
        }
    }

    pub(crate) fn active(&mut self, event: &Event) -> bool {
        if let Event::Focus { app_id, .. } = event {
            self.focus.clone_from(app_id);
        }
        for entry in &mut self.entries {
            if entry.manifest.start == StartPolicy::Always
                || self
                    .focus
                    .as_ref()
                    .is_some_and(|app_id| entry.manifest.app_ids.contains(app_id))
            {
                Self::start_entry(&self.start, entry);
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
                session.ready
                    && !session.refused
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
            if !session.ready || session.refused {
                continue;
            }
            let Some(call) = session.call_for_event(event, route_button && selected == Some(index)) else {
                continue;
            };
            let request_id = call.request_id;
            if let Err(error) = session.queue(&call) {
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
    let start: Start = Box::new(move |manifest| start_process(&handle, manifest, compositor));
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
        match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
            .and_then(|file| {
                if !file.metadata()?.is_file() {
                    return Err(io::Error::other("manifest must be a regular file"));
                }
                let mut bytes = Vec::new();
                let _count = file.take(65_537).read_to_end(&mut bytes)?;
                tehuti_protocol::parse_manifest(&bytes).map_err(io::Error::other)
            }) {
            Ok(mut manifest) => {
                if Path::new(&manifest.executable).is_relative() {
                    manifest.executable = directory
                        .join(&manifest.executable)
                        .to_string_lossy()
                        .into_owned();
                }
                let mut entry = Entry {
                    manifest,
                    attempted: false,
                    session: None,
                    wake: None,
                };
                if entry.manifest.start == StartPolicy::Always {
                    Plugins::start_entry(&start, &mut entry);
                }
                entries.push(entry);
            }
            Err(error) => tracing::warn!(?path, %error, "plugin manifest skipped"),
        }
    }
    Ok(Plugins {
        entries,
        start,
        focus: None,
    })
}

fn create_session(manifest: &Manifest) -> io::Result<(Rc<RefCell<Session>>, std::process::ChildStdout)> {
    let mut child = Command::new(&manifest.executable)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("plugin stdin missing"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("plugin stdout missing"))?;
    rustix::fs::fcntl_setfl(&stdin, rustix::fs::OFlags::NONBLOCK)?;
    rustix::fs::fcntl_setfl(&stdout, rustix::fs::OFlags::NONBLOCK)?;
    let session = Rc::new(RefCell::new(Session {
        child,
        stdin,
        output: VecDeque::new(),
        input: Vec::new(),
        dropping_line: false,
        ready: false,
        initialize_deadline: Instant::now()
            .checked_add(core::time::Duration::from_secs(2))
            .unwrap_or_else(Instant::now),
        latest_focus: None,
        refused: false,
        events: Vec::new(),
        manifest: manifest.clone(),
        next_id: 1,
        plugin: usize::MAX,
    }));
    session.borrow_mut().queue(&Call {
        jsonrpc: RpcVersion::Two,
        request_id: Some(0),
        method: Method::Initialize(Initialize {
            protocol_version: 0,
            encoding: "json".into(),
            core_version: env!("CARGO_PKG_VERSION").into(),
            options: Some(
                core::iter::once(("action_submission".into(), serde_json::json!("submit_actions"))).collect(),
            ),
        }),
    })?;
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
            move |_, _, _| {
                let result = writer.borrow_mut().write_ready();
                Ok(result.unwrap_or_else(|error| {
                    tracing::warn!(%error, "plugin write failed");
                    writer.borrow_mut().refused = true;
                    PostAction::Remove
                }))
            },
        )
        .map_err(|error| io::Error::other(error.to_string()))?;
    insert_reader(handle, stdout, session.clone(), token, compositor)?;
    let wake_handle = handle.clone();
    Ok((
        session,
        Box::new(move || {
            if let Err(error) = wake_handle.enable(&token) {
                tracing::warn!(%error, "plugin writer unavailable");
            }
        }),
    ))
}

fn insert_reader<Data: 'static>(
    handle: &smithay::reexports::calloop::LoopHandle<'static, Data>,
    stdout: std::process::ChildStdout,
    session: Rc<RefCell<Session>>,
    writer_token: smithay::reexports::calloop::RegistrationToken,
    compositor: fn(&mut Data) -> &mut Compositor,
) -> io::Result<()> {
    let reader = session;
    let writer_handle = handle.downgrade();
    let _reader_token = handle
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
    Ok(())
}

fn read_ready(
    reader: &Rc<RefCell<Session>>,
    mut source: &std::fs::File,
    compositor: &mut Compositor,
) -> PostAction {
    let mut bytes = [0; 4096];
    for _attempt in 0..16 {
        match source.read(&mut bytes) {
            Ok(0) => {
                reader.borrow_mut().refused = true;
                return PostAction::Remove;
            }
            Ok(count) => {
                let deliveries = reader
                    .borrow_mut()
                    .receive(bytes.get(..count).unwrap_or_default());
                compositor.advance_time();
                for delivery in deliveries {
                    match delivery {
                        Delivery::Reply {
                            key,
                            result,
                            submission_id,
                        } => {
                            let rejection = compositor.plugin_reply(key, result);
                            let response = if let Some(request_id) = submission_id {
                                let value = rejection.map_or_else(
                                    || serde_json::json!({"jsonrpc":"2.0", "id":request_id, "result":[]}),
                                    |rejected| serde_json::json!({"jsonrpc":"2.0", "id":request_id, "error":{"code":-32602, "message":rejected.reason}}));
                                serde_json::to_vec(&value).map_err(io::Error::from).and_then(
                                    |response_bytes| reader.borrow_mut().queue_bytes(response_bytes),
                                )
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
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                return PostAction::Continue;
            }
            Err(error) => {
                tracing::warn!(%error, "plugin read failed");
                reader.borrow_mut().refused = true;
                return PostAction::Remove;
            }
        }
    }
    PostAction::Continue
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
}
