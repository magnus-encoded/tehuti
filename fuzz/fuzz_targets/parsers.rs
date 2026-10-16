#![no_main]
use ibis_commands::{
    Button, Environment, Input, State, parse_words, read_value, specs, spelled, spoken_words, step,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    let _parsed = parse_words(text);
    let _spelled = spelled(text);
    let _words = spoken_words(text);
    if let Ok(specs) = specs() {
        for spec in &specs {
            for definition in &spec.moves {
                let _candidates = read_value(definition, Some(text), &Environment::default());
            }
        }
        let (state, _) = step(State::new("/tmp".into(), specs), Input::Press(Button::New));
        let (state, _) = step(state, Input::Say(text.into()));
        assert!(state.valid());
    }
    if let Ok(state) = serde_json::from_str::<State>(text) {
        let _valid = state.valid();
    }
});
