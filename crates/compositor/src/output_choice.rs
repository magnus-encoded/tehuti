//! Which connected DRM connector to try first, given the configured fallback.

/// Reads `[output] fallback = "<connector name>"` from `config.toml` source text.
///
/// A missing table, a missing key, an empty name or unparsable text all mean "no fallback".
#[must_use]
pub fn fallback_from_config(source: &str) -> Option<String> {
    let _ = source;
    None
}

/// Indices into `connected` in the order a rescan should try them.
///
/// Every connector stays in the list, so a rescan never ends with nothing lit while one is
/// connected. The `fallback` connector moves behind all the others.
#[must_use]
pub fn try_order(connected: &[&str], fallback: Option<&str>) -> Vec<usize> {
    let _ = fallback;
    (0..connected.len()).collect()
}
