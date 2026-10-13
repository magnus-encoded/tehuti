//! Deterministic plugin decisions and their fallback deadlines.

use core::time::Duration;
use tehuti_protocol::{Action, Event, Permission};

use crate::State;

pub const BUTTON_BUDGET: Duration = Duration::from_millis(100);
pub const BUTTON_LATENCY_BUDGET: Duration = Duration::from_millis(250);
pub const SPEECH_BUDGET: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestKey {
    pub plugin: usize,
    pub request_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Request {
        key: RequestKey,
        event: Event,
        permissions: Vec<Permission>,
    },
    Reply {
        key: RequestKey,
        result: Result<Vec<Action>, String>,
    },
    Tick {
        elapsed: Duration,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub key: RequestKey,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pending {
    key: RequestKey,
    remaining: Duration,
    fallback: Vec<Action>,
    permissions: Vec<Permission>,
}

/// Advances plugin routing using only supplied inputs.
#[must_use]
pub fn step(mut state: State, input: Input) -> (State, Vec<Action>, Option<Rejection>) {
    match input {
        Input::Request {
            key,
            event,
            permissions,
        } => {
            let fallback = crate::active_context(&state, &event)
                .or_else(|| crate::fallback(&event))
                .into_iter()
                .collect();
            if state.pending.iter().any(|pending| pending.key == key) {
                return (state, Vec::new(), None);
            }
            if state
                .pending
                .iter()
                .filter(|pending| pending.key.plugin == key.plugin)
                .count()
                >= 256
            {
                return (state, fallback, None);
            }
            let remaining = if matches!(event, Event::Speech { .. }) {
                SPEECH_BUDGET
            } else {
                BUTTON_BUDGET
            };
            state.pending.push(Pending {
                key,
                remaining,
                fallback,
                permissions,
            });
            (state, Vec::new(), None)
        }
        Input::Reply { key, result } => {
            let Some(index) = state.pending.iter().position(|pending| pending.key == key) else {
                return (state, Vec::new(), None);
            };
            let pending = state.pending.remove(index);
            match result.and_then(|actions| validate_actions(actions, &pending.permissions)) {
                Ok(actions) if !actions.is_empty() => (state, actions, None),
                Ok(_) => (state, pending.fallback, None),
                Err(reason) => (state, pending.fallback, Some(Rejection { key, reason })),
            }
        }
        Input::Tick { elapsed } => {
            let actions = expire(&mut state, elapsed);
            (state, actions, None)
        }
    }
}

pub(crate) fn expire(state: &mut State, elapsed: Duration) -> Vec<Action> {
    let mut actions = Vec::new();
    state.pending.retain_mut(|pending| {
        pending.remaining = pending.remaining.saturating_sub(elapsed);
        if pending.remaining.is_zero() {
            actions.append(&mut pending.fallback);
            false
        } else {
            true
        }
    });
    actions
}

/// Checks the whole batch before any action can be applied.
///
/// # Errors
/// An action is reserved for the core or lacks its manifest permission.
pub fn validate_actions(actions: Vec<Action>, permissions: &[Permission]) -> Result<Vec<Action>, String> {
    for (index, action) in actions.iter().enumerate() {
        let (name, permission) = match action {
            Action::Key { .. } => ("key", Some(Permission::Key)),
            Action::Text { .. } => ("text", Some(Permission::Text)),
            Action::Quit => ("quit", Some(Permission::Quit)),
            Action::SecretRequest { .. } => ("secret_request", None),
            Action::Launch { .. } => ("launch", None),
            Action::Close => ("close", None),
            Action::Notice { .. } => ("notice", None),
        };
        match permission {
            Some(permission) if permissions.contains(&permission) => {}
            Some(_) => return Err(format!("action {index} ({name}): missing {name} permission")),
            None => return Err(format!("action {index} ({name}): reserved for the core")),
        }
    }
    Ok(actions)
}
