//! Connector mode ordering and validated environment requests.

use core::cmp::Reverse;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ListedMode {
    pub size: (u16, u16),
    /// Millihertz, so 29.97 and 30 stay distinct.
    pub refresh: u32,
    pub preferred: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Request {
    size: (u16, u16),
    refresh: u32,
}

impl Request {
    pub(super) fn parse(value: &str) -> Option<Self> {
        let (size, refresh) = value.split_once('@')?;
        let (width, height) = size.split_once('x')?;
        let request = Self {
            size: (width.parse().ok()?, height.parse().ok()?),
            refresh: millihertz(refresh)?,
        };
        (request.size.0 > 0 && request.size.1 > 0 && request.refresh > 0).then_some(request)
    }
}

/// Parses `30`, `29.97` or `23.976` into millihertz.
fn millihertz(value: &str) -> Option<u32> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty() || fraction.len() > 3 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let digits = u32::try_from(fraction.len()).ok()?;
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u32>().ok()?
    };
    whole
        .parse::<u32>()
        .ok()?
        .checked_mul(1000)?
        .checked_add(fraction.checked_mul(10u32.pow(3_u32.saturating_sub(digits)))?)
}

/// `30` for whole rates, otherwise two decimals such as `29.97`.
pub(super) fn rate_label(millihertz: u32) -> String {
    let hundredths = millihertz.saturating_add(5) / 10;
    if hundredths.is_multiple_of(100) {
        format!("{}", hundredths / 100)
    } else {
        format!("{}.{:02}", hundredths / 100, hundredths % 100)
    }
}

pub(super) fn ordered(modes: &[ListedMode], request: Option<Request>) -> (Vec<usize>, bool) {
    let mut indices: Vec<_> = (0..modes.len()).collect();
    indices.sort_by_key(|index| {
        let mode = modes.get(*index);
        Reverse(mode.map(|mode| {
            (
                mode.preferred,
                u32::from(mode.size.0).saturating_mul(u32::from(mode.size.1)),
                mode.size,
                mode.refresh,
            )
        }))
    });
    let matched = request.and_then(|request| {
        indices
            .iter()
            .copied()
            .filter(|index| modes.get(*index).is_some_and(|mode| mode.size == request.size))
            .min_by_key(|index| {
                modes
                    .get(*index)
                    .map(|mode| mode.refresh.abs_diff(request.refresh))
            })
    });
    if let Some(index) = matched {
        indices.retain(|candidate| *candidate != index);
        indices.insert(0, index);
    }
    (indices, matched.is_some())
}

pub(super) fn ordered_with_saved(
    modes: &[ListedMode],
    request: Option<Request>,
    saved: Option<Request>,
) -> (Vec<usize>, bool) {
    let (mut indices, matched) = ordered(modes, request);
    if let Some(index) = saved.and_then(|saved| {
        // Labels round to hundredths, so 23.976 is saved as 23.98.
        modes
            .iter()
            .enumerate()
            .filter(|(_, mode)| mode.size == saved.size && mode.refresh.abs_diff(saved.refresh) <= 5)
            .min_by_key(|(_, mode)| mode.refresh.abs_diff(saved.refresh))
            .map(|(index, _)| index)
    }) && (!matched || indices.first() != Some(&index))
    {
        indices.retain(|candidate| *candidate != index);
        indices.insert(usize::from(matched), index);
    }
    (indices, matched)
}

