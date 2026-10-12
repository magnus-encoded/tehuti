//! A broken context cannot prevent other contexts from loading.

#[cfg(test)]
mod contexts {
    use core::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn invalid_context_is_skipped_while_valid_context_loads() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "tehuti-contexts-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(directory.join("valid")).unwrap();
        std::fs::write(directory.join("context.toml"), "invalid toml").unwrap();
        std::fs::write(
            directory.join("valid/context.toml"),
            "app_id = 'test'\n[buttons.east]\nkey = 'tab'",
        )
        .unwrap();
        let loaded = tehuti::contexts::load(&directory);
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(
            loaded.len(),
            1,
            "one broken file leaves the valid context available"
        );
        assert_eq!(
            loaded.first().unwrap().app_id(),
            "test",
            "the valid context loads"
        );
    }
    fn directory() -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "tehuti-context-boundary-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn unavailable_directory_loads_no_contexts() {
        let directory = directory();
        assert_eq!(tehuti::contexts::load(&directory), vec![]);
        std::fs::write(&directory, "not a directory").unwrap();
        assert_eq!(tehuti::contexts::load(&directory), vec![]);
        std::fs::remove_file(directory).unwrap();
    }

    #[test]
    fn oversized_and_non_regular_contexts_are_skipped() {
        use std::os::unix::fs::symlink;

        let directory = directory();
        for child in ["oversized", "directory", "symlink", "valid"] {
            std::fs::create_dir_all(directory.join(child)).unwrap();
        }
        let valid = "app_id = 'test'\n[buttons.east]\nkey = 'tab'";
        std::fs::write(directory.join("valid/context.toml"), valid).unwrap();
        std::fs::write(
            directory.join("oversized/context.toml"),
            format!("{valid}\n#{}", "x".repeat(64 * 1024)),
        )
        .unwrap();
        std::fs::create_dir_all(directory.join("directory/context.toml")).unwrap();
        symlink("/dev/zero", directory.join("symlink/context.toml")).unwrap();
        let loaded = tehuti::contexts::load(&directory);
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].app_id(), "test");
    }
}
