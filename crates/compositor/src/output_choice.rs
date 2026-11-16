//! Which connected DRM connector to try first, given the configured fallback.

use smithay::reexports::drm::control::connector::Interface;

/// Indices into `connected` in the order a rescan should try them.
///
/// Every connector stays in the list, so a rescan never ends with nothing lit while one is
/// connected. The `fallback` connector moves behind all the others.
#[must_use]
pub fn try_order(connected: &[&str], fallback: Option<&str>) -> Vec<usize> {
    let (mut rest, held): (Vec<_>, Vec<_>) = connected
        .iter()
        .enumerate()
        .partition(|(_, name)| Some(**name) != fallback);
    rest.extend(held);
    rest.into_iter().map(|(index, _)| index).collect()
}

/// The configured fallback connector name, from `$XDG_CONFIG_HOME/tehuti/fallback-output`.
///
/// The file holds one connector name; a missing, empty or unreadable file means no fallback.
#[must_use]
pub fn fallback_from_env() -> Option<String> {
    let store = crate::display_mode_store::Store::from_env()
        .ok()?
        .sibling("fallback-output");
    store.read().ok().flatten().filter(|name| !name.is_empty())
}

/// The kernel's connector name, such as `HDMI-A-1`, from the interface and its number.
#[must_use]
pub fn connector_name(interface: Interface, number: u32) -> String {
    let kernel = match interface {
        Interface::VGA => "VGA",
        Interface::DVII => "DVI-I",
        Interface::DVID => "DVI-D",
        Interface::DVIA => "DVI-A",
        Interface::Composite => "Composite",
        Interface::SVideo => "SVIDEO",
        Interface::LVDS => "LVDS",
        Interface::Component => "Component",
        Interface::NinePinDIN => "DIN",
        Interface::DisplayPort => "DP",
        Interface::HDMIA => "HDMI-A",
        Interface::HDMIB => "HDMI-B",
        Interface::TV => "TV",
        Interface::EmbeddedDisplayPort => "eDP",
        Interface::Virtual => "Virtual",
        Interface::DSI => "DSI",
        Interface::DPI => "DPI",
        Interface::Writeback => "Writeback",
        Interface::SPI => "SPI",
        Interface::USB => "USB",
        Interface::Unknown | _ => "Unknown",
    };
    format!("{kernel}-{number}")
}
