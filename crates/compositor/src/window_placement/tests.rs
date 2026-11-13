use super::Placement;
use proptest::prelude::*;
use tehuti_core::layout::{Command, OutputGeometry, Rect, WindowRules};

fn placement() -> Placement<u8, u8> {
    let mut placement = Placement::new();
    let area = Rect {
        x: 0.0,
        y: 0.0,
        width: 1200.0,
        height: 800.0,
    };
    placement.resize(OutputGeometry { area, usable: area }).unwrap();
    placement
}

proptest! {
    #[test]
    fn surface_sequences_preserve_membership_and_fullscreen(
        events in prop::collection::vec((0_u8..6, 0_u8..5), 1..100)
    ) {
        let mut placement = placement();
        let mut mapped = Vec::new();
        let mut fullscreen = std::collections::BTreeSet::new();
        for (key, event) in events {
            match event {
                0 => {
                    if !mapped.contains(&key) {
                        placement.map(key, key, WindowRules::default()).unwrap();
                        mapped.push(key);
                        prop_assert_eq!(placement.focused(), Some(&key));
                    }
                }
                1 => {
                    let removed = placement.unmap(&key);
                    prop_assert_eq!(removed.is_some(), mapped.contains(&key));
                    mapped.retain(|candidate| *candidate != key);
                    let _removed = fullscreen.remove(&key);
                }
                2 => {
                    placement.fullscreen(&key, true).unwrap();
                    let _inserted = fullscreen.insert(key);
                }
                3 => {
                    placement.fullscreen(&key, false).unwrap();
                    let _removed = fullscreen.remove(&key);
                }
                _ => {
                    placement.focus(&key).unwrap();
                    if mapped.contains(&key) {
                        prop_assert_eq!(placement.focused(), Some(&key));
                    }
                }
            }
            let placed = placement.placed(|_| 0.0);
            let keys: Vec<_> = placed.iter().map(|placed| *placed.key).collect();
            prop_assert_eq!(&keys, &mapped);
            prop_assert_eq!(placement.is_empty(), mapped.is_empty());
            for placed in placed {
                prop_assert_eq!(placed.key, placed.window);
                let (tile, _) = placed.tile.unwrap();
                prop_assert_eq!(tile.fullscreen_requested, fullscreen.contains(placed.key));
                prop_assert!(tile.tile.width.is_finite() && tile.tile.width > 0.0);
                prop_assert!(tile.tile.height.is_finite() && tile.tile.height > 0.0);
            }
            prop_assert!(placement.focused().is_none_or(|key| mapped.contains(key)));
        }
    }
}

#[test]
fn destroying_an_unmapped_surface_discards_its_fullscreen_request() {
    let mut placement = placement();
    placement.fullscreen(&1, true).unwrap();
    assert_eq!(placement.unmap(&1), None);
    placement.map(1, 1, WindowRules::default()).unwrap();
    let placed = placement.placed(|_| 0.0);
    let tile = &placed.first().unwrap().tile.as_ref().unwrap().0;
    assert!(!tile.fullscreen_requested);
}

#[test]
fn fullscreen_before_map_uses_the_output_rect_and_unfullscreen_restores_the_slot() {
    let mut placement = placement();
    placement.map(1, 1, WindowRules::default()).unwrap();
    let before = placement.placed(|_| 0.0).first().unwrap().tile.clone().unwrap().0;
    let _removed = placement.unmap(&1);
    placement.fullscreen(&2, true).unwrap();
    placement.map(2, 2, WindowRules::default()).unwrap();
    let fullscreen = placement.placed(|_| 0.0).first().unwrap().tile.clone().unwrap().0;
    assert_eq!(
        fullscreen.tile,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1200.0,
            height: 800.0
        }
    );
    placement.fullscreen(&2, false).unwrap();
    let restored = placement.placed(|_| 0.0).first().unwrap().tile.clone().unwrap().0;
    assert_eq!(restored.tile, before.tile);
    assert_eq!(restored.requested_client, before.requested_client);
}

#[test]
fn moving_a_column_keeps_its_surface_available_to_follow() {
    let mut placement = placement();
    placement.map(1, 1, WindowRules::default()).unwrap();
    placement.apply(Command::MoveColumnToWorkspaceDown);
    assert!(placement.follow(&1));
    assert_eq!(placement.focused(), Some(&1));
    assert_eq!(placement.active_tiles(), vec![(&1, true)]);
}

#[test]
fn remapping_a_surface_has_a_new_destination_and_trace() {
    let mut placement = placement();
    placement.map(1, 1, WindowRules::default()).unwrap();
    let before = placement.focused_destination();
    assert!(placement.observe(|_| Some("client".into())).is_some());
    assert!(placement.observe(|_| Some("client".into())).is_none());
    assert_eq!(placement.unmap(&1), Some(1));
    assert!(placement.observe(|_| Some("client".into())).is_some());
    placement.map(1, 1, WindowRules::default()).unwrap();
    assert_ne!(placement.focused_destination(), before);
}
