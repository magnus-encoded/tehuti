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
        let loaded = tehuti::contexts::load(&directory).unwrap();
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
}
