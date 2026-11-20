//! Tile focus: moving never leaves the tiles, and late tiles join at the row end (#782).

#[cfg(test)]
mod tiles {
    use proptest::prelude::*;
    use tehuti_core::tiles::{Direction, Focus, clamp, first, settle, step};

    fn directions() -> impl Strategy<Value = Direction> {
        prop::sample::select(vec![
            Direction::Leftward,
            Direction::Rightward,
            Direction::Upward,
            Direction::Downward,
        ])
    }

    fn rows_with_a_tile() -> impl Strategy<Value = Vec<usize>> {
        prop::collection::vec(0_usize..8, 1..6).prop_filter("some row holds a tile", |rows| {
            rows.iter().any(|length| *length > 0)
        })
    }

    proptest! {
        #[test]
        fn focus_never_leaves_the_tiles(
            rows in rows_with_a_tile(),
            moves in prop::collection::vec(directions(), 0..60),
        ) {
            let mut focus = first(&rows);
            for direction in moves {
                focus = step(&rows, focus, direction);
                prop_assert!(rows[focus.row] > 0, "focus is on a row with tiles");
                prop_assert!(focus.col < rows[focus.row], "focus is on a tile of its row");
            }
        }

        #[test]
        fn left_and_right_stay_in_their_row_and_move_one_tile(
            rows in rows_with_a_tile(),
            row in 0_usize..6,
            col in 0_usize..8,
        ) {
            let focus = clamp(&rows, Focus { row, col });
            prop_assume!(rows[focus.row] > 0);
            let right = step(&rows, focus, Direction::Rightward);
            let left = step(&rows, focus, Direction::Leftward);
            prop_assert_eq!(right.row, focus.row);
            prop_assert_eq!(left.row, focus.row);
            prop_assert!(right.col == focus.col || right.col == focus.col.saturating_add(1));
            prop_assert!(left.col == focus.col || left.col.saturating_add(1) == focus.col);
            prop_assert_eq!(right.col == focus.col, focus.col.saturating_add(1) == rows[focus.row]);
            prop_assert_eq!(left.col == focus.col, focus.col == 0);
        }

        #[test]
        fn up_and_down_keep_the_column_as_far_as_the_row_reaches(
            rows in rows_with_a_tile(),
            row in 0_usize..6,
            col in 0_usize..8,
        ) {
            let focus = clamp(&rows, Focus { row, col });
            prop_assume!(rows[focus.row] > 0);
            for direction in [Direction::Upward, Direction::Downward] {
                let moved = step(&rows, focus, direction);
                if moved.row == focus.row {
                    prop_assert_eq!(moved, focus);
                } else {
                    prop_assert_eq!(moved.col, focus.col.min(rows[moved.row].saturating_sub(1)));
                }
            }
        }

        #[test]
        fn late_tiles_follow_the_known_ones_and_known_tiles_keep_their_places(
            known in prop::collection::btree_set(0_u8..50, 0..8),
            late in prop::collection::btree_set(50_u8..100, 0..8),
            gone in prop::collection::btree_set(0_u8..50, 0..3),
        ) {
            let known: Vec<u8> = known.into_iter().collect();
            let mut current: Vec<u8> = late.iter().copied().collect();
            current.extend(known.iter().copied().filter(|tile| !gone.contains(tile)));
            let settled = settle(&known, current, |tile| *tile);
            let kept: Vec<u8> = known.iter().copied().filter(|tile| !gone.contains(tile)).collect();
            prop_assert_eq!(&settled[..kept.len()], &kept[..]);
            prop_assert_eq!(&settled[kept.len()..], &late.into_iter().collect::<Vec<u8>>()[..]);
        }
    }
}
