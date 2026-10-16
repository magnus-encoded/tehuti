use crate::{Button, Effect, Input, State, specs, step};

#[test]
fn running_preserves_the_edit_undo_stack() -> Result<(), serde_json::Error> {
    let (state, _) = step(State::new("/tmp".into(), specs()?), Input::Press(Button::New));
    let (state, _) = step(state, Input::Type("cat".into()));
    let previous = state.undo.clone();
    let (state, effects) = step(state, Input::Press(Button::Accept));
    assert!(!effects.is_empty(), "accept runs a ready draft");
    assert_eq!(state.undo, previous, "running is not an edit");
    Ok(())
}

#[test]
fn speech_cannot_edit_history_or_accept_nul() -> Result<(), serde_json::Error> {
    let (mut state, _) = step(State::new("/tmp".into(), specs()?), Input::Press(Button::New));
    (state, _) = step(state, Input::Say("\0".into()));
    assert!(state.list.is_none(), "NUL transcripts never open a list");
    (state, _) = step(state, Input::Type("cat".into()));
    let effects;
    (state, effects) = step(state, Input::Press(Button::Accept));
    for Effect::Run(invocation) in effects {
        (state, _) = step(
            state,
            Input::Completed {
                invocation,
                exit: 0,
                output: String::new(),
            },
        );
    }
    (state, _) = step(state, Input::Press(Button::New));
    (state, _) = step(state, Input::Press(Button::LsUp));
    (state, _) = step(state, Input::Say("find".into()));
    assert!(state.list.is_none(), "history focus never receives speech");
    assert!(
        state.draft.ready().is_none(),
        "history speech leaves the builder alone"
    );
    Ok(())
}

#[test]
fn undo_retains_exactly_fifty_edits() -> Result<(), serde_json::Error> {
    let (mut state, _) = step(State::new("/tmp".into(), specs()?), Input::Press(Button::New));
    for index in 1_usize..=55 {
        (state, _) = step(state, Input::Type(format!("program{index}")));
        assert_eq!(
            state.undo.len(),
            index.saturating_add(1).min(50),
            "undo retains fifty snapshots"
        );
        (state, _) = step(state, Input::Press(Button::Left));
    }
    Ok(())
}

#[test]
fn non_edits_do_not_add_undo_snapshots() -> Result<(), serde_json::Error> {
    let (mut state, _) = step(State::new("/tmp".into(), specs()?), Input::Press(Button::New));
    for input in [
        Input::Say("find".into()),
        Input::Press(Button::Down),
        Input::Press(Button::Back),
        Input::Press(Button::Left),
        Input::Press(Button::Right),
    ] {
        let before = state.undo.clone();
        (state, _) = step(state, input);
        assert_eq!(state.undo, before, "lists and navigation are not edits");
    }
    (state, _) = step(state, Input::Type("cat".into()));
    let mut expected = state.undo.clone();
    let _snapshot = expected.pop();
    (state, _) = step(state, Input::Press(Button::Back));
    assert_eq!(
        state.undo, expected,
        "undo removes one snapshot and never appends one"
    );
    Ok(())
}

#[test]
fn environment_rejects_nul_and_empty_program_names() -> Result<(), serde_json::Error> {
    use crate::Environment;
    let state = State::new("/tmp".into(), specs()?);
    let (state, _) = step(
        state,
        Input::Environment(Environment {
            programs: vec![String::new(), "bad\0name".into(), "valid".into()],
            names: vec!["bad\0path".into(), "src".into()],
        }),
    );
    assert_eq!(
        state.env.programs,
        vec!["valid"],
        "only valid executable names enter the reader pool"
    );
    assert_eq!(state.env.names, vec!["src"], "paths cannot contain NUL");
    Ok(())
}

#[test]
fn stage_navigation_cleans_empty_stages_and_undo_restores_them() -> Result<(), serde_json::Error> {
    let (mut state, _) = step(State::new("/tmp".into(), specs()?), Input::Press(Button::New));
    (state, _) = step(state, Input::Press(Button::Down));
    assert_eq!(state.screen().list.len(), 8, "browse offers all bundled programs");
    assert_eq!(
        state.screen().list.first().map(|entry| entry.value.as_str()),
        Some("find"),
        "program browse follows spec order"
    );
    (state, _) = step(state, Input::Press(Button::Accept));
    (state, _) = step(state, Input::Press(Button::NextStage));
    assert_eq!(state.draft.stages.len(), 2, "RB on the last stage adds a pipe");
    assert!(state.draft.ready().is_none(), "the new stage has a program hole");
    (state, _) = step(state, Input::Press(Button::PreviousStage));
    assert_eq!(state.draft.stages.len(), 1, "leaving an empty stage removes it");
    (state, _) = step(state, Input::Press(Button::Back));
    assert_eq!(state.draft.stages.len(), 2, "cleanup is an undoable edit");
    (state, _) = step(state, Input::Press(Button::Left));
    (state, _) = step(state, Input::Press(Button::Delete));
    assert_eq!(
        state.draft.stages.len(),
        1,
        "deleting a pipe removes its following stage"
    );
    assert!(state.valid(), "cleanup and pipe deletion preserve validity");
    Ok(())
}

#[test]
fn typed_values_obey_their_types_and_spec_group_order() -> Result<(), serde_json::Error> {
    let (mut state, _) = step(State::new("/tmp".into(), specs()?), Input::Press(Button::New));
    for text in ["find", "src", "-L", "-maxdepth", "oops"] {
        (state, _) = step(state, Input::Type(text.into()));
    }
    assert!(
        state.draft.ready().is_none(),
        "an invalid integer cannot fill a hole"
    );
    (state, _) = step(state, Input::Type("3".into()));
    assert_eq!(
        state.draft.preview(),
        "find -L src -maxdepth 3",
        "spec groups determine argv order"
    );
    (state, _) = step(state, Input::Type("-type".into()));
    (state, _) = step(state, Input::Type("invalid".into()));
    assert!(state.draft.ready().is_none(), "enum values come from the spec");
    (state, _) = step(state, Input::Type("f".into()));
    assert!(
        state.draft.ready().is_some(),
        "a valid enum completes the invocation"
    );
    Ok(())
}
