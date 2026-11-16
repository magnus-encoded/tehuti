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

#[cfg(test)]
mod closing {
    use tehuti::drm_holders::{Closed, Holder, closable, close, warning};

    fn holder(pid: u32, command: &str) -> Holder {
        Holder {
            pid,
            command: command.into(),
        }
    }

    #[test]
    fn tehuti_and_init_are_never_closable() {
        let holders = [holder(1, "init"), holder(50, "self"), holder(900, "player")];
        assert_eq!(closable(&holders, 50), vec![holder(900, "player")]);
    }

    #[test]
    fn closing_signals_each_closable_holder_once_and_reports_failures() {
        let holders = [holder(50, "self"), holder(300, "kept"), holder(900, "player")];
        let mut signalled = Vec::new();
        let closed = close(&holders, 50, |pid| {
            signalled.push(pid);
            if pid == 300 {
                Err(std::io::Error::from_raw_os_error(1))
            } else {
                Ok(())
            }
        });
        assert_eq!(signalled, [300, 900]);
        assert_eq!(closed.closed, vec![holder(900, "player")]);
        assert_eq!(closed.failed.len(), 1);
        assert_eq!(closed.failed[0].0, holder(300, "kept"));
        assert_eq!(close(&[], 50, |_| panic!("nothing to close")), Closed::default());
    }

    #[test]
    fn the_handoff_warning_names_each_holder_and_is_silent_without_any() {
        assert_eq!(warning(&[]), None);
        let text = warning(&[holder(900, "player"), holder(12, "clock")]).unwrap();
        assert!(text.contains("player") && text.contains("900"), "{text}");
        assert!(text.contains("clock") && text.contains("12"), "{text}");
    }

    #[test]
    fn terminate_ends_a_program_that_handles_the_signal_by_default() {
        let mut child = std::process::Command::new("sleep").arg("60").spawn().unwrap();
        tehuti::drm_holders::terminate(child.id()).unwrap();
        let status = child.wait().unwrap();
        assert!(!status.success());
        assert!(tehuti::drm_holders::terminate(0).is_err());
    }
}

#[cfg(test)]
mod ownership {
    use std::os::unix::fs::MetadataExt;
    use tehuti::drm_holders::{Holder, owned_by};

    #[test]
    fn only_holders_of_the_given_user_remain() {
        let proc_root = tempfile::tempdir().unwrap();
        std::fs::create_dir(proc_root.path().join("900")).unwrap();
        let mine = std::fs::metadata(proc_root.path().join("900")).unwrap().uid();
        let holders = [
            Holder {
                pid: 900,
                command: "player".into(),
            },
            Holder {
                pid: 901,
                command: "gone".into(),
            },
        ];
        assert_eq!(
            owned_by(proc_root.path(), &holders, mine),
            vec![holders[0].clone()]
        );
        assert_eq!(owned_by(proc_root.path(), &holders, mine + 1), vec![]);
    }
}
