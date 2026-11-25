#[cfg(test)]
mod modality_log {
    use smithay::{input::keyboard::Keycode, reexports::wayland_server::Display};
    use tehuti::{
        modality_log::{FileSink, KeyClass, Line, Modality, Record, Recorder, SpeechOutcome, Stamp, report},
        state::Compositor,
    };
    use tehuti_protocol::{ButtonName, ButtonState, Event, Text};

    const KEY_LEFTCTRL: u32 = 29;
    const KEY_S: u32 = 31;
    const KEY_Q: u32 = 16;
    const SPOKEN: &str = "zebra quartz umbrella";

    fn tap(compositor: &mut Compositor, code: u32) {
        for pressed in [true, false] {
            compositor.host_key(Keycode::new(code.saturating_add(8)), pressed);
        }
    }

    fn logged(
        compositor: &mut Compositor,
        path: &std::path::Path,
    ) -> Result<Vec<Record>, Box<dyn core::error::Error>> {
        compositor.set_modality_log(Recorder::start(
            FileSink::open(path.to_path_buf(), FileSink::DEFAULT_CAP_BYTES)?,
            Stamp {
                elapsed: compositor.start_time.elapsed(),
                unix_ms: 1,
            },
        ));
        compositor.handle(&Event::Focus {
            app_id: Some("editor".into()),
            title: String::new(),
        });
        compositor.host_key(Keycode::new(KEY_LEFTCTRL + 8), true);
        tap(compositor, KEY_S);
        compositor.host_key(Keycode::new(KEY_LEFTCTRL + 8), false);
        compositor.controller_input(tehuti_core::controller::Input::Event(Event::Button {
            name: ButtonName::South,
            state: ButtonState::Pressed,
        }));
        compositor.speech_input(tehuti_core::speech::SpeechInput::Transcript {
            session: 1,
            text: Text::new(SPOKEN.into())?,
        });
        tap(compositor, KEY_Q);
        drop(compositor.end_modality_log());
        let text = std::fs::read_to_string(path)?;
        assert!(
            !text.contains("zebra") && !text.contains("quartz"),
            "transcript text leaked: {text}"
        );
        Ok(text
            .lines()
            .map(serde_json::from_str::<Line>)
            .map(|line| line.map(|parsed| parsed.record))
            .collect::<Result<_, _>>()?)
    }

    #[test]
    fn a_session_logs_modality_key_class_and_fallback_without_text() -> Result<(), Box<dyn core::error::Error>>
    {
        let root = tempfile::tempdir()?;
        let path = root.path().join("modality.jsonl");
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        let records = logged(&mut compositor, &path)?;
        let editor = Some("editor".to_owned());
        for expected in [
            Record::SessionStart,
            Record::Input {
                modality: Modality::Keyboard,
                app_id: editor.clone(),
                key_class: Some(KeyClass::Shortcut),
            },
            Record::Input {
                modality: Modality::Pad,
                app_id: editor.clone(),
                key_class: None,
            },
            Record::Speech {
                outcome: SpeechOutcome::Unmatched,
                app_id: editor.clone(),
            },
            Record::Input {
                modality: Modality::Keyboard,
                app_id: editor.clone(),
                key_class: Some(KeyClass::TextEntry),
            },
            Record::SessionEnd,
        ] {
            assert!(records.contains(&expected), "missing {expected:?} in {records:?}");
        }
        assert!(
            records.iter().any(|record| matches!(
                record,
                Record::Fallback { app_id, key_class: KeyClass::TextEntry, .. } if *app_id == editor
            )),
            "{records:?}"
        );
        let (lines, skipped) = report::parse(&report::read_log(&path)?);
        let text = report::render(&report::build(&lines, skipped), 5);
        assert!(text.contains("editor") && text.contains("shortcut"), "{text}");
        Ok(())
    }

    #[test]
    fn without_a_log_nothing_is_written() -> Result<(), Box<dyn core::error::Error>> {
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        tap(&mut compositor, KEY_Q);
        assert!(compositor.end_modality_log().is_none(), "no log was started");
        Ok(())
    }
}
