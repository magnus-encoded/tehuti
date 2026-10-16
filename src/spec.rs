use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ValueType {
    Path,
    Glob,
    Regex,
    Integer,
    Enum,
    String,
    Command,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Move {
    pub key: String,
    pub summary: String,
    pub takes: Option<ValueType>,
    pub group: String,
    pub values: Option<BTreeMap<String, String>>,
    pub operand: bool,
    pub required: bool,
    pub repeat: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub summary: String,
    pub order: Vec<String>,
    pub moves: Vec<Move>,
}

/// Load the bundled, hand-written specifications.
///
/// # Errors
/// Returns an error if a bundled data file violates the specification schema.
pub fn specs() -> Result<Vec<CommandSpec>, serde_json::Error> {
    [
        include_str!("../specs/find.json"),
        include_str!("../specs/xargs.json"),
        include_str!("../specs/cat.json"),
        include_str!("../specs/grep.json"),
        include_str!("../specs/wc.json"),
        include_str!("../specs/sort.json"),
        include_str!("../specs/head.json"),
        include_str!("../specs/ls.json"),
    ]
    .into_iter()
    .map(serde_json::from_str)
    .collect()
}

pub(crate) fn raw_move() -> Move {
    Move {
        key: "argument".into(),
        summary: "a raw argument, passed as typed".into(),
        takes: Some(ValueType::String),
        group: "raw".into(),
        values: None,
        operand: true,
        required: false,
        repeat: true,
    }
}
