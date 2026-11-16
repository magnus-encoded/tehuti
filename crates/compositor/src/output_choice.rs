//! Which connected DRM connector to try first, given the configured fallback.

/// Reads `[output] fallback = "<connector name>"` from `config.toml` source text.
///
/// A missing table, a missing key, an empty name or unparsable text all mean "no fallback".
#[must_use]
pub fn fallback_from_config(source: &str) -> Option<String> {
    let table = toml::from_str::<toml::Table>(source).ok()?;
    let name = table.get("output")?.as_table()?.get("fallback")?.as_str()?.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// Indices into `connected` in the order a rescan should try them.
///
/// Every connector stays in the list, so a rescan never ends with nothing lit while one is
/// connected. The `fallback` connector moves behind all the others.
#[must_use]
pub fn try_order(connected: &[&str], fallback: Option<&str>) -> Vec<usize> {
    let (mut rest, held): (Vec<usize>, Vec<usize>) =
        (0..connected.len()).partition(|index| Some(connected[*index]) != fallback);
    rest.extend(held);
    rest
}

/// The configured fallback connector name, read from `$XDG_CONFIG_HOME/tehuti/config.toml`.
#[must_use]
pub fn fallback_from_env() -> Option<String> {
    let path = crate::display_mode_store::config_root()
        .ok()?
        .join("tehuti/config.toml");
    fallback_from_config(&std::fs::read_to_string(path).ok()?)
}

/// The kernel's connector name, such as `HDMI-A-1`, from the interface's variant name and id.
#[must_use]
pub fn connector_name(interface: &str, id: u32) -> String {
    let kernel = match interface {
        "DVII" => "DVI-I",
        "DVID" => "DVI-D",
        "DVIA" => "DVI-A",
        "SVideo" => "SVIDEO",
        "NinePinDIN" => "DIN",
        "DisplayPort" => "DP",
        "HDMIA" => "HDMI-A",
        "HDMIB" => "HDMI-B",
        "EmbeddedDisplayPort" => "eDP",
        other => other,
    };
    format!("{kernel}-{id}")
}
