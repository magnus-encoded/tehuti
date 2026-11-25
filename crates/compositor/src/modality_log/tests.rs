use super::report::{build, parse, render};
use super::*;
use proptest::prelude::*;

const KEY_A: u32 = 30;
const KEY_S: u32 = 31;

fn stamp(millis: u64) -> Stamp {
    Stamp {
        elapsed: Duration::from_millis(millis),
        unix_ms: millis,
    }
}

fn records(lines: &[String]) -> Vec<Record> {
    lines
        .iter()
        .map(|line| serde_json::from_str::<Line>(line).map(|parsed| parsed.record))
        .collect::<Result<_, _>>()
        .unwrap()
}

#[derive(Clone, Debug)]
enum Notice {
    Key { code: u32, pressed: bool },
    Speech(SpeechOutcome),
    Press(Modality),
    Wait(u64),
}

fn notice() -> impl Strategy<Value = Notice> {
    let outcome = prop_oneof![
        Just(SpeechOutcome::Matched),
        Just(SpeechOutcome::Unmatched),
        Just(SpeechOutcome::Dictated)
    ];
    let modality = prop_oneof![Just(Modality::Pad), Just(Modality::Pointer), Just(Modality::Osk)];
    prop_oneof![
        4 => (prop_oneof![1_u32..130, Just(29_u32), Just(125_u32)], any::<bool>())
            .prop_map(|(code, pressed)| Notice::Key { code, pressed }),
        2 => outcome.prop_map(Notice::Speech),
        1 => modality.prop_map(Notice::Press),
        2 => (0_u64..15_000).prop_map(Notice::Wait),
    ]
}

fn run(notices: &[Notice], app_id: Option<&str>) -> Vec<String> {
    let mut now = 0_u64;
    let mut recorder = Recorder::start(Vec::new(), stamp(now));
    for notice in notices {
        match notice {
            Notice::Key { code, pressed } => recorder.key(*code, *pressed, app_id, stamp(now)),
            Notice::Speech(outcome) => recorder.speech(*outcome, app_id, stamp(now)),
            Notice::Press(modality) => recorder.press(*modality, app_id, stamp(now)),
            Notice::Wait(millis) => now = now.saturating_add(*millis),
        }
    }
    recorder.end(stamp(now))
}

#[test]
fn a_modifier_alone_is_no_action_and_a_key_under_it_is_a_shortcut() {
    let lines = run(
        &[
            Notice::Key { code: 29, pressed: true },
            Notice::Key { code: KEY_S, pressed: true },
            Notice::Key { code: KEY_S, pressed: false },
            Notice::Key { code: 29, pressed: false },
            Notice::Key { code: KEY_A, pressed: true },
        ],
        Some("editor"),
    );
    let classes: Vec<_> = records(&lines)
        .into_iter()
        .filter_map(|record| match record {
            Record::Input { key_class, .. } => key_class,
            Record::SessionStart
            | Record::SessionEnd
            | Record::Speech { .. }
            | Record::Fallback { .. }
            | Record::SurfaceOpened { .. } => None,
        })
        .collect();
    assert_eq!(classes, [KeyClass::Shortcut, KeyClass::TextEntry], "{lines:?}");
}

#[test]
fn a_key_soon_after_an_unmatched_transcript_is_a_fallback() {
    let lines = run(
        &[
            Notice::Speech(SpeechOutcome::Unmatched),
            Notice::Wait(3_000),
            Notice::Key { code: KEY_A, pressed: true },
        ],
        Some("editor"),
    );
    assert!(
        records(&lines).contains(&Record::Fallback {
            app_id: Some("editor".into()),
            key_class: KeyClass::TextEntry,
            after_ms: 3_000,
        }),
        "{lines:?}"
    );
}

#[test]
fn a_session_is_a_span_from_start_to_end() {
    let lines = records(&run(&[Notice::Wait(5)], None));
    assert_eq!(lines, [Record::SessionStart, Record::SessionEnd], "span");
}

