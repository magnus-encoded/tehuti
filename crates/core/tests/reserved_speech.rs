#[cfg(test)]
mod reserved_speech {
    use tehuti_protocol::{Event, Text};

    #[test]
    fn reserved_commands_do_not_emit_context_fallback_actions() {
        for phrase in [
            "new",
            "open notes",
            "åpne notes",
            "work notes",
            "work on notes",
            "Start recording.",
            "STOP RECORDING!",
        ] {
            let (_, actions) = tehuti_core::step(
                tehuti_core::State::default(),
                &Event::Speech {
                    text: Text::new(phrase.into()).unwrap(),
                },
            );
            assert!(
                actions.is_empty(),
                "reserved command must bypass context fallback: {phrase}: {actions:?}"
            );
        }
    }
}
