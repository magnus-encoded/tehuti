//! One-line descriptions of the layout for the session log.
//!
//! Lines name windows by `app_id` only, never by title.
use crate::layout::{Snapshot, WindowId};

/// The layout description before and after one change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutChange {
    /// `None` for the first observed layout.
    pub before: Option<String>,
    pub after: String,
}

/// Remembers the last described layout so each change is reported once.
#[derive(Debug, Clone, Default)]
pub struct LayoutTrace {
    last: Option<String>,
}

impl LayoutTrace {
    /// Returns the change when the description differs from the last one observed.
    pub fn observe(
        &mut self,
        snapshot: &Snapshot,
        app_id: impl Fn(WindowId) -> Option<String>,
    ) -> Option<LayoutChange> {
        let after = describe(snapshot, app_id);
        if self.last.as_deref() == Some(after.as_str()) {
            return None;
        }
        let before = self.last.replace(after.clone());
        Some(LayoutChange { before, after })
    }
}

/// Describes the active workspace of each output: output width, view offset,
/// focused column index, and each column's `app_id`s, tile width and x.
#[must_use]
pub fn describe(snapshot: &Snapshot, app_id: impl Fn(WindowId) -> Option<String>) -> String {
    snapshot
        .outputs
        .iter()
        .map(|output| {
            let head = format!(
                "output={} output_width={}",
                output.output.0, output.geometry.area.width
            );
            let Some(workspace) = output.workspaces.get(output.active_workspace) else {
                return head;
            };
            let focused = workspace
                .focused_column
                .map_or_else(|| String::from("none"), |column| column.to_string());
            let columns: Vec<String> = workspace
                .columns
                .iter()
                .map(|column| {
                    let apps: Vec<String> = column
                        .tiles
                        .iter()
                        .map(|tile| app_id(tile.window).unwrap_or_else(|| String::from("?")))
                        .collect();
                    column.tiles.get(column.focused_tile).map_or_else(
                        || format!("[{}]", apps.join(",")),
                        |tile| format!("[{}] w={} x={}", apps.join(","), tile.tile.width, tile.tile.x),
                    )
                })
                .collect();
            format!(
                "{head} workspace={} view_offset={} focused={focused} columns={}",
                workspace.workspace.0,
                workspace.view_offset,
                columns.join(" | ")
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}
