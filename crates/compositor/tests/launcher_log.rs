#[cfg(test)]
mod tests {
    use core::time::Duration;
    use proptest::prelude::*;
    use tehuti_core::launcher::{Ending, Usage};

    fn arbitrary_usage() -> impl Strategy<Value = Usage> {
        (
            any::<String>(),
            prop::collection::vec(any::<String>(), 0..6),
            prop_oneof![
                (1_usize..6).prop_map(|rank| Ending::Chosen { rank }),
                Just(Ending::Cancelled),
                Just(Ending::Replaced),
            ],
            0_u64..1_000_000,
        )
            .prop_map(|(query, candidates, ending, millis)| Usage {
                query,
                candidates,
                ending,
                since_transcript: Duration::from_millis(millis),
            })
    }

    proptest! {
        #[test]
        fn a_logged_use_reads_back_from_a_prefixed_single_log_line(usage in arbitrary_usage(), prefix in "[ -~]{0,40}") {
            let line = format!("{prefix}{}", tehuti::launcher_log::line(&usage));
            prop_assert!(!line.contains('\n'));
            prop_assert_eq!(tehuti::launcher_log::parse(&line), Some(usage));
        }
    }

    #[test]
    fn lines_without_a_launcher_use_are_skipped() {
        assert_eq!(
            tehuti::launcher_log::parse("INFO tehuti: xwayland-satellite started"),
            None
        );
        assert_eq!(tehuti::launcher_log::parse("launcher use {not json"), None);
    }
}
