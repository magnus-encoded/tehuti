//! Holder detection on a fake `/proc`, and the busy-device message.

use std::{fs, os::unix::fs::symlink, path::Path};

#[cfg(test)]
mod holders {
    use super::*;
    use tehuti::drm_holders::{DeviceBusy, Holder, find, is_busy};

    fn process(proc_root: &Path, pid: u32, command: &str, open: &[&str]) {
        let directory = proc_root.join(pid.to_string());
        fs::create_dir_all(directory.join("fd")).unwrap();
        fs::write(directory.join("comm"), format!("{command}\n")).unwrap();
        for (index, target) in open.iter().enumerate() {
            symlink(target, directory.join("fd").join(index.to_string())).unwrap();
        }
    }

    #[test]
    fn holders_are_the_processes_with_the_device_open_sorted_by_pid() {
        let proc_root = tempfile::tempdir().unwrap();
        process(proc_root.path(), 900, "player", &["/dev/null", "/dev/dri/card1"]);
        process(proc_root.path(), 12, "other", &["/dev/dri/card0"]);
        process(
            proc_root.path(),
            300,
            "twice",
            &["/dev/dri/card1", "/dev/dri/card1"],
        );
        fs::create_dir(proc_root.path().join("self")).unwrap();
        fs::write(proc_root.path().join("uptime"), "1 1\n").unwrap();
        assert_eq!(
            find(proc_root.path(), Path::new("/dev/dri/card1")),
            vec![
                Holder {
                    pid: 300,
                    command: "twice".into()
                },
                Holder {
                    pid: 900,
                    command: "player".into()
                },
            ]
        );
    }

    #[test]
    fn an_unreadable_process_is_skipped_not_fatal() {
        let proc_root = tempfile::tempdir().unwrap();
        fs::create_dir_all(proc_root.path().join("5")).unwrap();
        assert_eq!(find(proc_root.path(), Path::new("/dev/dri/card1")), vec![]);
    }

    #[test]
    fn the_busy_message_names_each_holder_and_how_to_recover() {
        let busy = DeviceBusy {
            device: "/dev/dri/card1".into(),
            holders: vec![Holder {
                pid: 900,
                command: "player".into(),
            }],
        };
        let text = busy.to_string();
        assert!(text.contains("/dev/dri/card1"), "{text}");
        assert!(text.contains("player (pid 900)"), "{text}");
        assert!(text.contains("close"), "{text}");
    }

    #[test]
    fn the_busy_message_without_holders_says_none_were_found() {
        let busy = DeviceBusy {
            device: "/dev/dri/card1".into(),
            holders: vec![],
        };
        assert!(busy.to_string().contains("no holder was visible"), "{busy}");
    }

    #[test]
    fn busy_is_found_through_the_source_chain() {
        let busy = std::io::Error::from_raw_os_error(16);
        assert!(is_busy(&busy));
        let wrapped = DeviceBusy {
            device: "x".into(),
            holders: vec![],
        };
        assert!(is_busy(&wrapped));
        let other = std::io::Error::from_raw_os_error(2);
        assert!(!is_busy(&other));
    }
}
