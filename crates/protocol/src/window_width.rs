//! The `[window] width` setting: how it parses and how Tehuti rewrites it to a fraction.

use core::ops::{Div, Sub};

/// How a context's `[window] width` was written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowWidth {
    /// An exact fraction (`"2/3"`, `1`) or a named preset: already canonical.
    Fraction(f64),
    /// A decimal such as `0.667`; Tehuti rewrites it to a simple fraction.
    Decimal(f64),
    /// A pixel count; it needs the screen width and is rewritten to a fraction.
    Pixels(u32),
}

impl Eq for WindowWidth {}

/// The value of a `"2/3"` string, if the denominator is not zero.
pub(crate) fn parse_fraction(text: &str) -> Option<f64> {
    let (numerator, denominator) = text.split_once('/').unwrap_or((text, "1"));
    let numerator: u32 = numerator.trim().parse().ok()?;
    let denominator: u32 = denominator.trim().parse().ok()?;
    (denominator != 0).then(|| f64::from(numerator).div(f64::from(denominator)))
}

/// A fraction in (0, 1], or an error message.
pub(crate) fn check_fraction(value: f64) -> Result<f64, &'static str> {
    if value.is_finite() && value > 0.0_f64 && value <= 1.0_f64 {
        Ok(value)
    } else {
        Err("window size must be greater than 0 and at most 1")
    }
}

impl<'de> serde::Deserialize<'de> for WindowWidth {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Setting {
            Whole(i64),
            Decimal(f64),
            Text(String),
        }
        let fail = serde::de::Error::custom;
        match Setting::deserialize(deserializer)? {
            Setting::Whole(1) => Ok(Self::Fraction(1.0_f64)),
            Setting::Whole(pixels) => u32::try_from(pixels)
                .ok()
                .filter(|pixels| *pixels > 0)
                .map(Self::Pixels)
                .ok_or_else(|| fail("window width must be a fraction or a positive pixel count")),
            Setting::Decimal(value) => check_fraction(value).map(Self::Decimal).map_err(fail),
            Setting::Text(text) => {
                let value = match text.as_str() {
                    "one-third" => 1.0_f64.div(3.0_f64),
                    "one-half" => 0.5_f64,
                    "two-thirds" => 2.0_f64.div(3.0_f64),
                    "full" => 1.0_f64,
                    other => parse_fraction(other)
                        .ok_or_else(|| fail("window width must be a fraction such as \"2/3\""))?,
                };
                check_fraction(value).map(Self::Fraction).map_err(fail)
            }
        }
    }
}

/// The simple fraction (denominator 1 to 6) nearest to `value`, as numerator and denominator.
#[must_use]
pub fn nearest_fraction(value: f64) -> (u32, u32) {
    let mut best = (1, 1);
    let mut error = f64::INFINITY;
    for denominator in 1..=6_u32 {
        for numerator in 1..=denominator {
            let distance = value.sub(f64::from(numerator).div(f64::from(denominator))).abs();
            if distance < error.sub(1e-12_f64) {
                error = distance;
                best = (numerator, denominator);
            }
        }
    }
    best
}

/// The value of a simple fraction.
#[must_use]
pub fn fraction_value((numerator, denominator): (u32, u32)) -> f64 {
    f64::from(numerator).div(f64::from(denominator.max(1)))
}

/// Rewrites the `width` line of the `[window]` table to `"n/d"`, keeping the old value as a comment.
///
/// `screen` is the pixel width a pixel value was measured against. Everything
/// else in `source` is kept byte for byte. `None` when there is no such line.
#[must_use]
pub fn rewrite_window_width(source: &str, fraction: (u32, u32), screen: Option<u32>) -> Option<String> {
    let mut in_window = false;
    let mut done = false;
    let mut output = String::new();
    for line in source.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let eol = line.strip_prefix(body).unwrap_or_default();
        let trimmed = body.trim_start();
        let indent = body.strip_suffix(trimmed).unwrap_or_default();
        if trimmed.starts_with('[') {
            in_window = trimmed
                .split('#')
                .next()
                .is_some_and(|head| head.trim() == "[window]");
        }
        let found = ["column_width", "width"].into_iter().find_map(|key| {
            let value = trimmed.strip_prefix(key)?.trim_start().strip_prefix('=')?;
            Some((key, value))
        });
        if let (true, false, Some((key, value))) = (in_window, done, found) {
            let (value, comment) = value
                .split_once('#')
                .map_or((value.trim(), None), |(value, comment)| {
                    (value.trim(), Some(comment))
                });
            let was = screen.map_or_else(|| value.to_owned(), |screen| format!("{value} px on {screen} px"));
            let (numerator, denominator) = fraction;
            output.push_str(&format!(
                "{indent}{key} = \"{numerator}/{denominator}\"  # was {was}"
            ));
            if let Some(comment) = comment {
                output.push_str(&format!(" #{comment}"));
            }
            output.push_str(eol);
            done = true;
        } else {
            output.push_str(line);
        }
    }
    done.then_some(output)
}

/// A screen width in whole pixels, at least 1.
#[must_use]
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to the u32 range and rounded before the cast"
)]
pub fn whole_pixels(width: f64) -> u32 {
    width.round().clamp(1.0_f64, f64::from(u32::MAX)) as u32
}
