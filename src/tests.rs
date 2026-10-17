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
    for effect in effects {
        let Effect::Run(invocation) = effect else {
            continue;
        };
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
            folders: vec!["bad\0folder".into(), "src".into()],
        }),
    );
    assert_eq!(
        state.env.programs,
        vec!["valid"],
        "only valid executable names enter the reader pool"
    );
    assert_eq!(state.env.names, vec!["src"], "paths cannot contain NUL");
    assert_eq!(state.env.folders, vec!["src"], "folders cannot contain NUL");
    Ok(())
}

#[test]
fn stage_navigation_cleans_empty_stages_and_undo_restores_them() -> Result<(), serde_json::Error> {
    let (mut state, _) = step(State::new("/tmp".into(), specs()?), Input::Press(Button::New));
    (state, _) = step(state, Input::Press(Button::Down));
    assert_eq!(
        state.screen().list.len(),
        10,
        "browse offers all bundled programs"
    );
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

#[test]
fn spoken_raw_argument_keeps_its_value() -> Result<(), serde_json::Error> {
    let (mut state, _) = step(State::new("/tmp".into(), specs()?), Input::Press(Button::New));
    (state, _) = step(state, Input::Type("unknownprog".into()));
    (state, _) = step(state, Input::Say("a".into()));
    assert_eq!(
        state.screen().list.first().map(|entry| entry.value.as_str()),
        Some("a"),
        "the raw candidate is the heard text"
    );
    (state, _) = step(state, Input::Press(Button::Accept));
    assert_eq!(
        state.draft.preview(),
        "unknownprog a",
        "one accept fills the raw argument"
    );
    assert!(state.draft.ready().is_some(), "no empty argument hole remains");
    Ok(())
}

proptest::proptest! {
    #[test]
    fn raw_candidates_carry_the_spoken_text(word in "[a-z]{1,8}") {
        let (mut state, _) = step(State::new("/tmp".into(), specs().map_err(|error| proptest::test_runner::TestCaseError::fail(error.to_string()))?), Input::Press(Button::New));
        (state, _) = step(state, Input::Type("unknownprog".into()));
        (state, _) = step(state, Input::Say(word.clone()));
        let screen = state.screen();
        proptest::prop_assert!(screen.list.iter().any(|entry| entry.value == word), "raw text offered");
        proptest::prop_assert!(screen.list.iter().all(|entry| entry.value != "argument"), "no empty raw hole offered");
    }
}

#[test]
fn cd_is_a_builtin_that_changes_the_working_folder() -> Result<(), serde_json::Error> {
    use crate::Environment;
    let (mut state, _) = step(State::new("/home/me".into(), specs()?), Input::Press(Button::New));
    (state, _) = step(
        state,
        Input::Environment(Environment {
            programs: vec!["ls".into()],
            names: vec!["notes.txt".into(), "src".into()],
            folders: vec!["src".into()],
        }),
    );
    (state, _) = step(state, Input::Say("cd".into()));
    assert!(
        state.screen().list.iter().any(|entry| entry.value == "cd"),
        "cd is a program candidate though it is not on PATH"
    );
    (state, _) = step(state, Input::Press(Button::Accept));
    (state, _) = step(state, Input::Press(Button::Down));
    let offered: Vec<_> = state
        .screen()
        .list
        .iter()
        .map(|entry| entry.value.clone())
        .collect();
    assert!(offered.contains(&"src".to_owned()), "folders are offered");
    assert!(
        !offered.contains(&"notes.txt".to_owned()),
        "files are not offered"
    );
    (state, _) = step(state, Input::Press(Button::Back));
    (state, _) = step(state, Input::Say("source".into()));
    (state, _) = step(state, Input::Type("src".into()));
    let effects;
    (state, effects) = step(state, Input::Press(Button::Accept));
    assert_eq!(
        effects,
        vec![Effect::ChangeFolder("/home/me/src".into())],
        "cd changes the folder instead of spawning"
    );
    assert_eq!(
        state.cwd(),
        "/home/me/src",
        "the builder's folder segment follows"
    );
    Ok(())
}

#[test]
fn join_folder_resolves_lexically() {
    use crate::join_folder;
    assert_eq!(join_folder("/a/b", ".."), "/a", "parent");
    assert_eq!(join_folder("/a/b", "/etc/./x/.."), "/etc", "absolute");
    assert_eq!(join_folder("/", "../.."), "/", "root stays root");
    assert_eq!(join_folder("/a", "b/c/"), "/a/b/c", "relative");
}

#[test]
fn ip_spec_offers_objects_and_family_flags() -> Result<(), serde_json::Error> {
    let (mut state, _) = step(State::new("/tmp".into(), specs()?), Input::Press(Button::New));
    (state, _) = step(state, Input::Type("ip".into()));
    (state, _) = step(state, Input::Say("a".into()));
    (state, _) = step(state, Input::Press(Button::Accept));
    assert_eq!(state.draft.preview(), "ip a", "saying a gives ip a");
    (state, _) = step(state, Input::Press(Button::New));
    (state, _) = step(state, Input::Type("ip -br -4 route".into()));
    assert_eq!(
        state.draft.preview(),
        "ip -br -4 route",
        "typed ip parses into spec moves"
    );
    let ip_spec = specs()?.into_iter().find(|spec| spec.program == "ip");
    let keys: Vec<_> = ip_spec
        .iter()
        .flat_map(|spec| spec.moves.iter().map(|entry| entry.key.as_str()))
        .collect();
    for key in ["addr", "a", "link", "route", "r", "neigh", "-br", "-4", "-6"] {
        assert!(keys.contains(&key), "ip offers {key}");
    }
    Ok(())
}
