//! Reserved speech prefixes owned by core.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationKind {
    Application,
    Creation,
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherRequest {
    pub kind: DestinationKind,
    pub query: String,
}

/// Launcher verbs followed by a query, in match order.
const QUERY_VERBS: [(&str, DestinationKind); 4] = [
    ("open ", DestinationKind::Application),
    ("åpne ", DestinationKind::Application),
    ("work on ", DestinationKind::Project),
    ("work ", DestinationKind::Project),
];

/// The reserved launcher word a phrase starts with, when the core would take it before contexts.
#[must_use]
pub fn reserved_word(phrase: &str) -> Option<&'static str> {
    let normalized = tehuti_protocol::normalize_phrase(phrase);
    let request = launcher_request(phrase)?;
    if normalized == "new" {
        return Some("new");
    }
    QUERY_VERBS
        .iter()
        .find(|(verb, kind)| *kind == request.kind && normalized.starts_with(verb))
        .map(|(verb, _)| verb.trim_end())
}

/// Recognizes a whole verb followed by one or more query words.
#[must_use]
pub fn launcher_request(phrase: &str) -> Option<LauncherRequest> {
    let normalized = tehuti_protocol::normalize_phrase(phrase);
    if normalized == "new" {
        return Some(LauncherRequest {
            kind: DestinationKind::Creation,
            query: String::new(),
        });
    }
    let (kind, query) = QUERY_VERBS
    .into_iter()
    .find_map(|(verb, kind)| normalized.strip_prefix(verb).map(|query| (kind, query)))?;
    (!(query.is_empty() || kind == DestinationKind::Project && query == "on")).then(|| LauncherRequest {
        kind,
        query: query.into(),
    })
}
