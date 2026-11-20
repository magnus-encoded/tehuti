//! The speech resolver runs off the compositor loop, and its real curl path holds up.

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::{
        io,
        os::unix::fs::PermissionsExt as _,
        path::{Path, PathBuf},
        time::Instant,
    };

    use proptest::prelude::*;
    use smithay::reexports::{
        calloop::{self, EventLoop},
        wayland_server::Display,
    };
    use tehuti::{
        display_mode_store::Store,
        speech::{
            resolver::{ApiKey, Curl, DEADLINE, OptIn, Resolver, Transport, TurnLog},
            resolver_worker::{Job, Link, Pending, Reply, Waiting, Worker},
        },
        state::Compositor,
    };
    use tehuti_protocol::Event;

    const REPLY: &str = r#"{"answers":{"command":{"choice":"mute","confidence":0.95,"probabilities":{"mute":0.95,"pause":0.05}}}}"#;

    /// Sleeps `delay_for` the transcript's first word before answering.
    struct Slow {
        delays: Vec<(&'static str, Duration)>,
    }

    impl Transport for Slow {
        fn post(&self, _key: &ApiKey, body: &str, _deadline: Duration) -> io::Result<String> {
            let value: serde_json::Value = serde_json::from_str(body).unwrap();
            let transcript = value["state"]["transcript"].as_str().unwrap().to_owned();
            if let Some((_, delay)) = self.delays.iter().find(|(word, _)| transcript.starts_with(word)) {
                std::thread::sleep(*delay);
            }
            Ok(REPLY.into())
        }
    }

    fn key() -> Option<ApiKey> {
        ApiKey::new("secret-key-123".into())
    }

    fn worker_into(delays: Vec<(&'static str, Duration)>) -> (Worker, calloop::channel::Channel<Reply>) {
        let (sender, receiver) = calloop::channel::channel();
        let worker = Worker::spawn(Resolver::new(Slow { delays }, key()), move |reply| {
            sender.send(reply).is_ok()
        })
        .unwrap();
        (worker, receiver)
    }

    #[test]
    fn a_slow_resolver_never_blocks_the_loop() {
        let (worker, receiver) = worker_into(vec![("slow", Duration::from_millis(400))]);
        let mut event_loop: EventLoop<Vec<Reply>> = EventLoop::try_new().unwrap();
        let _token = event_loop
            .handle()
            .insert_source(receiver, |event, (), replies| {
                if let calloop::channel::Event::Msg(reply) = event {
                    replies.push(reply);
                }
            })
            .unwrap();
        let started = Instant::now();
        assert!(worker.submit(Job {
            number: 1,
            transcript: "slow words".into(),
            commands: vec!["mute".into(), "pause".into()],
        }));
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "submit waited for the model"
        );
        let mut replies = Vec::new();
        let mut turns = 0_u32;
        while replies.is_empty() && started.elapsed() < Duration::from_secs(5) {
            event_loop
                .dispatch(Duration::from_millis(5), &mut replies)
                .unwrap();
            turns += 1;
        }
        assert_eq!(replies.len(), 1, "the reply arrived through the loop");
        assert!(
            turns >= 20,
            "the loop kept dispatching while the model slept, ran {turns} turns"
        );
        assert!(replies[0].answer.is_some());
    }

    proptest! {
        #[test]
        fn only_the_newest_utterance_takes_its_reply(count in 1_usize..8, order in any::<u64>()) {
            let mut pending = Pending::default();
            let ids: Vec<u64> = (0..count)
                .map(|index| pending.begin(Waiting { transcript: format!("t{index}"), commands: Vec::new() }).0)
                .collect();
            let mut shuffled = ids.clone();
            let length = shuffled.len();
            shuffled.rotate_left(usize::try_from(order % u64::try_from(length).unwrap()).unwrap());
            let taken: Vec<u64> = shuffled.into_iter().filter(|id| pending.finish(*id).is_some()).collect();
            prop_assert_eq!(taken, vec![*ids.last().unwrap()]);
            prop_assert!(!pending.is_waiting());
        }
    }

    fn lines(path: &Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn a_late_reply_for_a_superseded_utterance_is_dropped() {
        let directory = tempfile::tempdir().unwrap();
        let contexts = directory.path().join("contexts");
        std::fs::create_dir_all(contexts.join("editor")).unwrap();
        std::fs::write(
            contexts.join("editor/context.toml"),
            "app_id = 'editor'\nunmatched_speech = 'type'\n[buttons]\n[speech]\n'mute' = { notify = 'muted' }\n'pause' = { notify = 'paused' }\n",
        )
        .unwrap();
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_contexts(tehuti::contexts::load(&contexts));
        compositor.handle(&Event::Focus {
            app_id: Some("editor".into()),
            title: String::new(),
        });
        let opt_in = OptIn::stored_at(Store::new(directory.path().join("opt-in")));
        opt_in.set(true).unwrap();
        let log_path = directory.path().join("turns.jsonl");
        let (worker, receiver) = worker_into(vec![("first", Duration::from_millis(400))]);
        compositor.set_speech_resolver(Link::new(
            worker,
            opt_in,
            Some(TurnLog::stored_at(log_path.clone())),
        ));
        let mut event_loop: EventLoop<Compositor> = EventLoop::try_new().unwrap();
        let _token = event_loop
            .handle()
            .insert_source(receiver, |event, (), compositor| {
                if let calloop::channel::Event::Msg(reply) = event {
                    compositor.resolver_replied(&reply);
                }
            })
            .unwrap();

        let said = |text: &str| Event::Speech {
            text: tehuti_protocol::Text::new(text.into()).unwrap(),
        };
        let started = Instant::now();
        compositor.handle(&said("first unknown words"));
        assert!(compositor.speech_resolver_waiting());
        compositor.handle(&said("second unknown words"));
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "handling waited for the model"
        );
        while compositor.speech_resolver_waiting() && started.elapsed() < Duration::from_secs(5) {
            event_loop
                .dispatch(Duration::from_millis(5), &mut compositor)
                .unwrap();
        }
        // Let the superseded utterance's reply arrive late.
        while started.elapsed() < Duration::from_millis(900) {
            event_loop
                .dispatch(Duration::from_millis(5), &mut compositor)
                .unwrap();
        }
        let turns = lines(&log_path);
        assert_eq!(
            turns.len(),
            2,
            "one line per utterance, the late reply adds none: {turns:?}"
        );
        assert_eq!(turns[0]["heard"], "first unknown words");
        assert_eq!(
            turns[0]["outcome"], "dictated",
            "the superseded utterance dictates in order"
        );
        assert_eq!(turns[1]["heard"], "second unknown words");
        assert_eq!(turns[1]["outcome"], "ran");
        assert_eq!(turns[1]["chosen"]["command"], "mute");
    }

    #[test]
    fn nothing_is_held_without_the_opt_in() {
        let directory = tempfile::tempdir().unwrap();
        let contexts = directory.path().join("contexts");
        std::fs::create_dir_all(contexts.join("editor")).unwrap();
        std::fs::write(
            contexts.join("editor/context.toml"),
            "app_id = 'editor'\nunmatched_speech = 'type'\n[buttons]\n[speech]\n'mute' = { notify = 'muted' }\n",
        )
        .unwrap();
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_contexts(tehuti::contexts::load(&contexts));
        compositor.handle(&Event::Focus {
            app_id: Some("editor".into()),
            title: String::new(),
        });
        let (worker, _receiver) = worker_into(Vec::new());
        compositor.set_speech_resolver(Link::new(
            worker,
            OptIn::stored_at(Store::new(directory.path().join("opt-in"))),
            None,
        ));
        compositor.handle(&Event::Speech {
            text: tehuti_protocol::Text::new("unknown words".into()).unwrap(),
        });
        assert!(!compositor.speech_resolver_waiting());
    }

    fn script(directory: &Path, body: &str) -> PathBuf {
        let path = directory.join("fake-curl");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn a_missing_curl_fails_the_post_and_reports_unavailable() {
        let curl = Curl::program_at("/nonexistent/curl".into());
        assert!(!curl.available());
        let error = curl.post(&key().unwrap(), "{}", DEADLINE).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(Curl::PACKAGE, "curl");
    }

    #[test]
    fn curl_that_outlives_the_deadline_is_killed() {
        let directory = tempfile::tempdir().unwrap();
        let curl = Curl::program_at(script(directory.path(), "exec sleep 30"));
        let started = Instant::now();
        let error = curl
            .post(&key().unwrap(), "{}", Duration::from_millis(100))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the child was not killed"
        );
    }

    #[test]
    fn curl_ignores_its_rc_file_and_never_sees_the_key_in_arguments() {
        let directory = tempfile::tempdir().unwrap();
        let arguments = directory.path().join("arguments");
        let program = script(
            directory.path(),
            &format!(
                "printf '%s\\n' \"$@\" > '{}'\ncat > /dev/null\necho '{{}}'",
                arguments.display()
            ),
        );
        let reply = Curl::program_at(program)
            .post(&key().unwrap(), "{}", DEADLINE)
            .unwrap();
        assert_eq!(reply.trim(), "{}");
        let seen = std::fs::read_to_string(arguments).unwrap();
        assert_eq!(
            seen.lines().next(),
            Some("-q"),
            "-q must come first so ~/.curlrc is ignored"
        );
        assert!(!seen.contains("secret-key-123"));
    }

    #[test]
    fn a_key_that_could_break_the_config_is_refused() {
        for bad in ["a\"b", "a\\b", "a b", "", "a\nb"] {
            assert!(ApiKey::new(bad.to_owned()).is_none(), "{bad:?}");
        }
        assert!(ApiKey::new("sk-abc_123".into()).is_some());
    }
}
