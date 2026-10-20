//! Version queries work without a desktop or runtime directory.

#[cfg(test)]
mod version {
    use std::process::Command;

    #[test]
    fn version_flags_exit_before_starting_the_compositor() {
        for flag in ["--version", "-V"] {
            let output = Command::new(env!("CARGO_BIN_EXE_tehuti"))
                .arg(flag)
                .env_clear()
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
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
            assert!(env!("TEHUTI_GIT_HASH").len() >= 9);
            assert!(
                env!("TEHUTI_GIT_HASH")
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
            );
            assert_eq!(env!("TEHUTI_BUILD_DATE").len(), 10);
        }
    }
}
