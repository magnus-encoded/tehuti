//! One plugin's protocol state (ADR 0003).
//!
//! Framing, handshake, decision deadlines and invalid-message limits live here. Bytes and the
//! time come in; deliveries and bytes to write go out. No process, pipe or event loop appears.

#![expect(
    clippy::redundant_pub_crate,
    reason = "unreachable_pub rejects bare pub in a crate-private module, so pub(crate) is the only form both lints accept"
)]

use core::time::Duration;
use std::{
    collections::{BTreeMap, VecDeque},
    io,
    time::Instant,
};
use tehuti_protocol::{
    Action, Call, Event, Initialize, MAX_MESSAGE_BYTES, Manifest, Message, Method, PluginKind, ResultValue,
    RpcVersion, Subscription,
};

pub(crate) const HANG_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const INVALID_MESSAGE_LIMIT: usize = 16;
pub(crate) const FLOOD_WINDOW: Duration = Duration::from_secs(1);
pub(crate) const FLOOD_BYTES: usize = 262_144;
pub(crate) const RESTART_BACKOFF_BASE: Duration = Duration::from_millis(100);
pub(crate) const RESTART_BACKOFF_CAP: Duration = Duration::from_secs(30);
pub(crate) const RESTART_BACKOFF_RESET: Duration = Duration::from_secs(30);

/// When a plugin may be started again, and how long the next wait is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Backoff {
    pub(crate) retry_at: Option<Instant>,
    pub(crate) delay: Duration,
}

