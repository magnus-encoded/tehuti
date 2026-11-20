//! The session is registered with systemd once the socket listens, and never fatally.

#[cfg(test)]
mod registration {
    use std::ffi::OsStr;
    use std::io;
    use tehuti::session_registration::{Invocation, report, start_plan, stop_plan};

    const NAMES: [&str; 5] = [
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_TYPE",
        "XDG_SESSION_DESKTOP",
    ];

    fn env<'a>(invocation: &'a Invocation, name: &str) -> Option<&'a str> {
        invocation
            .envs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn start_imports_the_session_environment_then_starts_the_target() {
        let plan = start_plan(OsStr::new("wayland-7"), Some(OsStr::new(":3")));
        assert_eq!(plan.len(), 3);
        for invocation in &plan[..2] {
            for name in NAMES {
                assert!(
                    invocation.args.iter().any(|argument| argument == name),
                    "{name} imported"
                );
            }
            assert_eq!(env(invocation, "WAYLAND_DISPLAY"), Some("wayland-7"));
            assert_eq!(env(invocation, "DISPLAY"), Some(":3"));
            assert_eq!(env(invocation, "XDG_CURRENT_DESKTOP"), Some("Tehuti"));
            assert_eq!(env(invocation, "XDG_SESSION_TYPE"), Some("wayland"));
            assert_eq!(env(invocation, "XDG_SESSION_DESKTOP"), Some("Tehuti"));
        }
        assert_eq!(plan[0].program, "systemctl");
        assert_eq!(plan[0].args[..2], ["--user", "import-environment"]);
        assert_eq!(plan[1].program, "dbus-update-activation-environment");
        assert_eq!(plan[2].program, "systemctl");
        assert_eq!(plan[2].args, ["--user", "start", "tehuti-session.target"]);
    }

    #[test]
    fn without_x11_display_nothing_stale_is_imported() {
        let plan = start_plan(OsStr::new("wayland-7"), None);
        for invocation in &plan[..2] {
            assert!(!invocation.args.iter().any(|argument| argument == "DISPLAY"));
            assert_eq!(env(invocation, "DISPLAY"), None);
        }
    }

    #[test]
    fn stop_stops_the_target_and_clears_the_imported_names() {
        let plan = stop_plan();
        assert_eq!(plan[0].args, ["--user", "stop", "tehuti-session.target"]);
        assert_eq!(plan[1].args[..2], ["--user", "unset-environment"]);
        for name in NAMES {
            assert!(plan[1].args.iter().any(|argument| argument == name));
        }
    }

    #[test]
    fn every_step_runs_even_when_earlier_ones_fail() {
        let plan = start_plan(OsStr::new("wayland-7"), None);
        let mut attempts = 0;
        let warnings = report(&plan, &mut |_| {
            attempts += 1;
            Err(io::Error::other("no systemd user manager"))
        });
        assert_eq!(attempts, plan.len(), "a failed step never stops the rest");
        assert_eq!(warnings.len(), plan.len(), "each failure is reported for logging");
    }

    #[test]
    fn success_reports_nothing() {
        let plan = start_plan(OsStr::new("wayland-7"), None);
        assert_eq!(report(&plan, &mut |_| Ok(())), Vec::<String>::new());
    }
}
