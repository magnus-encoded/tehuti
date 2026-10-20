//! Desktop entry cache reads use the startup snapshot.

#[cfg(test)]
mod desktop_cache {

    #[test]
    fn cache_reads_do_not_rescan_removed_entries() {
        let directory = tempfile::tempdir().unwrap();
        let applications = directory.path().join("applications");
        std::fs::create_dir(&applications).unwrap();
        std::fs::write(applications.join("example.desktop"), "[Desktop Entry]\n").unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "desktop_cache::isolated_cache_probe"])
            .env("XDG_DATA_HOME", directory.path())
            .env("XDG_DATA_DIRS", directory.path())
            .env("TEHUTI_CACHE_PROBE", &applications)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
    }

    #[test]
    fn isolated_cache_probe() {
        let Some(applications) = std::env::var_os("TEHUTI_CACHE_PROBE") else {
            return;
        };
        let mut cache = tehuti::desktop::EntryCache::new().unwrap();
        assert!(cache.entries().iter().any(|entry| entry == "example.desktop"));
        std::fs::remove_dir_all(applications).unwrap();
        let _updated = cache.poll();
        assert!(cache.entries().iter().any(|entry| entry == "example.desktop"));
    }
}
