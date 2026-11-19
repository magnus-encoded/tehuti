//! Equal square tiles for the Display and Input settings pages.

use tehuti_core::tiles::Focus;

/// Where tiles may go: left, top, width and height in reference pixels.
const AREA: (usize, usize, usize, usize) = (80, 222, 1120, 420);
const GAP: usize = 14;
const LARGEST_SIDE: usize = 150;
/// The side of a Display tile: 15% of the 1280-wide reference, so it scales with the output.
pub(super) const DISPLAY_SIDE: usize = 192;
/// The largest side of an Input tile; the page shrinks it until pads, their settings and microphones all show.
const INPUT_SIDE_MAX: usize = 180;
/// The smallest side of an Input tile; below it the page scrolls rows into view instead.
pub(super) const INPUT_SIDE_MIN: usize = 114;
/// The height kept under every Input tile; the pad row's slot carries the settings row heading.
pub(super) const INPUT_CAPTION: usize = 16;

/// The square tiles of a page: one side for every tile, rows filled from the left.
///
/// A row that is wider than the area scrolls sideways at the same tile size,
/// just far enough to keep the focused tile in view; the other rows rest at
/// their start. Rows that do not fit under each other scroll the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Grid {
    rows: Vec<usize>,
    side: usize,
    caption: usize,
    focus: Focus,
}

impl Grid {
    /// A grid with `rows[row]` tiles in each row; empty rows take no room.
    /// `caption` is the height kept under every tile for its name. With no
    /// `side`, the tiles shrink until every row fits; with one, they keep it.
    pub(super) fn new(rows: &[usize], caption: usize, focus: Focus, side: Option<usize>) -> Self {
        Self {
            rows: rows.to_vec(),
            side: side.map_or_else(
                || Self::fitting_side(rows, caption).clamp(1, LARGEST_SIDE),
                |fixed| fixed.max(1),
            ),
            caption,
            focus,
        }
    }

    /// The Input page's grid: tiles shrink from their largest side until every row shows at once.
    pub(super) fn input(rows: &[usize], focus: Focus) -> Self {
        Self {
            rows: rows.to_vec(),
            side: Self::fitting_side(rows, INPUT_CAPTION).clamp(INPUT_SIDE_MIN, INPUT_SIDE_MAX),
            caption: INPUT_CAPTION,
            focus,
        }
    }

    /// The side at which every row with tiles fits under the others.
    fn fitting_side(rows: &[usize], caption: usize) -> usize {
        let bands = rows.iter().filter(|length| **length > 0).count().max(1);
        AREA.3
            .saturating_sub(GAP.saturating_mul(bands.saturating_sub(1)))
            .checked_div(bands)
            .unwrap_or(0)
            .saturating_sub(caption)
    }

    pub(super) const fn side(&self) -> usize {
        self.side
    }

    /// How many tiles of a row show at once.
    fn visible(&self) -> usize {
        AREA.2
            .saturating_add(GAP)
            .checked_div(self.side.saturating_add(GAP))
            .unwrap_or(1)
            .max(1)
    }

    /// How many rows of tiles show at once.
    fn visible_bands(&self) -> usize {
        AREA.3
            .saturating_add(GAP)
            .checked_div(self.side.saturating_add(self.caption).saturating_add(GAP))
            .unwrap_or(1)
            .max(1)
    }

    /// Position of `row` among the rows that hold tiles.
    fn band(&self, row: usize) -> usize {
        self.rows.iter().take(row).filter(|length| **length > 0).count()
    }

    /// The first tile of `row` that shows.
    fn first_shown(&self, row: usize) -> usize {
        if row == self.focus.row {
            self.focus.col.saturating_add(1).saturating_sub(self.visible())
        } else {
            0
        }
    }

    /// The top-left corner of a tile, or none when the row has no such tile or it is scrolled out of view.
    pub(super) fn origin(&self, row: usize, col: usize) -> Option<(usize, usize)> {
        if col >= self.rows.get(row).copied()? {
            return None;
        }
        let shown = col.checked_sub(self.first_shown(row))?;
        if shown >= self.visible() {
            return None;
        }
        let first_band = self
            .band(self.focus.row)
            .saturating_add(1)
            .saturating_sub(self.visible_bands());
        let shown_band = self.band(row).checked_sub(first_band)?;
        if shown_band >= self.visible_bands() {
            return None;
        }
        let step = self.side.saturating_add(GAP);
        let band_step = step.saturating_add(self.caption);
        Some((
            AREA.0.saturating_add(shown.saturating_mul(step)),
            AREA.1.saturating_add(shown_band.saturating_mul(band_step)),
        ))
    }
}

/// How many characters of `points` size fit across a tile of `width`.
pub(super) fn per_line(width: usize, points: usize) -> usize {
    width
        .saturating_sub(12)
        .saturating_mul(10)
        .checked_div(points.saturating_mul(6))
        .unwrap_or(1)
}

/// `text` cut to at most `per_line` characters, ending in two dots when it was cut.
pub(super) fn ellipsize(text: &str, per_line: usize) -> String {
    if text.chars().count() <= per_line {
        return text.to_owned();
    }
    let kept = per_line.saturating_sub(2);
    let head: String = text.chars().take(kept).collect();
    let cut = head.trim_end();
    if per_line >= 2 {
        format!("{cut}..")
    } else {
        cut.to_owned()
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
    if lines.len() > max_lines && max_lines > 0 {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            *last = ellipsize(&format!("{last} .."), per_line);
        }
    }
    lines.truncate(max_lines);
    lines
}

