//! At most one holder of the instance lock per runtime directory.

#[cfg(test)]
mod instance {
    use tehuti::instance::{InstanceError, acquire};

    #[test]
    fn the_lock_has_one_holder_until_it_is_dropped() {
        let runtime = tempfile::tempdir().unwrap();
        let first = acquire(runtime.path()).unwrap();
        match acquire(runtime.path()) {
            Err(InstanceError::AlreadyRunning { pid, path }) => {
                assert_eq!(pid, Some(std::process::id()));
                assert_eq!(path, runtime.path().join(tehuti::instance::LOCK_FILE_NAME));
            }
            other => panic!("second acquire must fail as already running: {other:?}"),
        }
        drop(first);
        assert!(
            acquire(runtime.path()).is_ok(),
            "a released lock can be taken again"
        );
    }

    #[test]
    fn a_stale_lock_file_without_a_holder_does_not_block() {
        let runtime = tempfile::tempdir().unwrap();
        std::fs::write(runtime.path().join(tehuti::instance::LOCK_FILE_NAME), "999999\n").unwrap();
        assert!(
            acquire(runtime.path()).is_ok(),
            "a file left by a dead instance is reused"
        );
    }

    #[test]
    fn separate_runtime_directories_do_not_share_a_lock() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let _held = acquire(first.path()).unwrap();
        assert!(
            acquire(second.path()).is_ok(),
            "another user's runtime directory is independent"
        );
    }

    #[test]
    fn a_nested_start_takes_no_lock_but_a_console_start_does() {
        let runtime = tempfile::tempdir().unwrap();
        let console = tehuti::instance::acquire_for(tehuti::backend::Backend::Tty, runtime.path()).unwrap();
        assert!(console.is_some());
        assert!(matches!(
            tehuti::instance::acquire_for(tehuti::backend::Backend::Tty, runtime.path()),
            Err(InstanceError::AlreadyRunning { .. })
        ));
        assert!(
            tehuti::instance::acquire_for(tehuti::backend::Backend::Nested, runtime.path())
                .unwrap()
                .is_none(),
            "nested starts run beside the session"
        );
    }
}
