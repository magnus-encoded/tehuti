//! JSON boundaries for plugin programs.

use serde::{Deserialize, Serialize};

use crate::{Action, ButtonName, ButtonState};

pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// An entry name in a secret store, never its contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretReference {
    pub entry: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subscription {
    Button,
    Focus,
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
    Initialize(Initialize),
    Button { name: ButtonName, state: ButtonState },
    Focus { app_id: Option<String>, title: String },
    Actions { actions: Vec<Action> },
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
#[serde(untagged)]
pub enum Message {
    Call(Call),
    Response(Response),
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
        let call: Call = serde_json::from_value(value).map_err(|error| error.to_string())?;
        let needs_reply = matches!(call.method, Method::Initialize(_) | Method::Button { .. });
        if needs_reply != call.request_id.is_some() {
            return Err("initialize/button require an request_id; focus/actions must be notices".into());
        }
        Ok(Message::Call(call))
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
            ResultValue::Actions(serde_json::from_value(envelope.result).map_err(|error| error.to_string())?)
        };
        Ok(Message::Response(Response {
            jsonrpc: envelope.jsonrpc,
            request_id: envelope.request_id,
            result,
        }))
    }
}
