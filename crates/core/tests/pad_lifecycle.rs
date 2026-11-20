//! Pad lifecycle through the roster: keys survive reconnects and a drop's effects come once.

use std::path::{Path, PathBuf};
use tehuti_core::pad::{Origin, PadEffect, PadRoster, PadSighting};
use tehuti_protocol::ButtonName;

#[cfg(test)]
mod tests {
    use super::*;

    fn see(roster: &mut PadRoster, sightings: Vec<PadSighting>) {
        let _pads = roster.observe(sightings);
    }

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

    #[test]
    fn reconnect_keeps_the_key() {
        let mut roster = PadRoster::default();
        let first = keys(&mut roster, vec![sighting("/dev/input/event5", "")]);
        see(&mut roster, vec![]);
        let again = keys(&mut roster, vec![sighting("/dev/input/event9", "")]);
        assert_eq!(first, again);
    }

    #[test]
    fn a_path_that_vanishes_and_reappears_keeps_its_key() {
        let mut roster = PadRoster::default();
        let first = keys(&mut roster, vec![sighting("/dev/input/event5", "")]);
        see(&mut roster, vec![]);
        let again = keys(&mut roster, vec![sighting("/dev/input/event5", "")]);
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
    fn a_held_button_on_a_vanished_pad_is_released_exactly_once() {
        let mut roster = PadRoster::default();
        let first = PathBuf::from("/dev/input/event5");
        see(
            &mut roster,
            vec![
                sighting("/dev/input/event5", ""),
                sighting("/dev/input/event6", ""),
            ],
        );
        roster.on_button(&first, ButtonName::South, true);
        roster.on_button(&first, ButtonName::East, true);
        roster.on_button(&first, ButtonName::East, false);
        roster.on_button(Path::new("/dev/input/event6"), ButtonName::West, true);
        see(&mut roster, vec![sighting("/dev/input/event6", "")]);
        assert_eq!(
            roster.take_effects(),
            vec![
                (first.clone(), PadEffect::ReleaseHeld(vec![ButtonName::South])),
                (first, PadEffect::ForgetGate),
            ]
        );
        see(&mut roster, vec![sighting("/dev/input/event6", "")]);
        assert_eq!(roster.take_effects(), Vec::new());
    }

    #[test]
    fn a_vanished_pad_with_nothing_held_only_forgets_the_gate() {
        let mut roster = PadRoster::default();
        see(&mut roster, vec![sighting("/dev/input/event5", "")]);
        see(&mut roster, vec![]);
        assert_eq!(
            roster.take_effects(),
            vec![(PathBuf::from("/dev/input/event5"), PadEffect::ForgetGate)]
        );
    }

    #[test]
    fn held_buttons_survive_a_rescan_that_keeps_the_pad() {
        let mut roster = PadRoster::default();
        let path = PathBuf::from("/dev/input/event5");
        see(&mut roster, vec![sighting("/dev/input/event5", "")]);
        roster.on_button(&path, ButtonName::South, true);
        see(&mut roster, vec![sighting("/dev/input/event5", "")]);
        see(&mut roster, vec![]);
        assert_eq!(
            roster.take_effects(),
            vec![
                (path.clone(), PadEffect::ReleaseHeld(vec![ButtonName::South])),
                (path, PadEffect::ForgetGate),
            ]
        );
    }
}
