//! The command line answers queries and refuses bad starts before any session setup.

#[cfg(test)]
mod cli {
    use std::{
        ffi::OsString,
        path::Path,
        process::{Command, Output},
    };

    use tehuti::cli::{self, Invocation, UsageError};

    fn parse(arguments: &[&str]) -> Result<Invocation, UsageError> {
        cli::parse(arguments.iter().map(OsString::from))
    }

    fn run_in(runtime_directory: &Path, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tehuti"))
            .args(arguments)
            .env_clear()
            .env("XDG_RUNTIME_DIR", runtime_directory)
            .output()
            .unwrap()
    }

    fn entries(directory: &Path) -> Vec<OsString> {
        std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect()
    }

    #[test]
    fn help_and_version_flags_are_queries() {
        assert_eq!(parse(&["--help"]), Ok(Invocation::Help));
        assert_eq!(parse(&["-h"]), Ok(Invocation::Help));
        assert_eq!(parse(&["--version"]), Ok(Invocation::Version));
        assert_eq!(parse(&["-V"]), Ok(Invocation::Version));
    }

    #[test]
    fn arguments_after_the_context_directory_belong_to_the_application() {
        assert_eq!(
            parse(&["contexts", "foot", "--help", "-V"]),
            Ok(Invocation::Session {
                context_directory: "contexts".into(),
                application: Some("foot".into()),
                application_arguments: vec!["--help".into(), "-V".into()],
            })
        );
        assert_eq!(
            parse(&["contexts"]),
            Ok(Invocation::Session {
                context_directory: "contexts".into(),
                application: None,
                application_arguments: vec![],
            })
        );
    }

    #[test]
    fn a_missing_directory_or_unknown_option_is_a_usage_error() {
        assert_eq!(parse(&[]), Err(UsageError::MissingContextDirectory));
        assert_eq!(
            parse(&["--halp"]),
            Err(UsageError::UnknownOption("--halp".into()))
        );
        assert_eq!(parse(&["-"]), Err(UsageError::UnknownOption("-".into())));
    }

    #[test]
    fn help_prints_usage_and_exits_without_touching_the_runtime_directory() {
        for flag in ["--help", "-h"] {
            let runtime = tempfile::tempdir().unwrap();
            let output = run_in(runtime.path(), &[flag]);
            assert_eq!(output.status.code(), Some(0), "{output:?}");
            assert!(output.stderr.is_empty(), "{output:?}");
            assert_eq!(String::from_utf8(output.stdout).unwrap(), cli::USAGE);
            assert_eq!(
                entries(runtime.path()),
                Vec::<OsString>::new(),
                "no socket or lock"
            );
        }
    }

    #[test]
    fn version_exits_without_touching_the_runtime_directory() {
        let runtime = tempfile::tempdir().unwrap();
        let output = run_in(runtime.path(), &["--version"]);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert_eq!(
            entries(runtime.path()),
            Vec::<OsString>::new(),
            "no socket or lock"
        );
    }

    #[test]
    fn usage_errors_exit_2_with_the_usage_on_stderr() {
        for arguments in [&[][..], &["--halp"][..]] {
            let runtime = tempfile::tempdir().unwrap();
            let output = run_in(runtime.path(), arguments);
            assert_eq!(output.status.code(), Some(2), "{output:?}");
            assert!(output.stdout.is_empty(), "{output:?}");
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert!(stderr.contains("usage: tehuti"), "{stderr}");
            assert_eq!(
                entries(runtime.path()),
                Vec::<OsString>::new(),
                "no socket or lock"
            );
        }
    }

    #[test]
    fn a_second_instance_refuses_to_start_while_the_lock_is_held() {
        let runtime = tempfile::tempdir().unwrap();
        let _held = tehuti::instance::acquire(runtime.path()).unwrap();
        let output = run_in(runtime.path(), &["contexts"]);
        assert_eq!(output.status.code(), Some(3), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("another Tehuti is already running"), "{stderr}");
        assert!(
            stderr.contains(&format!("pid {}", std::process::id())),
            "{stderr}"
        );
        assert_eq!(
            entries(runtime.path()),
            vec![OsString::from(tehuti::instance::LOCK_FILE_NAME)],
            "no Wayland socket"
        );
    }
}

#[cfg(test)]
mod output_mode_help {
    #[test]
    fn help_advertises_running_session_output_mode_control() {
        assert!(tehuti::cli::USAGE.contains("tehuti output-mode <W>x<H>[@<Hz>] | --restore"));
    }
}

#[cfg(test)]
mod output_mode_control {
    use core::time::Duration;
    use std::{
        io::{Read, Write},
        os::unix::net::UnixListener,
        process::Command,
    };

    #[test]
    fn commands_send_the_mode_and_restore_over_the_session_socket() {
        let runtime = tempfile::tempdir().unwrap();
        let directory = runtime.path().join("tehuti-control");
        std::fs::create_dir(&directory).unwrap();
        let listener = UnixListener::bind(directory.join("output-mode")).unwrap();
        listener.set_nonblocking(true).unwrap();
        for argument in ["1280x720@60", "--restore"] {
            let mut child = Command::new(env!("CARGO_BIN_EXE_tehuti"))
                .args(["output-mode", argument])
                .env_clear()
                .env("XDG_RUNTIME_DIR", runtime.path())
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let stream = loop {
                if let Ok((stream, _)) = listener.accept() {
                    break Some(stream);
                }
                if child.try_wait().unwrap().is_some() || std::time::Instant::now() >= deadline {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            if let Some(mut stream) = stream {
                stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                let mut bytes = [0; 128];
                let count = stream.read(&mut bytes).unwrap();
                assert_eq!(bytes.get(..count).unwrap(), format!("{argument}\n").as_bytes());
                stream.write_all(b"ok\n").unwrap();
                drop(stream);
                assert!(child.wait().unwrap().success());
            } else {
                let _killed = child.kill();
                let _waited = child.wait();
                panic!("command did not contact the session socket");
            }
        }
    }
}
