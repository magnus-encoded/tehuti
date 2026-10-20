//! Version queries work without a desktop or runtime directory.

#[path = "../build_metadata.rs"]
pub mod build_metadata;

#[cfg(test)]
mod version {
    use super::build_metadata;
    use std::process::Command;

    #[test]
    fn version_flags_exit_before_starting_the_compositor() {
        for flag in ["--version", "-V"] {
            let output = Command::new(env!("CARGO_BIN_EXE_tehuti"))
                .arg(flag)
                .env_clear()
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(0), "{output:?}");
            assert!(output.stderr.is_empty(), "{output:?}");
            let text = String::from_utf8(output.stdout).unwrap();
            assert_eq!(
                text,
                format!(
                    "tehuti {} ({} {})\n",
                    env!("CARGO_PKG_VERSION"),
                    env!("TEHUTI_GIT_HASH"),
                    env!("TEHUTI_BUILD_DATE")
                )
            );
            let hash = env!("TEHUTI_GIT_HASH");
            assert!(hash == "unknown" || hash.len() >= 9);
            assert!((hash == "unknown") || hash.bytes().all(|byte| byte.is_ascii_hexdigit()));
            let date = env!("TEHUTI_BUILD_DATE");
            assert!(date == "unknown" || chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok());
        }
    }

    #[test]
    fn unavailable_git_discards_hash_and_watch_paths() {
        assert_eq!(build_metadata::git_metadata(|_| None), ("unknown".into(), vec![]));
        let mut responses = [Some("abcdef123".into()), Some("HEAD".into()), None].into_iter();
        assert_eq!(
            build_metadata::git_metadata(|_| responses.next().flatten()),
            ("unknown".into(), vec![])
        );
    }

    #[test]
    fn build_date_uses_only_a_valid_source_date_epoch() {
        for epoch in [None, Some("invalid"), Some("9223372036854775807")] {
            assert_eq!(build_metadata::build_date(epoch), "unknown");
        }
        assert_eq!(build_metadata::build_date(Some("0")), "1970-01-01");
        assert_eq!(build_metadata::build_date(Some("1709251199")), "2024-02-29");
        assert_eq!(build_metadata::build_date(Some("1709251200")), "2024-03-01");
    }
}
