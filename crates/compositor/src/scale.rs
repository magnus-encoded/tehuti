//! Integer output scale from `TEHUTI_SCALE`, applied to a Smithay output.

use smithay::{
    output::{Mode, Output, Scale},
    utils::{Logical, Physical, Size},
};

/// Modes wider than this get scale 2 unless `TEHUTI_SCALE` says otherwise.
const HIGH_DENSITY_WIDTH: i32 = 2560;

/// The scale used when `TEHUTI_SCALE` is unset or invalid.
#[must_use]
pub const fn default_scale(mode_width: i32) -> i32 {
    if mode_width > HIGH_DENSITY_WIDTH { 2 } else { 1 }
}

/// Picks the scale from a `TEHUTI_SCALE` value.
///
/// # Errors
///
/// Returns the default scale and a warning when the value is not a positive integer.
pub fn choose(value: Option<&str>, mode_width: i32) -> Result<i32, (i32, String)> {
    let fallback = default_scale(mode_width);
    match value.map(|text| (text, text.trim().parse::<i32>())) {
        None => Ok(fallback),
        Some((_, Ok(scale))) if scale > 0 => Ok(scale),
        Some((text, _)) => Err((
            fallback,
            format!("TEHUTI_SCALE={text:?} is not a positive integer; using {fallback}"),
        )),
    }
}

/// Reads `TEHUTI_SCALE`, warning and falling back to the default on invalid values.
#[must_use]
pub fn from_env(mode_width: i32) -> i32 {
    let value = std::env::var("TEHUTI_SCALE").ok();
    choose(value.as_deref(), mode_width).unwrap_or_else(|(scale, warning)| {
        tracing::warn!("{warning}");
        scale
    })
}

/// The layout size of a mode at an integer scale.
#[must_use]
pub fn logical_size(mode: Size<i32, Physical>, scale: i32) -> Size<i32, Logical> {
    mode.to_logical(scale.max(1))
}

/// Sets the mode and integer scale, so clients receive `wl_output.scale`.
pub fn apply(output: &Output, mode: Mode, scale: i32) {
    output.change_current_state(Some(mode), None, Some(Scale::Integer(scale.max(1))), None);
}
