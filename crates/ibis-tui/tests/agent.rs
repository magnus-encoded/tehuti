#[cfg(test)]
mod agent {
    use core::error::Error;
    use core::time::Duration;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ibis_commands::{Input, State, specs};
    use ibis_tui::InputLine;
    use ibis_tui::agent::{Session, Socket};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};

    fn session() -> Result<Session, Box<dyn Error>> {
        Ok(Session::new(State::new(".".into(), specs()?), ".".into()))
    }

    fn request(
        socket: &mut Socket,
        session: &mut Session,
        client: &mut BufReader<UnixStream>,
        line: &str,
    ) -> Result<serde_json::Value, Box<dyn Error>> {
        writeln!(client.get_mut(), "{line}")?;
        socket.poll(session)?;
        let mut reply = String::new();
        let _count = client.read_line(&mut reply)?;
        Ok(serde_json::from_str(&reply)?)
    }

    #[test]
    fn socket_and_keyboard_share_builder_and_command_execution() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("agent.sock");
        let mut socket = Socket::bind(&path)?;
        let mut remote = session()?;
        let mut local = session()?;
        let stream = UnixStream::connect(&path)?;
        stream.set_read_timeout(Some(Duration::from_secs(1)))?;
        let mut client = BufReader::new(stream);
        let initial = local.state().screen().to_string();
        assert_eq!(
            request(&mut socket, &mut remote, &mut client, r#"["screen"]"#)?["screen"],
            initial
        );
        let mut line = InputLine::default();
        for (json, code) in [
            (r#"["press","Y"]"#, Some(KeyCode::F(4))),
            (r#"["say","echo hello"]"#, None),
            (r#"["press","A"]"#, Some(KeyCode::F(1))),
            (r#"["press","A"]"#, Some(KeyCode::F(1))),
        ] {
            let input = if let Some(code) = code {
                line.key(KeyEvent::new(code, KeyModifiers::NONE))
                    .ok_or("missing key input")?
            } else {
                for character in "echo hello ".chars() {
                    let _input = line.key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
                }
                line.quiet(Duration::from_secs(1)).ok_or("missing speech input")?
            };
            local.input(input);
            let reply = request(&mut socket, &mut remote, &mut client, json)?;
            assert_eq!(reply["screen"], local.state().screen().to_string());
        }
        assert_eq!(remote.state().history().len(), 1);
        assert_eq!(
            remote
                .state()
                .history()
                .first()
                .ok_or("missing history")?
                .output
                .trim(),
            "hello"
        );
        assert_eq!(
            request(&mut socket, &mut remote, &mut client, r#"["reset"]"#)?["screen"],
            initial
        );
        Ok(())
    }

    #[test]
    fn partial_and_invalid_requests_leave_other_clients_usable() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("agent.sock");
        let mut socket = Socket::bind(&path)?;
        let mut state = session()?;
        let mut partial = UnixStream::connect(&path)?;
        partial.write_all(b"[\"scr")?;
        socket.poll(&mut state)?;
        let stream = UnixStream::connect(&path)?;
        stream.set_read_timeout(Some(Duration::from_secs(1)))?;
        let mut client = BufReader::new(stream);
        for invalid in [
            "not json",
            r#"["press","Y","bogus"]"#,
            r#"["say"]"#,
            r#"["unknown"]"#,
        ] {
            assert!(request(&mut socket, &mut state, &mut client, invalid)?["error"].is_string());
        }
        assert_eq!(state.state().screen().segments, []);
        partial.write_all(b"een\"]\n[\"screen\"]\n")?;
        socket.poll(&mut state)?;
        socket.poll(&mut state)?;
        partial.set_read_timeout(Some(Duration::from_secs(1)))?;
        let mut partial = BufReader::new(partial);
        for _ in 0..2 {
            let mut reply = String::new();
            let _count = partial.read_line(&mut reply)?;
            assert!(serde_json::from_str::<serde_json::Value>(&reply)?["screen"].is_string());
        }
        let _reply = request(&mut socket, &mut state, &mut client, r#"["press","Y"]"#)?;
        let reply = request(&mut socket, &mut state, &mut client, r#"["type","echo"]"#)?;
        let mut expected = session()?;
        expected.input(Input::Press(ibis_commands::Button::New));
        expected.input(Input::Type("echo".into()));
        assert_eq!(reply["screen"], expected.state().screen().to_string());
        drop(partial);
        socket.poll(&mut state)?;
        assert!(request(&mut socket, &mut state, &mut client, r#"["screen"]"#)?["screen"].is_string());
        Ok(())
    }

    #[test]
    fn client_and_line_limits_preserve_the_session() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("agent.sock");
        let mut socket = Socket::bind(&path)?;
        assert_eq!(std::fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
        let mut state = session()?;
        let mut clients = Vec::new();
        for _ in 0..16 {
            clients.push(UnixStream::connect(&path)?);
        }
        socket.poll(&mut state)?;
        let mut excess = UnixStream::connect(&path)?;
        excess.set_read_timeout(Some(Duration::from_secs(1)))?;
        socket.poll(&mut state)?;
        assert_eq!(excess.read(&mut [0])?, 0);
        drop(clients);
        socket.poll(&mut state)?;
        let stream = UnixStream::connect(&path)?;
        stream.set_read_timeout(Some(Duration::from_secs(1)))?;
        let mut client = BufReader::new(stream);
        client.get_mut().write_all(&vec![b'x'; 65537])?;
        for _ in 0..17 {
            socket.poll(&mut state)?;
        }
        assert_eq!(client.read(&mut [0])?, 0);
        let healthy_stream = UnixStream::connect(&path)?;
        healthy_stream.set_read_timeout(Some(Duration::from_secs(1)))?;
        let mut healthy_client = BufReader::new(healthy_stream);
        assert!(
            request(&mut socket, &mut state, &mut healthy_client, r#"["screen"]"#)?["screen"].is_string()
        );
        Ok(())
    }

    #[test]
    fn stale_socket_is_replaced_active_socket_and_files_are_preserved() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("agent.sock");
        drop(UnixListener::bind(&path)?);
        let socket = Socket::bind(&path)?;
        assert!(Socket::bind(&path).is_err());
        drop(socket);
        assert!(!path.exists());
        std::fs::write(&path, "keep")?;
        assert!(Socket::bind(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path)?, "keep");
        Ok(())
    }
}