impl Backoff {
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            retry_at: None,
            delay: RESTART_BACKOFF_BASE,
        }
    }

    /// Whether a start is allowed at `now`.
    #[must_use]
    pub(crate) fn due(&self, now: Instant) -> bool {
        self.retry_at.is_none_or(|deadline| now >= deadline)
    }

    /// Records that a start is being made.
    pub(crate) const fn started(&mut self) {
        self.retry_at = None;
    }

    /// Schedules the next start after the current delay, then doubles the delay up to the cap.
    pub(crate) fn schedule(&mut self, now: Instant) {
        self.retry_at = now.checked_add(self.delay);
        self.delay = self.delay.saturating_mul(2).min(RESTART_BACKOFF_CAP);
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

/// What a session hands the compositor after reading plugin output.
#[derive(Debug)]
pub(crate) enum Delivery {
    Reply {
        key: tehuti_core::plugin::RequestKey,
        result: Result<Vec<Action>, String>,
        submission_id: Option<u64>,
    },
    /// The plugin handled the request itself: no actions and no fallback.
    Handled(tehuti_core::plugin::RequestKey),
    Notice(Vec<Action>),
    PadOverrides(Vec<tehuti_protocol::PadOverride>),
    /// The plugin replaced its speech hotwords.
    SpeechHints,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Starting,
    Ready,
    Refused,
    Failed,
}

#[derive(Debug)]
pub(crate) struct Session {
    pub(crate) output: VecDeque<u8>,
    pub(crate) input: Vec<u8>,
    pub(crate) dropping_line: bool,
    pub(crate) status: Status,
    pub(crate) initialize_deadline: Instant,
    pub(crate) latest_focus: Option<(Option<String>, String)>,
    pub(crate) events: Vec<Subscription>,
    pub(crate) manifest: Manifest,
    pub(crate) next_id: u64,
    pub(crate) plugin: usize,
    pub(crate) invalid_messages: usize,
    pub(crate) missed_decisions: usize,
    pub(crate) healthy_since: Option<Instant>,
    pub(crate) pending: BTreeMap<u64, Instant>,
    pub(crate) read_window: Instant,
    pub(crate) read_bytes: usize,
    pub(crate) status_text: Option<String>,
    /// A verb plugin's current verbs; empty for application plugins.
    pub(crate) verbs: Vec<String>,
    /// The hotwords the plugin last sent; a restart starts empty.
    pub(crate) hints: Vec<String>,
    /// The invoke words the plugin last sent; a restart starts empty.
    pub(crate) invoke_words: Vec<String>,
    /// The standing dictation language this plugin set; gone with the session.
    pub(crate) language: Option<String>,
}

impl Session {
    /// A session for a plugin that has just been started at `now`; the first message queued is
    /// `initialize`.
    ///
    /// # Errors
    /// The `initialize` message does not fit the output queue.
    pub(crate) fn new(manifest: &Manifest, now: Instant) -> io::Result<Self> {
        let mut session = Self {
            output: VecDeque::new(),
            input: Vec::new(),
            dropping_line: false,
            status: Status::Starting,
            initialize_deadline: now.checked_add(HANG_TIMEOUT).unwrap_or(now),
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
            status_text: None,
            verbs: manifest.verbs.clone(),
            hints: Vec::new(),
            invoke_words: Vec::new(),
            language: None,
        };
        session.queue_initialize()?;
        Ok(session)
    }

    /// The bytes waiting to be written to the plugin's stdin.
    #[must_use]
    pub(crate) fn output_slice(&self) -> &[u8] {
        self.output.as_slices().0
    }

    /// Drops `count` bytes the adapter wrote.
    pub(crate) fn written(&mut self, count: usize) {
        let _written = self.output.drain(..count);
    }

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

    pub(crate) fn accept_bytes(&mut self, count: usize, now: Instant) -> bool {
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

    pub(crate) fn fail(&mut self, reason: &str) {
        if self.status != Status::Failed {
            tracing::warn!(plugin = self.plugin, executable = %self.manifest.executable, reason, "plugin restart scheduled");
            self.status = Status::Failed;
        }
    }

    pub(crate) fn invalid(&mut self, now: Instant) {
        self.invalid_messages = self.invalid_messages.saturating_add(1);
        self.healthy_since = (self.status == Status::Ready).then_some(now);
        if self.invalid_messages >= INVALID_MESSAGE_LIMIT {
            self.fail("repeated invalid messages");
        }
    }

    pub(crate) fn supervise(&mut self, now: Instant) {
        self.expire_initialize(now);
        let expired = self.pending.values().filter(|deadline| now >= **deadline).count();
        self.pending.retain(|_, deadline| now < *deadline);
        self.missed_decisions = self.missed_decisions.saturating_add(usize::from(expired > 0));
        if self.missed_decisions >= 3 {
            self.fail("three consecutive plugin decisions missed");
        }
    }

    /// One supervision tick: expires deadlines, applies a child exit (`exit`, the reason the
    /// adapter found), and resets `backoff` once the session has stayed healthy long enough.
    /// Returns whether the session has failed and must be replaced.
    pub(crate) fn supervise_child(
        &mut self,
        now: Instant,
        exit: Option<&str>,
        backoff: &mut Backoff,
    ) -> bool {
        self.supervise(now);
        if let Some(reason) = exit {
            self.fail(reason);
        }
        if self.status != Status::Failed
            && self
                .healthy_since
                .is_some_and(|since| now.duration_since(since) >= RESTART_BACKOFF_RESET)
        {
            backoff.delay = RESTART_BACKOFF_BASE;
            self.invalid_messages = 0;
        }
        self.status == Status::Failed
    }

    /// Whether the manifest lists this application; focus notices go to no other plugin.
    #[must_use]
    pub(crate) fn owns_app(&self, app_id: Option<&String>) -> bool {
        app_id.is_some_and(|app_id| self.manifest.app_ids.contains(app_id))
    }

    pub(crate) fn observe(&mut self, event: &Event) {
        if let Event::Focus { app_id, title } = event {
            self.latest_focus = Some((app_id.clone(), title.clone()));
        }
    }

    pub(crate) fn request(&mut self) -> Option<u64> {
        let request_id = self.next_id;
        self.next_id = self.next_id.checked_add(1)?;
        Some(request_id)
    }

    /// Queues `call` for the plugin and starts the decision clock for a request.
    ///
    /// # Errors
    /// The pending queue or the output queue is full, or the call cannot be encoded.
    pub(crate) fn queue_event(&mut self, call: &Call, now: Instant) -> io::Result<()> {
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
            && let Some(deadline) = now.checked_add(budget)
        {
            let _deadline = self.pending.insert(request_id, deadline);
        }
        Ok(())
    }

    /// Queues `call` for the plugin.
    ///
    /// # Errors
    /// The output queue is full or the call cannot be encoded.
    pub(crate) fn queue(&mut self, call: &Call) -> io::Result<()> {
        self.queue_bytes(serde_json::to_vec(call)?)
    }

    /// Queues one message line for the plugin.
    ///
    /// # Errors
    /// The output queue is full.
    pub(crate) fn queue_bytes(&mut self, bytes: Vec<u8>) -> io::Result<()> {
        if self.output.len().saturating_add(bytes.len()).saturating_add(1) > MAX_MESSAGE_BYTES {
            return Err(io::Error::other("plugin output queue full"));
        }
        self.output.extend(bytes);
        self.output.push_back(b'\n');
        Ok(())
    }

    pub(crate) fn receive(&mut self, bytes: &[u8], now: Instant) -> Vec<Delivery> {
        let mut actions = Vec::new();
        for byte in bytes {
            if *byte == b'\n' {
                if !self.dropping_line {
                    let line = core::mem::take(&mut self.input);
                    match tehuti_protocol::parse_message(&line) {
                        Ok(message) => actions.extend(self.message(message, now)),
                        Err(error) => {
                            tracing::warn!(%error, "plugin message dropped");
                            actions.extend(self.invalid_message(&line, error));
                            self.invalid(now);
                        }
                    }
                }
                self.dropping_line = false;
            } else if !self.dropping_line {
                if self.input.len() == MAX_MESSAGE_BYTES {
                    self.input.clear();
                    self.dropping_line = true;
                    tracing::warn!("overlong plugin line dropped");
                    self.invalid(now);
                } else {
                    self.input.push(*byte);
                }
            }
        }
        actions
    }

    pub(crate) fn invalid_message(&mut self, line: &[u8], reason: String) -> Vec<Delivery> {
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

    pub(crate) fn expire_initialize(&mut self, now: Instant) {
        if self.status == Status::Starting && now >= self.initialize_deadline {
            self.fail("plugin initialize timed out");
        }
    }

    pub(crate) fn initialize_reply(&mut self, message: Message, now: Instant) {
        self.expire_initialize(now);
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
                self.healthy_since = Some(now);
                if self.events.contains(&Subscription::Focus)
                    && let Some((app_id, title)) = self.latest_focus.clone()
                    && self.owns_app(app_id.as_ref())
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

    pub(crate) fn notice_actions(&mut self, actions: Vec<Action>, now: Instant) -> Vec<Delivery> {
        match tehuti_core::plugin::validate_actions(actions, &self.manifest.permissions) {
            Ok(actions) => vec![Delivery::Notice(actions)],
            Err(reason) => {
                self.invalid(now);
                tracing::warn!(%reason, "plugin action rejected");
                if let Err(error) = self.reject(None, &reason) {
                    tracing::warn!(%error, "plugin rejection dropped");
                }
                Vec::new()
            }
        }
    }

    pub(crate) fn complete_decision(&mut self, message: &Message, now: Instant) {
        self.supervise(now);
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

    pub(crate) fn message(&mut self, message: Message, now: Instant) -> Vec<Delivery> {
        if let Message::Error(response) = &message {
            tracing::warn!(code = response.error.code, message = %response.error.message, "plugin request declined");
        }
        if matches!(self.status, Status::Refused | Status::Failed) {
            return Vec::new();
        }
        if self.status != Status::Ready {
            self.initialize_reply(message, now);
            return Vec::new();
        }
        self.complete_decision(&message, now);
        match message {
            Message::Response(response) => {
                let key = tehuti_core::plugin::RequestKey {
                    plugin: self.plugin,
                    request_id: response.request_id,
                };
                vec![match response.result {
                    ResultValue::Handled(_) => Delivery::Handled(key),
                    ResultValue::Actions(actions) => Delivery::Reply {
                        key,
                        result: Ok(actions),
                        submission_id: None,
                    },
                    ResultValue::Initialized(_) => Delivery::Reply {
                        key,
                        result: Err("result must be an action list".into()),
                        submission_id: None,
                    },
                }]
            }
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
            Message::Call(call) => self.notice(call.method, now),
        }
    }

    /// Handles a plugin notice that needs no reply.
    pub(crate) fn notice(&mut self, method: Method, now: Instant) -> Vec<Delivery> {
        match method {
            Method::Actions { actions } => self.notice_actions(actions, now),
            Method::Notice { text } => vec![Delivery::Notice(vec![Action::Notice { text }])],
            Method::PadOverrides { overrides } => vec![Delivery::PadOverrides(overrides)],
            Method::Status { text } => {
                self.status_text = text;
                Vec::new()
            }
            Method::Verbs { verbs } => {
                self.set_verbs(verbs, now);
                Vec::new()
            }
            Method::SpeechHints { hotwords } => {
                self.hints = hotwords;
                vec![Delivery::SpeechHints]
            }
            Method::SpeechLanguage { language } => {
                self.language = Some(language);
                Vec::new()
            }
            Method::InvokeWords { words } => {
                self.invoke_words = words;
                vec![Delivery::SpeechHints]
            }
            Method::Adjuster { .. }
            | Method::Initialize(_)
            | Method::Button { .. }
            | Method::Speech { .. }
            | Method::Verb { .. }
            | Method::Focus { .. }
            | Method::SubmitActions { .. }
            | Method::PadInput { .. } => Vec::new(),
        }
    }

    /// Replaces a verb plugin's verbs; an application plugin may not declare any.
    pub(crate) fn set_verbs(&mut self, verbs: Vec<String>, now: Instant) {
        if self.manifest.kind == PluginKind::Verb {
            self.verbs = verbs;
            return;
        }
        let reason = "an application plugin declares no verbs";
        self.invalid(now);
        tracing::warn!(reason, "plugin verbs rejected");
        if let Err(error) = self.reject(None, reason) {
            tracing::warn!(%error, "plugin rejection dropped");
        }
    }

    pub(crate) fn call_for_event(&mut self, event: &Event, selected: bool) -> Option<Call> {
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
            Event::Focus { app_id, title }
                if self.events.contains(&Subscription::Focus) && self.owns_app(app_id.as_ref()) =>
            {
                (
                    None,
                    Method::Focus {
                        app_id: app_id.clone(),
                        title: title.clone(),
                    },
                )
            }
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

    /// Tells the plugin its action was rejected.
    ///
    /// # Errors
    /// The output queue is full.
    pub(crate) fn reject(&mut self, request_id: Option<u64>, reason: &str) -> io::Result<()> {
        self.queue_bytes(serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0", "method": "action_rejected",
            "params": { "event_id": request_id, "error": { "code": -32602, "message": reason } }
        }))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use tehuti_protocol::{ButtonName, ButtonState, Permission, StartPolicy};

    fn after(now: Instant, elapsed: Duration) -> Instant {
        now.checked_add(elapsed).unwrap()
    }

    fn manifest() -> Manifest {
        Manifest {
            adjusters: Vec::new(),
            plugin_id: None,
            author: None,
            security_contact: None,
            kind: PluginKind::Application,
            verbs: Vec::new(),
            executable: "plugin".into(),
            app_ids: vec!["test".into()],
            events: vec![Subscription::Button, Subscription::Speech],
            start: StartPolicy::Always,
            permissions: vec![Permission::Key],
        }
    }

    fn ready(now: Instant) -> Session {
        let mut session = Session::new(&manifest(), now).unwrap();
        session.status = Status::Ready;
        session.events = vec![Subscription::Button, Subscription::Speech];
        session
    }

    fn call(session: &mut Session, button: bool, now: Instant) -> u64 {
        let request_id = session.request().unwrap();
        let method = if button {
            Method::Button {
                name: ButtonName::East,
                state: ButtonState::Pressed,
            }
        } else {
            Method::Speech {
                text: tehuti_protocol::Text::new("hello".into()).unwrap(),
            }
        };
        session
            .queue_event(
                &Call {
                    jsonrpc: RpcVersion::Two,
                    request_id: Some(request_id),
                    method,
                },
                now,
            )
            .unwrap();
        request_id
    }

    proptest! {
        #[test]
        fn a_request_expires_exactly_at_its_budget(button in any::<bool>(), early in 0_u64..100_000, late in 0_u64..10_000) {
            let start = Instant::now();
            let budget = if button {
                tehuti_core::plugin::BUTTON_BUDGET
            } else {
                tehuti_core::plugin::SPEECH_BUDGET
            };
            let mut session = ready(start);
            let request_id = call(&mut session, button, start);
            let before = Duration::from_micros(early).min(budget.saturating_sub(Duration::from_micros(1)));
            session.supervise(after(start, before));
            prop_assert!(session.pending.contains_key(&request_id), "still within budget");
            prop_assert_eq!(session.missed_decisions, 0);
            session.supervise(after(start, budget.saturating_add(Duration::from_millis(late))));
            prop_assert!(session.pending.is_empty(), "expired at the budget");
            prop_assert_eq!(session.missed_decisions, 1);
        }

        #[test]
        fn three_missed_decisions_in_a_row_fail_the_session(button in any::<bool>()) {
            let mut now = Instant::now();
            let mut session = ready(now);
            for miss in 1..=3 {
                let _request = call(&mut session, button, now);
                now = after(now, Duration::from_secs(3));
                session.supervise(now);
                prop_assert_eq!(session.status == Status::Failed, miss == 3);
            }
        }

        #[test]
        fn a_silent_plugin_fails_the_handshake_after_two_seconds(before in 0_u64..2_000, late in 0_u64..60_000) {
            let start = Instant::now();
            let mut session = Session::new(&manifest(), start).unwrap();
            session.supervise(after(
                start,
                Duration::from_millis(before).min(HANG_TIMEOUT.saturating_sub(Duration::from_millis(1))),
            ));
            prop_assert_eq!(session.status, Status::Starting);
            session.supervise(after(start, HANG_TIMEOUT.saturating_add(Duration::from_millis(late))));
            prop_assert_eq!(session.status, Status::Failed);
        }
    }

    #[test]
    fn the_button_limit_is_shorter_than_the_speech_limit() {
        assert!(tehuti_core::plugin::BUTTON_BUDGET < tehuti_core::plugin::SPEECH_BUDGET);
        let start = Instant::now();
        let mut session = ready(start);
        let button = call(&mut session, true, start);
        let speech = call(&mut session, false, start);
        session.supervise(after(start, tehuti_core::plugin::BUTTON_BUDGET));
        assert!(!session.pending.contains_key(&button), "button expired first");
        assert!(session.pending.contains_key(&speech), "speech still waiting");
    }

    #[test]
    fn a_late_reply_is_dropped() {
        let start = Instant::now();
        let mut session = ready(start);
        let request_id = call(&mut session, true, start);
        let late = after(start, tehuti_core::plugin::BUTTON_BUDGET);
        session.supervise(late);
        let line = format!("{{\"jsonrpc\":\"2.0\",\"id\":{request_id},\"result\":{{\"actions\":[]}}}}\n");
        let _deliveries = session.receive(line.as_bytes(), late);
        assert_eq!(
            session.missed_decisions, 1,
            "the miss stands; the late reply does not clear it"
        );
    }

    #[test]
    fn backoff_doubles_to_the_cap_and_resets_when_healthy() {
        let now = Instant::now();
        let mut backoff = Backoff::new();
        for _ in 0..20 {
            backoff.schedule(now);
        }
        assert_eq!(backoff.delay, RESTART_BACKOFF_CAP);
        let mut session = ready(now);
        session.healthy_since = Some(now);
        let healthy = after(now, RESTART_BACKOFF_RESET);
        assert!(!session.supervise_child(healthy, None, &mut backoff));
        assert_eq!(backoff.delay, RESTART_BACKOFF_BASE);
        assert!(session.supervise_child(healthy, Some("exited"), &mut backoff));
    }

    #[test]
    fn an_unknown_field_is_rejected_and_named() {
        let now = Instant::now();
        let mut session = ready(now);
        let request_id = call(&mut session, false, now);
        let line = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"submit_actions\",\"params\":{{\"event_id\":{request_id},\"actions\":[]}},\"bogus\":1}}\n"
        );
        let deliveries = session.receive(line.as_bytes(), now);
        let [
            Delivery::Reply {
                result,
                submission_id,
                ..
            },
        ] = deliveries.as_slice()
        else {
            panic!("one rejected reply expected");
        };
        let reason = result.as_ref().unwrap_err();
        assert!(reason.contains("bogus"), "the error names the field: {reason}");
        assert_eq!(*submission_id, Some(9));
        assert_eq!(session.invalid_messages, 1);
        assert!(session.pending.is_empty(), "the request is answered");
    }

    #[test]
    fn unknown_fields_in_a_notice_are_rejected_and_the_plugin_is_told() {
        let now = Instant::now();
        let mut session = ready(now);
        let line = b"{\"jsonrpc\":\"2.0\",\"method\":\"actions\",\"params\":{\"actions\":[]},\"bogus\":1}\n";
        assert!(session.receive(line, now).is_empty());
        let sent = String::from_utf8(session.output.iter().copied().collect()).unwrap();
        assert!(
            sent.contains("action_rejected") && sent.contains("bogus"),
            "{sent}"
        );
    }
}
