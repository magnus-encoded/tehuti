//! Connector mode ordering and validated environment requests.

use core::cmp::Reverse;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ListedMode {
    pub size: (u16, u16),
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
            refresh: refresh.parse().ok()?,
        };
        (request.size.0 > 0 && request.size.1 > 0 && request.refresh > 0).then_some(request)
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

pub(super) fn retry<State, Value, Error>(
    state: &mut State,
    mut attempt: impl FnMut(&mut State) -> Result<Value, Error>,
    mut fallback: impl FnMut(&mut State, Error) -> Result<bool, Error>,
    mut restore: impl FnMut(&mut State),
) -> Result<Option<Value>, Error> {
    loop {
        match attempt(state) {
            Ok(value) => return Ok(Some(value)),
            Err(error) => {
                if !fallback(state, error)? {
                    restore(state);
                    return Ok(None);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ListedMode, Request, ordered, retry};
    use proptest::prelude::*;

    const MODES: [ListedMode; 4] = [
        ListedMode {
            size: (3840, 2160),
            refresh: 60,
            preferred: false,
        },
        ListedMode {
            size: (1920, 1080),
            refresh: 240,
            preferred: false,
        },
        ListedMode {
            size: (3840, 2160),
            refresh: 144,
            preferred: false,
        },
        ListedMode {
            size: (3840, 2160),
            refresh: 120,
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
                refresh: 144,
                preferred: false,
            },
            ListedMode {
                size: (1920, 1080),
                refresh: 60,
                preferred: true,
            },
            ListedMode {
                size: (1280, 720),
                refresh: 60,
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
            |_, error| Ok(error != "exhausted"),
            |_| {},
        );
        assert_eq!(result, Ok(Some((1920, 1080))));
    }

    #[test]
    fn exhausted_modes_keep_the_session_running() {
        let result = retry(
            &mut [false, false].into_iter(),
            |candidates| match candidates.next() {
                Some(_) => Err::<(), _>("modeset rejected"),
                None => Err("exhausted"),
            },
            |_, error| Ok(error != "exhausted"),
            |_| {},
        );
        assert_eq!(result, Ok(None));
    }

    #[test]
    fn rejected_candidates_restore_the_last_working_mode() {
        let mut state = (1920, [3840, 2560, 1280].into_iter());
        let result = retry(
            &mut state,
            |(width, _)| Err::<(), _>(*width),
            |(width, candidates), _| {
                candidates.next().map_or(Ok(false), |candidate| {
                    *width = candidate;
                    Ok(true)
                })
            },
            |(width, _)| *width = 1920,
        );
        assert_eq!(result, Ok(None));
        assert_eq!(state.0, 1920);
    }

    #[test]
    fn frame_errors_outside_modesetting_reach_the_caller() {
        let result = retry(
            &mut (),
            |()| Err::<(), _>("device lost"),
            |(), error| Err(error),
            |()| {},
        );
        assert_eq!(result, Err("device lost"));
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
