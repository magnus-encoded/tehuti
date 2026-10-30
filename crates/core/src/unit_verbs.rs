//! Reserved speech prefixes owned by core.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationKind {
    Application,
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
    let (kind, query) = [
        ("open ", DestinationKind::Application),
        ("åpne ", DestinationKind::Application),
        ("work on ", DestinationKind::Project),
    ]
    .into_iter()
    .find_map(|(verb, kind)| normalized.strip_prefix(verb).map(|query| (kind, query)))?;
    (!query.is_empty()).then(|| LauncherRequest {
        kind,
        query: query.into(),
    })
}
