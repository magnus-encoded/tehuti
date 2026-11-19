//! Pad lifecycle: keys survive reconnects and a drop's effects come exactly once.

use std::path::PathBuf;
use tehuti_core::pad::{Identity, Origin, Pad, PadBook, PadEffect, PadKey, PadRoster, PadSighting, Presence};
use tehuti_protocol::ButtonName;

fn sighting(path: &str, uniq: &str) -> PadSighting {
    PadSighting {
        path: PathBuf::from(path),
        name: "Twin Pad".into(),
        vendor: 1,
        product: 2,
        uniq: uniq.into(),
        origin: Origin::Usb,
    }
}

fn keys(roster: &mut PadRoster, sightings: Vec<PadSighting>) -> Vec<String> {
    roster
        .observe(sightings)
        .into_iter()
        .map(|(_, pad)| pad.key.as_str().to_owned())
        .collect()
}

fn pad() -> Pad {
    Pad {
        key: PadKey::new("k"),
        label: "PAD #1".into(),
        model: Identity::default(),
        origin: Origin::Usb,
        presence: Presence::Connecting,
        held: Vec::new(),
    }
}

#[test]
fn reconnect_keeps_the_key() {
    let mut roster = PadRoster::default();
    let first = keys(&mut roster, vec![sighting("/dev/input/event5", "")]);
    let _none = keys(&mut roster, vec![]);
    let again = keys(&mut roster, vec![sighting("/dev/input/event9", "")]);
    assert_eq!(first, again);
}

#[test]
fn unplugging_the_first_of_two_identical_pads_does_not_swap_keys() {
    let mut roster = PadRoster::default();
    let both = keys(
        &mut roster,
        vec![
            sighting("/dev/input/event5", ""),
            sighting("/dev/input/event6", ""),
        ],
    );
    let second_alone = keys(&mut roster, vec![sighting("/dev/input/event6", "")]);
    assert_eq!(second_alone[0], both[1]);
    let replugged = keys(
        &mut roster,
        vec![
            sighting("/dev/input/event6", ""),
            sighting("/dev/input/event7", ""),
        ],
    );
    assert_eq!(replugged, vec![both[1].clone(), both[0].clone()]);
}

#[test]
fn grab_failure_keeps_the_pad_connecting_until_a_grab_works() {
    let mut pad = pad();
    assert!(pad.on_seen().is_empty());
    assert_eq!(pad.presence, Presence::Connecting);
    assert!(pad.on_seen().is_empty());
    assert_eq!(pad.presence, Presence::Connecting);
    assert!(pad.on_grabbed().is_empty());
    assert_eq!(pad.presence, Presence::Present);
}

#[test]
fn a_pad_gone_while_connecting_only_forgets_the_gate_and_can_reappear() {
    let mut pad = pad();
    assert_eq!(pad.on_gone(), vec![PadEffect::ForgetGate]);
    assert_eq!(pad.presence, Presence::Gone);
    let _quiet = pad.on_seen();
    assert_eq!(pad.presence, Presence::Connecting);
    let _quiet = pad.on_grabbed();
    assert_eq!(pad.presence, Presence::Present);
}

#[test]
fn a_drop_releases_held_buttons_exactly_once() {
    let mut pad = pad();
    let _quiet = pad.on_grabbed();
    pad.on_button(ButtonName::South, true);
    pad.on_button(ButtonName::East, true);
    pad.on_button(ButtonName::East, false);
    let effects = pad.on_gone();
    assert_eq!(
        effects,
        vec![
            PadEffect::ReleaseHeld(vec![ButtonName::South]),
            PadEffect::ForgetGate
        ]
    );
    assert!(pad.on_gone().is_empty());
}

#[test]
fn the_book_forgets_the_gate_only_for_pads_that_vanish() {
    let mut roster = PadRoster::default();
    let mut book = PadBook::default();
    let both = vec![
        sighting("/dev/input/event5", ""),
        sighting("/dev/input/event6", ""),
    ];
    assert!(book.sync(roster.observe(both)).is_empty());
    let effects = book.sync(roster.observe(vec![sighting("/dev/input/event6", "")]));
    assert_eq!(
        effects,
        vec![(PathBuf::from("/dev/input/event5"), PadEffect::ForgetGate)]
    );
    assert!(book.get(std::path::Path::new("/dev/input/event5")).is_none());
    assert_eq!(
        book.get(std::path::Path::new("/dev/input/event6"))
            .map(|pad| pad.presence),
        Some(Presence::Present)
    );
    assert!(
        book.sync(roster.observe(vec![sighting("/dev/input/event6", "")]))
            .is_empty()
    );
}
