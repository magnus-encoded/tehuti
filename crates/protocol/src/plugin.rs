//! JSON boundaries for plugin programs.

use serde::{Deserialize, Serialize};

use crate::{Action, ButtonName, ButtonState};

pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// An entry name in a secret store, never its contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
#[serde(deny_unknown_fields)]
pub struct SecretReference {
    pub entry: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subscription {
    Button,
    Focus,
    Speech,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartPolicy {
    Always,
    OnFocus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Key,
    Text,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adjusters: Vec<crate::Adjuster>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "id")]
    pub plugin_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_contact: Option<String>,
    pub executable: String,
    pub app_ids: Vec<String>,
    pub events: Vec<Subscription>,
    pub start: StartPolicy,
    pub permissions: Vec<Permission>,
}

/// Parses a JSON manifest without reading files or executing programs.
///
/// # Errors
/// Invalid JSON, unknown fields or empty executable/application ids.
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest, String> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err("manifest exceeds 64 KiB".into());
    }
    let manifest: Manifest = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if manifest.executable.trim().is_empty() || manifest.app_ids.iter().any(|name| name.trim().is_empty()) {
        return Err("executable and application ids must not be empty".into());
    }
    let mut names = std::collections::BTreeSet::new();
    if manifest.adjusters.len() > 32
        || manifest
            .adjusters
            .iter()
            .any(|adjuster| !names.insert(adjuster.name()))
    {
        return Err("adjuster names must be unique, with at most 32 registrations".into());
    }
    Ok(manifest)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Initialize {
    pub protocol_version: u64,
    pub encoding: String,
    pub core_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<std::collections::BTreeMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Initialized {
    pub protocol_version: u64,
    pub encoding: String,
    pub events: Vec<Subscription>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Method {
    Adjuster {
        name: String,
        value: i32,
        finished: bool,
    },
    Initialize(Initialize),
    Button {
        name: ButtonName,
        state: ButtonState,
    },
    Speech {
        text: crate::Text,
    },
    Focus {
        app_id: Option<String>,
        title: String,
    },
    Actions {
        actions: Vec<Action>,
    },
    SubmitActions {
        event_id: u64,
        actions: Vec<Action>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RpcVersion {
    #[serde(rename = "2.0")]
    Two,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    pub jsonrpc: RpcVersion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "id")]
    pub request_id: Option<u64>,
    #[serde(flatten)]
    pub method: Method,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResultValue {
    Initialized(Initialized),
    Actions(Vec<Action>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub jsonrpc: RpcVersion,
    #[serde(rename = "id")]
    pub request_id: u64,
    pub result: ResultValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorResponse {
    pub jsonrpc: RpcVersion,
    #[serde(rename = "id")]
    pub request_id: u64,
    pub error: RpcError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Call(Call),
    Response(Response),
    Error(ErrorResponse),
}

/// Parses one UTF-8 JSON-RPC line, rejecting fields outside the v0 schema.
///
/// # Errors
/// Invalid JSON, unsupported methods, unknown fields or invalid request/notice shape.
pub fn parse_message(bytes: &[u8]) -> Result<Message, String> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err("message exceeds 64 KiB".into());
    }
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if value.get("method").is_some() {
        let object = value.as_object().ok_or("message must be an object")?;
        for field in object.keys() {
            if !["jsonrpc", "id", "method", "params"].contains(&field.as_str()) {
                return Err(format!("unknown field `{field}`"));
            }
        }
        validate_call_actions(&value)?;
        let call: Call = serde_json::from_value(value).map_err(|error| error.to_string())?;
        let needs_reply = matches!(
            call.method,
            Method::Initialize(_)
                | Method::Button { .. }
                | Method::Speech { .. }
                | Method::SubmitActions { .. }
        );
        if needs_reply != call.request_id.is_some() {
            return Err(
                "initialize/button/speech/submit_actions require an id; focus/actions must be notices".into(),
            );
        }
        Ok(Message::Call(call))
    } else if value.get("error").is_some() {
        serde_json::from_value(value)
            .map(Message::Error)
            .map_err(|error| error.to_string())
    } else {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Envelope {
            jsonrpc: RpcVersion,
            #[serde(rename = "id")]
            request_id: u64,
            result: serde_json::Value,
        }
        let envelope: Envelope = serde_json::from_value(value).map_err(|error| error.to_string())?;
        let result = if envelope.result.is_object() {
            ResultValue::Initialized(
                serde_json::from_value(envelope.result).map_err(|error| error.to_string())?,
            )
        } else {
            ResultValue::Actions(parse_actions(&envelope.result)?)
        };
        Ok(Message::Response(Response {
            jsonrpc: envelope.jsonrpc,
            request_id: envelope.request_id,
            result,
        }))
    }
}

fn validate_call_actions(value: &serde_json::Value) -> Result<(), String> {
    if value
        .get("method")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|method| matches!(method, "actions" | "submit_actions"))
    {
        let actions = value
            .get("params")
            .and_then(|params| params.get("actions"))
            .ok_or("actions notice requires actions")?;
        let _validated = parse_actions(actions)?;
    }
    Ok(())
}

/// Validates each action and names the rejected index and action kind.
///
/// # Errors
/// The result is not an action list or an action violates its schema.
pub fn parse_actions(value: &serde_json::Value) -> Result<Vec<Action>, String> {
    let values = value.as_array().ok_or("result must be an action list")?;
    values
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let name = action
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            serde_json::from_value(action.clone())
                .map_err(|error| format!("action {index} ({name}): {error}"))
        })
        .collect()
}
