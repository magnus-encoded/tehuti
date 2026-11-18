//! Focus over rows of equal square tiles, shared by the Display and Input settings pages.
//!
//! Tiles fill a row from the left like windows in the columns layout. Moving
//! focus never changes a setting, and a tile that arrives late joins the end of
//! its row without moving the tile that has focus.

use tehuti_protocol::ButtonName;

/// The focused tile: a row and the column inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Focus {
    pub row: usize,
    pub col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Leftward,
    Rightward,
    Upward,
    Downward,
}

impl Direction {
    /// The direction a d-pad button points, if it is one.
    #[must_use]
    pub fn from_button(name: ButtonName) -> Option<Self> {
        [
            (ButtonName::DpadLeft, Self::Leftward),
            (ButtonName::DpadRight, Self::Rightward),
            (ButtonName::DpadUp, Self::Upward),
            (ButtonName::DpadDown, Self::Downward),
        ]
        .into_iter()
        .find_map(|(button, direction)| (button == name).then_some(direction))
    }
}

/// The first row that holds a tile, as a focus on its first tile.
#[must_use]
pub fn first(rows: &[usize]) -> Focus {
    Focus {
        row: rows.iter().position(|length| *length > 0).unwrap_or(0),
        col: 0,
    }
}

/// Keeps `focus` on a tile: the nearest row that exists, the nearest column in it.
#[must_use]
pub fn clamp(rows: &[usize], focus: Focus) -> Focus {
    let row = focus.row.min(rows.len().saturating_sub(1));
    let last = rows.get(row).copied().unwrap_or(0).saturating_sub(1);
    Focus {
        row,
        col: focus.col.min(last),
    }
}

/// Moves focus one tile. Left and right stop at the row ends. Up and down skip
/// empty rows and keep the column as far as the new row reaches; with no row
/// to move to, focus stays.
#[must_use]
pub fn step(rows: &[usize], focus: Focus, direction: Direction) -> Focus {
    let focus = clamp(rows, focus);
    let length = |row: usize| rows.get(row).copied().unwrap_or(0);
    let landing = |row: usize| Focus {
        row,
        col: focus.col.min(length(row).saturating_sub(1)),
    };
    match direction {
        Direction::Leftward => Focus {
            col: focus.col.saturating_sub(1),
            ..focus
        },
        Direction::Rightward => Focus {
            col: focus
                .col
                .saturating_add(1)
                .min(length(focus.row).saturating_sub(1)),
            ..focus
        },
        Direction::Upward => (0..focus.row)
            .rev()
            .find(|row| length(*row) > 0)
            .map_or(focus, landing),
        Direction::Downward => (focus.row.saturating_add(1)..rows.len())
            .find(|row| length(*row) > 0)
            .map_or(focus, landing),
    }
}

/// Tiles in the order the user already sees them, with new arrivals at the end.
///
/// Tiles in `known` that `current` still has keep their place (showing their
/// latest state); tiles only in `current` follow in the order given; tiles only
/// in `known` are gone.
#[must_use]
pub fn settle<T, K: PartialEq>(known: &[T], current: Vec<T>, key: impl Fn(&T) -> K) -> Vec<T> {
    let mut remaining: Vec<Option<T>> = current.into_iter().map(Some).collect();
    let mut settled = Vec::with_capacity(remaining.len());
    for old in known {
        let wanted = key(old);
        let found = remaining
            .iter_mut()
            .find(|slot| slot.as_ref().is_some_and(|tile| key(tile) == wanted));
        if let Some(tile) = found.and_then(Option::take) {
            settled.push(tile);
        }
    }
    settled.extend(remaining.into_iter().flatten());
    settled
}
