//! `tehuti --version` status lines on fake runtime, `/proc` and device directories.

#[cfg(test)]
mod status {
    use std::{fs, os::unix::fs::symlink, path::Path};

    use tehuti::{
        drm_holders::Holder,
        instance,
        status::{Status, gather},
    };

    fn holder(proc_root: &Path, pid: u32, command: &str, device: &Path) {
        let directory = proc_root.join(pid.to_string());
        fs::create_dir_all(directory.join("fd")).unwrap();
        fs::write(directory.join("comm"), format!("{command}\n")).unwrap();
        symlink(device, directory.join("fd/3")).unwrap();
    }

    #[test]
    fn a_quiet_machine_reports_no_session_holders_or_sockets() {
        let runtime = tempfile::tempdir().unwrap();
        let proc_root = tempfile::tempdir().unwrap();
        let dri = tempfile::tempdir().unwrap();
        let status = gather(Some(runtime.path()), proc_root.path(), dri.path());
        assert_eq!(
            status,
            Status {
                known_runtime: true,
                ..Status::default()
            }
        );
        let text = status.to_string();
        assert!(text.contains("session: not running"), "{text}");
        assert!(text.contains("drm holders: none"), "{text}");
        assert!(text.contains("sockets: none"), "{text}");
    }

    #[test]
    fn a_live_session_shows_its_pid_and_lock() {
        let runtime = tempfile::tempdir().unwrap();
        let _lock = instance::acquire(runtime.path()).unwrap();
        fs::write(runtime.path().join("wayland-7"), "").unwrap();
        fs::write(runtime.path().join("unrelated"), "").unwrap();
        let proc_root = tempfile::tempdir().unwrap();
        let dri = tempfile::tempdir().unwrap();
        let status = gather(Some(runtime.path()), proc_root.path(), dri.path());
        assert_eq!(status.session_pid, Some(std::process::id()));
        assert_eq!(status.sockets, ["tehuti.lock", "wayland-7"]);
        let text = status.to_string();
        assert!(
            text.contains(&format!("session: pid {}", std::process::id())),
            "{text}"
        );
    }

    #[test]
    fn a_stale_lock_file_is_not_a_running_session() {
        let runtime = tempfile::tempdir().unwrap();
        fs::write(runtime.path().join(instance::LOCK_FILE_NAME), "4242\n").unwrap();
        assert_eq!(instance::running_pid(runtime.path()), None);
    }

    #[test]
    fn holders_are_listed_per_card() {
        let runtime = tempfile::tempdir().unwrap();
        let proc_root = tempfile::tempdir().unwrap();
        let dri = tempfile::tempdir().unwrap();
        let card = dri.path().join("card1");
        fs::write(&card, "").unwrap();
        fs::write(dri.path().join("renderD128"), "").unwrap();
        holder(proc_root.path(), 900, "player", &card);
        let status = gather(Some(runtime.path()), proc_root.path(), dri.path());
        assert_eq!(
            status.devices,
            vec![(
                card.clone(),
                vec![Holder {
                    pid: 900,
                    command: "player".into()
                }]
            )]
        );
        let text = status.to_string();
        assert!(
            text.contains(&format!("{}: player (pid 900)", card.display())),
            "{text}"
        );
    }

    #[test]
    fn an_unknown_runtime_directory_is_said_plainly() {
        let proc_root = tempfile::tempdir().unwrap();
        let dri = tempfile::tempdir().unwrap();
        let text = gather(None, proc_root.path(), dri.path()).to_string();
        assert!(text.contains("session: unknown"), "{text}");
    }
}
