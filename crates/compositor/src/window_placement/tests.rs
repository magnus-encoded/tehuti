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
            let placements = placement.placed(|_| 0.0);
            let keys: Vec<_> = placements.iter().map(|placed| *placed.key).collect();
            prop_assert_eq!(&keys, &mapped);
            prop_assert_eq!(placement.is_empty(), mapped.is_empty());
            for placed in placements {
                prop_assert_eq!(placed.key, placed.window);
                let (tile, _) = placed.tile.unwrap();
                prop_assert_eq!(tile.fullscreen_requested, fullscreen.contains(placed.key));
                prop_assert!(tile.tile.width.is_finite() && tile.tile.width > 0.0);
                prop_assert!(tile.tile.height.is_finite() && tile.tile.height > 0.0);
            }
            prop_assert!(placement.focused().is_none_or(|focused| mapped.contains(focused)));
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

/// A saved session whose first two windows share a column and whose third has its own.
fn saved_stack() -> crate::session_restore::Saved {
    use tehuti_core::layout::{Layout, LayoutConfig, OutputId, WindowId};
    let mut layout = Layout::new(LayoutConfig::default());
    let area = Rect {
        x: 0.0,
        y: 0.0,
        width: 1200.0,
        height: 800.0,
    };
    layout
        .add_output(OutputId(0), OutputGeometry { area, usable: area }, None)
        .unwrap();
    for window in 1..=3_u64 {
        layout
            .add_window(WindowId(window), WindowRules::default())
            .unwrap();
    }
    layout.apply(Command::FocusColumnFirst);
    layout.apply(Command::ConsumeWindowIntoColumn);
    let installed = vec!["app".to_owned()];
    crate::session_restore::Saved::capture(&layout.snapshot(), |_| Some("app".to_owned()), &installed)
}

/// Opens each key through a restore claim, as the compositor does, and returns the window
/// keys of every column of the first workspace.
fn restored(opens: &[(u8, usize)], closed: &[u8]) -> Vec<Vec<u8>> {
    let now = std::time::Instant::now();
    let mut placements = crate::session_restore::Placements::<u8>::new(&saved_stack(), now);
    let mut placement = placement();
    for (key, token) in opens {
        let claim = placements.claim(Some(*token), now, key).unwrap();
        let open_at = placement.open_at(claim);
        let rules = WindowRules {
            open_at: Some(open_at),
            ..WindowRules::default()
        };
        placement.map(*key, *key, rules).unwrap();
        if closed.contains(key) {
            let _window = placement.unmap(key);
        }
    }
    let keys: Vec<(u64, u8)> = placement
        .placed(|_| 0.0)
        .iter()
        .map(|placed| (placed.identity, *placed.key))
        .collect();
    placement.snapshot().outputs[0].workspaces[0]
        .columns
        .iter()
        .map(|column| {
            column
                .tiles
                .iter()
                .filter_map(|tile| {
                    keys.iter()
                        .find(|(identity, _)| *identity == tile.window.0)
                        .map(|(_, key)| *key)
                })
                .collect()
        })
        .collect()
}

#[test]
fn saved_stack_returns_stacked_in_saved_order() {
    assert_eq!(
        restored(&[(10, 0), (11, 1), (12, 2)], &[]),
        vec![vec![10, 11], vec![12]]
    );
}

#[test]
fn a_closed_anchor_sends_the_stacked_tile_to_its_saved_column() {
    assert_eq!(restored(&[(10, 0), (11, 1)], &[10]), vec![vec![11]]);
}

/// A restore of three windows whose second had focus, driven the way the compositor drives it.
struct Restore {
    now: std::time::Instant,
    placements: crate::session_restore::Placements<u8>,
    placement: Placement<u8, u8>,
}

impl Restore {
    fn new() -> Self {
        let now = std::time::Instant::now();
        let mut saved_layout = placement();
        for key in 1..=3_u8 {
            saved_layout.map(key, key, WindowRules::default()).unwrap();
        }
        saved_layout.focus(&2).unwrap();
        let installed = vec!["app".to_owned()];
        let saved = crate::session_restore::Saved::capture(
            &saved_layout.snapshot(),
            |_| Some("app".to_owned()),
            &installed,
        );
        Self {
            now,
            placements: crate::session_restore::Placements::new(&saved, now),
            placement: placement(),
        }
    }

    fn open(&mut self, key: u8, token: usize) {
        self.placement.guard_restored_focus(&mut self.placements);
        let claim = self.placements.claim(Some(token), self.now, &key).unwrap();
        let rules = WindowRules {
            open_at: Some(self.placement.open_at(claim)),
            ..WindowRules::default()
        };
        self.placement.map(key, key, rules).unwrap();
        self.placement
            .finish_restored_focus(&mut self.placements, self.now);
    }

    fn tick(&mut self, later: core::time::Duration) {
        self.now = self.now.checked_add(later).unwrap();
        self.placement.guard_restored_focus(&mut self.placements);
        self.placement
            .finish_restored_focus(&mut self.placements, self.now);
    }
}

#[test]
fn the_restored_focus_lands_on_the_saved_window_after_later_opens() {
    let mut restore = Restore::new();
    for (key, token) in [(21_u8, 1_usize), (20, 0), (22, 2)] {
        restore.open(key, token);
    }
    assert_eq!(restore.placement.focused(), Some(&21));
}

#[test]
fn a_launch_that_expires_releases_the_focus_on_the_next_tick() {
    let mut restore = Restore::new();
    restore.open(21, 1);
    restore.open(20, 0);
    assert_eq!(restore.placement.focused(), Some(&20));
    restore.tick(core::time::Duration::from_secs(30));
    assert_eq!(restore.placement.focused(), Some(&20));
    restore.tick(core::time::Duration::from_secs(31));
    assert_eq!(restore.placement.focused(), Some(&21));
}

#[test]
fn a_user_focus_change_is_never_overridden() {
    let mut restore = Restore::new();
    restore.open(22, 2);
    restore.open(21, 1);
    restore.placement.focus(&22).unwrap();
    restore.tick(core::time::Duration::from_secs(61));
    assert_eq!(restore.placement.focused(), Some(&22));
    assert!(restore.placements.is_empty());
}