pub(super) fn retry<State, Value, Error>(
    state: &mut State,
    mut attempt: impl FnMut(&mut State) -> Result<Value, Error>,
    mut fallback: impl FnMut(&mut State, Error) -> Result<(), Error>,
) -> Result<Value, Error> {
    loop {
        match attempt(state) {
            Ok(value) => return Ok(value),
            Err(error) => fallback(state, error)?,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ListedMode, Request, ordered, ordered_with_saved, retry};
    use proptest::prelude::*;

    const MODES: [ListedMode; 4] = [
        ListedMode {
            size: (3840, 2160),
            refresh: 60_000,
            preferred: false,
        },
        ListedMode {
            size: (1920, 1080),
            refresh: 240_000,
            preferred: false,
        },
        ListedMode {
            size: (3840, 2160),
            refresh: 144_000,
            preferred: false,
        },
        ListedMode {
            size: (3840, 2160),
            refresh: 120_000,
            preferred: true,
        },
    ];

    #[test]
    fn preferred_precedes_resolution_and_refresh() {
        assert_eq!(ordered(&MODES, None).0, [3, 2, 0, 1]);
        let mut modes = MODES;
        for mode in &mut modes {
            mode.preferred = false;
        }
        assert_eq!(ordered(&modes, None).0, [2, 3, 0, 1]);
    }

    #[test]
    fn override_uses_nearest_listed_refresh_then_default_fallbacks() {
        assert_eq!(
            ordered(&MODES, Request::parse("3840x2160@140")),
            (vec![2, 3, 0, 1], true)
        );
        assert_eq!(
            ordered(&MODES, Request::parse("1280x720@60")),
            (vec![3, 2, 0, 1], false)
        );
        assert_eq!(ordered(&[], Request::parse("3840x2160@120")), (vec![], false));
    }

    #[test]
    fn preferred_resolution_wins_and_override_can_select_a_smaller_mode() {
        let modes = [
            ListedMode {
                size: (3840, 2160),
                refresh: 144_000,
                preferred: false,
            },
            ListedMode {
                size: (1920, 1080),
                refresh: 60_000,
                preferred: true,
            },
            ListedMode {
                size: (1280, 720),
                refresh: 60_000,
                preferred: false,
            },
        ];
        assert_eq!(ordered(&modes, None).0, [1, 0, 2]);
        assert_eq!(
            ordered(&modes, Request::parse("1280x720@120")),
            (vec![2, 1, 0], true)
        );
    }

    #[test]
    fn saved_mode_precedes_preferred_but_environment_override_wins() {
        let saved = Request::parse("3840x2160@60");
        assert_eq!(ordered_with_saved(&MODES, None, saved).0, [0, 3, 2, 1]);
        assert_eq!(
            ordered_with_saved(&MODES, Request::parse("3840x2160@144"), saved).0,
            [2, 0, 3, 1]
        );
        assert_eq!(ordered_with_saved(&MODES, saved, saved).0, [0, 3, 2, 1]);
        assert_eq!(
            ordered_with_saved(&MODES, None, Request::parse("3840x2160@90")).0,
            [3, 2, 0, 1]
        );
        assert_eq!(
            ordered_with_saved(&MODES, None, Request::parse("1280x720@60")).0,
            [3, 2, 0, 1]
        );
    }

    #[test]
    fn fractional_rates_parse_and_label_distinctly() {
        assert_eq!(super::millihertz("29.97"), Some(29_970));
        assert_eq!(super::millihertz("23.976"), Some(23_976));
        assert_eq!(super::millihertz("30"), Some(30_000));
        assert_eq!(super::rate_label(29_970), "29.97");
        assert_eq!(super::rate_label(23_976), "23.98");
        assert_eq!(super::rate_label(30_000), "30");
        assert_eq!(super::rate_label(59_940), "59.94");
        let ntsc = Request::parse("3840x2160@29.97").map(|request| request.refresh);
        assert_eq!(ntsc, Some(29_970));
    }

    #[test]
    fn malformed_requests_are_rejected() {
        for value in [
            "",
            "3840x2160",
            "0x2160@120",
            "3840x0@120",
            "3840x2160@0",
            "3840x2160@-1",
            "65536x2160@120",
            "3840x2160@120junk",
            "3840x2160@29.9701",
            "3840x2160@.5",
        ] {
            assert!(Request::parse(value).is_none(), "accepted {value}");
        }
    }

    #[test]
    fn failed_modesets_reach_a_lower_resolution_without_ending_the_session() {
        let mut candidates = [3, 2, 0, 1].into_iter();
        let result = retry(
            &mut candidates,
            |candidates| match candidates.next() {
                Some(1) => Ok((1920, 1080)),
                Some(_) => Err("modeset rejected"),
                None => Err("exhausted"),
            },
            |_, error| if error == "exhausted" { Err(error) } else { Ok(()) },
        );
        assert_eq!(result, Ok((1920, 1080)));
    }

    #[test]
    fn exhausted_modes_return_an_error() {
        let result = retry(
            &mut [false, false].into_iter(),
            |candidates| match candidates.next() {
                Some(_) => Err::<(), _>("modeset rejected"),
                None => Err("exhausted"),
            },
            |_, error| if error == "exhausted" { Err(error) } else { Ok(()) },
        );
        assert_eq!(result, Err("exhausted"));
    }

    proptest! {
        #[test]
        fn ordering_preserves_exactly_the_driver_list(
            sizes in prop::collection::vec((1_u16..=8192, 1_u16..=8192, 1_u32..=360, any::<bool>()), 0..40),
        ) {
            let modes: Vec<_> = sizes.into_iter().map(|(width, height, refresh, preferred)| ListedMode { size: (width, height), refresh, preferred }).collect();
            let (indices, _) = ordered(&modes, None);
            let mut sorted = indices.clone();
            sorted.sort_unstable();
            prop_assert_eq!(sorted, (0..modes.len()).collect::<Vec<_>>());
            for pair in indices.windows(2) {
                if let [first, second] = pair {
                    let first = modes.get(*first);
                    let second = modes.get(*second);
                    if let (Some(first), Some(second)) = (first, second) {
                        prop_assert!(first.preferred || !second.preferred);
                        if first.preferred == second.preferred {
                            let first_area = u32::from(first.size.0).saturating_mul(u32::from(first.size.1));
                            let second_area = u32::from(second.size.0).saturating_mul(u32::from(second.size.1));
                            prop_assert!(first_area >= second_area);
                            if first.size == second.size {
                                prop_assert!(first.refresh >= second.refresh);
                            }
                        }
                    }
                }
            }
        }
    }
}
