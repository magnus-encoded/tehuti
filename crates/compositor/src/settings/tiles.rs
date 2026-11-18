//! Equal square tiles for the Display and Input settings pages.

/// Where tiles may go: left, top, width and height in reference pixels.
const AREA: (usize, usize, usize, usize) = (80, 222, 1120, 380);
const GAP: usize = 14;
const LARGEST_SIDE: usize = 150;

/// The square tiles of a page: one side for every tile, rows filled from the left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Grid {
    rows: Vec<usize>,
    side: usize,
}

impl Grid {
    /// A grid with `rows[row]` tiles in each row; empty rows take no room.
    pub(super) fn new(rows: &[usize]) -> Self {
        let bands = rows.iter().filter(|length| **length > 0).count().max(1);
        let widest = rows.iter().copied().max().unwrap_or(0).max(1);
        let by_width = AREA
            .2
            .saturating_sub(GAP.saturating_mul(widest.saturating_sub(1)))
            .checked_div(widest)
            .unwrap_or(0);
        let by_height = AREA
            .3
            .saturating_sub(GAP.saturating_mul(bands.saturating_sub(1)))
            .checked_div(bands)
            .unwrap_or(0);
        Self {
            rows: rows.to_vec(),
            side: by_width.min(by_height).min(LARGEST_SIDE).max(1),
        }
    }

    pub(super) const fn side(&self) -> usize {
        self.side
    }

    /// The top-left corner of a tile, or none when the row has no such tile.
    pub(super) fn origin(&self, row: usize, col: usize) -> Option<(usize, usize)> {
        if col >= self.rows.get(row).copied()? {
            return None;
        }
        let band = self.rows.iter().take(row).filter(|length| **length > 0).count();
        let step = self.side.saturating_add(GAP);
        Some((
            AREA.0.saturating_add(col.saturating_mul(step)),
            AREA.1.saturating_add(band.saturating_mul(step)),
        ))
    }
}

/// Breaks `label` into at most `max_lines` lines of at most `per_line` characters,
/// at spaces where it can and inside a word where it must.
pub(super) fn wrap(label: &str, per_line: usize, max_lines: usize) -> Vec<String> {
    let per_line = per_line.max(1);
    let mut lines: Vec<String> = Vec::new();
    for word in label.split_whitespace() {
        let characters: Vec<char> = word.chars().collect();
        for piece in characters.chunks(per_line) {
            let piece: String = piece.iter().collect();
            let fits = lines.last().is_some_and(|line| {
                line.chars()
                    .count()
                    .saturating_add(1)
                    .saturating_add(piece.chars().count())
                    <= per_line
            });
            match lines.last_mut().filter(|_| fits) {
                Some(line) => {
                    line.push(' ');
                    line.push_str(&piece);
                }
                None => lines.push(piece),
            }
        }
    }
    lines.truncate(max_lines);
    lines
}

#[cfg(test)]
mod tests {
    use super::{AREA, GAP, Grid, wrap};
    use proptest::prelude::*;

    fn cells(grid: &Grid, rows: &[usize]) -> Vec<(usize, usize)> {
        rows.iter()
            .enumerate()
            .flat_map(|(row, length)| (0..*length).map(move |col| (row, col)))
            .filter_map(|(row, col)| grid.origin(row, col))
            .collect()
    }

    proptest! {
        #[test]
        fn every_tile_is_one_square_inside_the_area_and_none_overlap(
            rows in prop::collection::vec(0_usize..16, 0..6),
        ) {
            let grid = Grid::new(&rows);
            let side = grid.side();
            let origins = cells(&grid, &rows);
            prop_assert_eq!(origins.len(), rows.iter().sum::<usize>());
            for (x, y) in &origins {
                prop_assert!(*x >= AREA.0 && x.saturating_add(side) <= AREA.0.saturating_add(AREA.2));
                prop_assert!(*y >= AREA.1 && y.saturating_add(side) <= AREA.1.saturating_add(AREA.3));
            }
            for (index, first) in origins.iter().enumerate() {
                for second in origins.iter().skip(index.saturating_add(1)) {
                    let apart = first.0.abs_diff(second.0) >= side || first.1.abs_diff(second.1) >= side;
                    prop_assert!(apart, "tiles {first:?} and {second:?} overlap");
                }
            }
        }

        #[test]
        fn rows_fill_from_the_left_with_equal_steps(
            rows in prop::collection::vec(1_usize..16, 1..5),
        ) {
            let grid = Grid::new(&rows);
            for (row, length) in rows.iter().enumerate() {
                for col in 0..*length {
                    let (x, _) = grid.origin(row, col).unwrap_or_default();
                    prop_assert_eq!(x, AREA.0.saturating_add(col.saturating_mul(grid.side().saturating_add(GAP))));
                }
                prop_assert_eq!(grid.origin(row, *length), None);
            }
        }

        #[test]
        fn wrapped_labels_respect_both_limits(
            label in "[A-Z0-9 #-]{0,60}",
            per_line in 1_usize..14,
            max_lines in 0_usize..4,
        ) {
            let lines = wrap(&label, per_line, max_lines);
            prop_assert!(lines.len() <= max_lines);
            prop_assert!(lines.iter().all(|line| line.chars().count() <= per_line));
        }
    }
}
