//! The speech resolver adapter sends only with the opt-in, and only transcript and command names.

#[cfg(test)]
mod tests {
    use core::{cell::RefCell, time::Duration};
    use std::io;

    use tehuti::{
        display_mode_store::Store,
        speech::resolver::{ApiKey, DEADLINE, OptIn, Resolver, Transport, TurnLog, request_body},
    };
    use tehuti_core::speech::resolver::{Answer, Resolution, Turn};

    #[derive(Default)]
    struct Fake {
        sent: RefCell<Vec<String>>,
        reply: Option<String>,
    }

    impl Transport for &Fake {
        fn post(&self, _key: &ApiKey, body: &str, deadline: Duration) -> io::Result<String> {
            assert_eq!(deadline, DEADLINE, "the call carries the deadline");
            self.sent.borrow_mut().push(body.to_owned());
            self.reply
                .clone()
                .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "late"))
        }
    }

    const REPLY: &str = r#"{"model":"jev-1","answers":{"command":{"type":"choice","choice":"mute","confidence":0.9,"probabilities":{"mute":0.95,"pause":0.05}}}}"#;

    fn commands() -> Vec<String> {
        vec!["mute".into(), "pause".into()]
    }

    fn key() -> Option<ApiKey> {
        ApiKey::new("secret-key-123".into())
    }

    #[test]
    fn nothing_is_sent_without_the_opt_in_even_with_a_key() {
        let fake = Fake {
            reply: Some(REPLY.into()),
            ..Fake::default()
        };
        let resolver = Resolver::new(&fake, key());
        assert_eq!(resolver.ask(false, "quiet", &commands()), None);
        assert!(fake.sent.borrow().is_empty(), "no request left the process");
    }

    #[test]
    fn nothing_is_sent_without_a_key() {
        let fake = Fake {
            reply: Some(REPLY.into()),
            ..Fake::default()
        };
        assert_eq!(Resolver::new(&fake, None).ask(true, "quiet", &commands()), None);
        assert!(fake.sent.borrow().is_empty(), "no key, no request");
    }

    #[test]
    fn an_opted_in_ask_returns_the_answer_in_thousandths() {
        let fake = Fake {
            reply: Some(REPLY.into()),
            ..Fake::default()
        };
        let answer = Resolver::new(&fake, key()).ask(true, "quiet", &commands());
        let answer = answer.expect("answered");
        assert_eq!((answer.command.as_str(), answer.confidence), ("mute", 900));
        assert_eq!(answer.ranked.len(), 2);
    }

    #[test]
    fn a_late_or_failed_reply_and_garbage_fall_back() {
        let late = Fake::default();
        assert_eq!(Resolver::new(&late, key()).ask(true, "quiet", &commands()), None);
        let garbage = Fake {
            reply: Some("not json".into()),
            ..Fake::default()
        };
        assert_eq!(
            Resolver::new(&garbage, key()).ask(true, "quiet", &commands()),
            None
        );
        assert_eq!(late.sent.borrow().len(), 1, "the late call was attempted once");
    }

    #[test]
    fn the_request_holds_only_the_transcript_and_command_names() {
        let body = request_body("quiet", &commands());
        let text = body.to_string();
        assert!(text.contains("quiet") && text.contains("mute") && text.contains("pause"));
        assert_eq!(body["state"], serde_json::json!({"transcript": "quiet"}));
        let keys: Vec<_> = body["questions"]["command"]["criteria"]
            .as_object()
            .expect("criteria")
            .keys()
            .cloned()
            .collect();
        assert!(keys.contains(&"mute".to_owned()) && keys.contains(&"pause".to_owned()));
    }

    #[test]
    fn the_key_never_prints() {
        assert!(!format!("{:?}", key()).contains("secret"));
    }

    #[test]
    fn the_opt_in_is_off_until_set_and_survives_reopening() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("tehuti/speech-resolver");
        assert!(!OptIn::at(Store::new(path.clone())).enabled());
        OptIn::at(Store::new(path.clone())).set(true)?;
        assert!(OptIn::at(Store::new(path.clone())).enabled());
        OptIn::at(Store::new(path.clone())).set(false)?;
        assert!(!OptIn::at(Store::new(path)).enabled());
        Ok(())
    }

    #[test]
    fn each_turn_is_one_log_line_with_what_was_heard_chosen_and_run() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let log = TurnLog::at(directory.path().join("state/turns.jsonl"));
        let answer = Answer {
            command: "mute".into(),
            confidence: 900,
            ranked: Vec::new(),
        };
        let turn = Turn::new("quiet", Some(&answer), &Resolution::Run("mute".into()));
        log.append(&turn, false)?;
        log.append(&Turn::new("hello", None, &Resolution::Dictate), true)?;
        let text = std::fs::read_to_string(directory.path().join("state/turns.jsonl"))?;
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("json"))
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["heard"], "quiet");
        assert_eq!(lines[0]["chosen"]["confidence"], 900);
        assert_eq!(lines[0]["outcome"], "ran");
        assert_eq!(lines[0]["cancelled"], false);
        assert_eq!(lines[1]["outcome"], "dictated");
        assert_eq!(lines[1]["cancelled"], true);
        Ok(())
    }
}