proptest! {
    #[test]
    fn a_modifier_or_shift_alone_never_classifies(held in any::<bool>()) {
        for code in [29_u32, 97, 56, 100, 125, 126, 42, 54, 58] {
            prop_assert_eq!(classify(code, held), None, "code {}", code);
        }
    }

    #[test]
    fn every_other_key_under_a_command_modifier_is_a_shortcut(code in 0_u32..256) {
        prop_assume!(classify(code, false).is_some());
        prop_assert_eq!(classify(code, true), Some(KeyClass::Shortcut), "code {}", code);
    }

    #[test]
    fn fallbacks_follow_an_unmatched_transcript_within_the_window_once(
        notices in proptest::collection::vec(notice(), 0..60)
    ) {
        let mut now = 0_u64;
        let mut unmatched_at: Option<u64> = None;
        let mut recorder = Recorder::start(Vec::new(), stamp(0));
        for notice in &notices {
            let before = recorder.sink.len();
            match notice {
                Notice::Key { code, pressed } => recorder.key(*code, *pressed, None, stamp(now)),
                Notice::Speech(outcome) => {
                    recorder.speech(*outcome, None, stamp(now));
                    unmatched_at = (*outcome == SpeechOutcome::Unmatched).then_some(now);
                }
                Notice::Press(modality) => recorder.press(*modality, None, stamp(now)),
                Notice::Wait(millis) => now = now.saturating_add(*millis),
            }
            let written = records(recorder.sink.get(before..).unwrap_or_default());
            let keyboard = written.iter().any(|record| matches!(
                record,
                Record::Input { modality: Modality::Keyboard, .. }
            ));
            let fallbacks: Vec<u64> = written.iter().filter_map(|record| match record {
                Record::Fallback { after_ms, .. } => Some(*after_ms),
                Record::SessionStart
                | Record::SessionEnd
                | Record::Input { .. }
                | Record::Speech { .. }
                | Record::SurfaceOpened { .. } => None,
            }).collect();
            if keyboard {
                let expected = unmatched_at
                    .take()
                    .map(|at| now.saturating_sub(at))
                    .filter(|after| *after <= 10_000);
                prop_assert_eq!(fallbacks.first().copied(), expected, "{:?}", notices);
            }
            prop_assert!(fallbacks.len() <= 1, "{:?}", fallbacks);
        }
    }

    #[test]
    fn the_report_counts_every_keyboard_press_once_and_bursts_never_exceed_presses(
        notices in proptest::collection::vec(notice(), 0..80),
        app in prop_oneof![Just(None), Just(Some("editor")), Just(Some("viewer"))],
    ) {
        let lines = run(&notices, app);
        let presses = records(&lines).iter().filter(|record| matches!(
            record,
            Record::Input { modality: Modality::Keyboard, .. }
        )).count();
        let (parsed, skipped) = parse(&lines.join("\n"));
        let report = build(&parsed, skipped);
        let counted: u64 = report.situations.iter().map(|situation| situation.presses).sum();
        prop_assert_eq!(counted, u64::try_from(presses).unwrap(), "presses");
        prop_assert_eq!(skipped, 0, "skipped");
        for situation in &report.situations {
            prop_assert!(situation.occurrences >= 1 && situation.occurrences <= situation.presses, "{:?}", situation);
        }
        for pair in report.situations.windows(2) {
            prop_assert!(pair[0].occurrences >= pair[1].occurrences, "ranked {:?}", report.situations);
        }
    }

    #[test]
    fn a_line_reads_back_as_the_record_it_was_written_from(
        notices in proptest::collection::vec(notice(), 0..30)
    ) {
        let lines = run(&notices, Some("editor"));
        let (parsed, skipped) = parse(&lines.join("\n"));
        prop_assert_eq!(skipped, 0, "skipped");
        prop_assert_eq!(parsed.len(), lines.len(), "count");
    }
}

#[test]
fn presses_far_apart_are_separate_occurrences_and_close_ones_one() {
    let lines = run(
        &[
            Notice::Key { code: KEY_A, pressed: true },
            Notice::Wait(500),
            Notice::Key { code: KEY_S, pressed: true },
            Notice::Wait(60_000),
            Notice::Key { code: KEY_A, pressed: true },
        ],
        Some("editor"),
    );
    let (parsed, skipped) = parse(&lines.join("\n"));
    let report = build(&parsed, skipped);
    let situation = report.situations.first().unwrap();
    assert_eq!((situation.occurrences, situation.presses), (2, 3), "{report:?}");
}

#[test]
fn a_cut_off_line_is_skipped_not_fatal() {
    let (parsed, skipped) = parse("{\"time_unix_ms\":1,\"name\":\"session_start\"}\n{\"time_unix_ms\":2,\"na");
    assert_eq!((parsed.len(), skipped), (1, 1), "parsed");
}

#[test]
fn the_rendered_report_ranks_the_most_used_situation_first() {
    let mut notices = Vec::new();
    for _burst in 0..3 {
        notices.push(Notice::Key { code: 103, pressed: true });
        notices.push(Notice::Wait(5_000));
    }
    notices.push(Notice::Key { code: KEY_A, pressed: true });
    let lines = run(&notices, Some("viewer"));
    let (parsed, skipped) = parse(&lines.join("\n"));
    let text = render(&build(&parsed, skipped), 10);
    let navigation = text.find("navigation").unwrap();
    let typing = text.find("text entry").unwrap();
    assert!(navigation < typing, "{text}");
    assert!(text.contains("Inputs: keyboard 4"), "{text}");
}

#[test]
fn the_file_moves_aside_at_the_cap_so_both_files_stay_under_it() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state/modality.jsonl");
    let mut sink = FileSink::open(path.clone(), 200).unwrap();
    for _line in 0..100 {
        sink.append("{\"time_unix_ms\":0,\"name\":\"session_start\"}").unwrap();
    }
    for file in [path.clone(), FileSink::rotated(&path)] {
        let size = std::fs::metadata(&file).unwrap().len();
        assert!(size > 0 && size <= 200, "{} is {size} bytes", file.display());
    }
    let (parsed, skipped) = parse(&report::read_log(&path).unwrap());
    assert!(!parsed.is_empty() && skipped == 0, "read back");
}

#[test]
fn the_switch_file_turns_the_log_off_and_absence_leaves_it_on() {
    let root = tempfile::tempdir().unwrap();
    let switch = crate::display_mode_store::Store::new(root.path().join("modality-log"));
    assert!(enabled(&switch), "absent");
    std::fs::write(root.path().join("modality-log"), "off\n").unwrap();
    assert!(!enabled(&switch), "off");
    std::fs::write(root.path().join("modality-log"), "on\n").unwrap();
    assert!(enabled(&switch), "on");
}
