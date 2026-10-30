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
    let (kind, query) = [
        ("open ", DestinationKind::Application),
        ("åpne ", DestinationKind::Application),
        ("work on ", DestinationKind::Project),
        ("work ", DestinationKind::Project),
        ("new ", DestinationKind::Creation),
    ]
    .into_iter()
    .find_map(|(verb, kind)| normalized.strip_prefix(verb).map(|query| (kind, query)))?;
    (!(query.is_empty() || kind == DestinationKind::Project && query == "on")).then(|| LauncherRequest {
        kind,
        query: query.into(),
    })
}
