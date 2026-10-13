//! Desktop entry discovery without a desktop session.

#[cfg(test)]
mod desktop_entries {
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn ids_include_nested_entries_and_skip_directory_symlinks(name in "[a-z]{1,25}") {
            let directory = tempfile::tempdir().unwrap();
            let applications = directory.path().join("applications");
            std::fs::create_dir_all(applications.join("nested")).unwrap();
            std::fs::write(applications.join(format!("{name}.desktop")), "[Desktop Entry]\n").unwrap();
            std::fs::write(applications.join("nested").join(format!("{name}.desktop")), "[Desktop Entry]\n").unwrap();
            std::fs::write(applications.join("ignored.txt"), "ignored").unwrap();
            std::os::unix::fs::symlink(&applications, applications.join("loop")).unwrap();
            let mut entries = tehuti::desktop::entries_in(&[directory.path().into()]);
            entries.sort();
            let mut expected = vec![format!("{name}.desktop"), format!("nested-{name}.desktop")];
            expected.sort();
            prop_assert_eq!(entries, expected);
        }

        #[test]
        fn invalid_desktop_ids_return_errors_without_starting_a_process(entry_id in prop_oneof![Just(String::new()), "-[a-z]*", "[a-z]*/[a-z]*", "[a-z]*\\x00[a-z]*"]) {
            prop_assert!(tehuti::desktop::launch(&entry_id, std::ffi::OsStr::new("unused")).is_err());
        }
    }
}