#[cfg(test)]
mod tests {
    use super::{AREA, GAP, Grid, ellipsize, per_line, wrap};
    use proptest::prelude::*;
    use tehuti_core::tiles::{Focus, clamp};

    fn cells(grid: &Grid, rows: &[usize]) -> Vec<(usize, usize)> {
        rows.iter()
            .enumerate()
            .flat_map(|(row, length)| (0..*length).map(move |col| (row, col)))
            .filter_map(|(row, col)| grid.origin(row, col))
            .collect()
    }

    proptest! {
        #[test]
        fn shown_tiles_are_squares_inside_the_area_and_none_overlap(
            rows in prop::collection::vec(0_usize..16, 0..6),
            caption in 0_usize..30,
            fixed in prop::option::of(1_usize..260),
            spot in (0_usize..6, 0_usize..16),
        ) {
            let focus = clamp(&rows, Focus { row: spot.0, col: spot.1 });
            let grid = Grid::new(&rows, caption, focus, fixed);
            let side = grid.side();
            let origins = cells(&grid, &rows);
            for (x, y) in &origins {
                prop_assert!(*x >= AREA.0 && x.saturating_add(side) <= AREA.0.saturating_add(AREA.2));
                prop_assert!(*y >= AREA.1 && y.saturating_add(side).saturating_add(caption) <= AREA.1.saturating_add(AREA.3));
            }
            for (index, first) in origins.iter().enumerate() {
                for second in origins.iter().skip(index.saturating_add(1)) {
                    let apart = first.0.abs_diff(second.0) >= side || first.1.abs_diff(second.1) >= side;
                    prop_assert!(apart, "tiles {first:?} and {second:?} overlap");
                }
            }
        }

        #[test]
        fn every_input_row_shows_whatever_has_focus(
            pads in 1_usize..9,
            microphones in 2_usize..14,
            spot in (0_usize..3, 0_usize..14),
        ) {
            let rows = [pads, 4, microphones];
            let focus = clamp(&rows, Focus { row: spot.0, col: spot.1 });
            let grid = Grid::input(&rows, focus);
            for (row, length) in rows.iter().enumerate() {
                let col = if row == focus.row { focus.col } else { 0 };
                prop_assert!(*length > 0 && grid.origin(row, col).is_some(), "row {row} hidden with focus {focus:?}");
            }
        }

        #[test]
        fn tile_size_does_not_shrink_when_a_row_grows(
            rows in prop::collection::vec(1_usize..4, 1..5),
            extra in 0_usize..30,
        ) {
            let mut longer = rows.clone();
            longer[0] = longer[0].saturating_add(extra);
            prop_assert_eq!(
                Grid::new(&rows, 0, Focus::default(), None).side(),
                Grid::new(&longer, 0, Focus::default(), None).side()
            );
        }

        #[test]
        fn the_focused_tile_is_always_in_view(
            rows in prop::collection::vec(1_usize..40, 1..5),
            spot in (0_usize..5, 0_usize..40),
        ) {
            let focus = clamp(&rows, Focus { row: spot.0, col: spot.1 });
            let grid = Grid::new(&rows, 20, focus, Some(super::DISPLAY_SIDE));
            prop_assert!(grid.origin(focus.row, focus.col).is_some());
        }

        #[test]
        fn shown_tiles_have_equal_steps_and_a_scrolled_row_ends_at_the_focus(
            length in 1_usize..40,
            col in 0_usize..40,
        ) {
            let rows = [length];
            let focus = clamp(&rows, Focus { row: 0, col });
            let grid = Grid::new(&rows, 0, focus, None);
            let step = grid.side().saturating_add(GAP);
            let shown: Vec<usize> = (0..length).filter_map(|index| grid.origin(0, index).map(|(x, _)| x)).collect();
            prop_assert_eq!(shown[0], AREA.0);
            for pair in shown.windows(2) {
                prop_assert_eq!(pair[1].saturating_sub(pair[0]), step);
            }
        }

        #[test]
        fn captions_never_reach_past_their_tile(
            text in "[A-Z0-9 ().#-]{0,60}",
            side in super::INPUT_SIDE_MIN..260_usize,
            points in 8_usize..20,
        ) {
            let fit = ellipsize(&text, per_line(side, points));
            prop_assert!(fit.chars().count().saturating_mul(points).saturating_mul(6) / 10 <= side.saturating_sub(12));
        }

        #[test]
        fn input_tiles_keep_their_size_and_the_focused_row_scrolls_into_view(
            rows in prop::collection::vec(0_usize..12, 1..8),
            spot in (0_usize..8, 0_usize..12),
        ) {
            let focus = clamp(&rows, Focus { row: spot.0, col: spot.1 });
            let grid = Grid::input(&rows, focus);
            prop_assert!(grid.side() >= super::INPUT_SIDE_MIN);
            if rows.get(focus.row).copied().unwrap_or(0) > 0 {
                prop_assert!(grid.origin(focus.row, focus.col).is_some());
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
