//! Surface membership, layout identity and placement decisions.
use tehuti_core::layout::{
    Command, Layout, LayoutConfig, LayoutError, OutputGeometry, OutputId, Rect, Snapshot, TileView, WindowId,
    WindowRules, WorkspaceId,
};

#[derive(Debug)]
struct Entry<Key, Window> {
    key: Key,
    window: Window,
    identity: WindowId,
}

#[derive(Debug)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "conflicts with the workspace unreachable_pub lint"
)]
pub(crate) struct Placement<Key, Window> {
    layout: Layout,
    entries: Vec<Entry<Key, Window>>,
    early_fullscreen: Vec<Key>,
    trace: tehuti_core::layout_trace::LayoutTrace,
    next_window: u64,
    windows: tehuti_core::windows::Windows<Key>,
}

#[derive(Debug)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "conflicts with the workspace unreachable_pub lint"
)]
pub(crate) struct Placed<'a, Key, Window> {
    pub key: &'a Key,
    pub window: &'a Window,
    pub identity: u64,
    pub focused: bool,
    pub helper: bool,
    pub helper_position: Option<(f64, f64)>,
    pub tile: Option<(TileView, f64)>,
}

impl<Key: Clone + Eq, Window> Placement<Key, Window> {
    pub(crate) fn new() -> Self {
        Self {
            layout: Layout::new(LayoutConfig::default()),
            entries: Vec::new(),
            early_fullscreen: Vec::new(),
            trace: tehuti_core::layout_trace::LayoutTrace::default(),
            next_window: 0,
            windows: tehuti_core::windows::Windows::new(),
        }
    }

    pub(crate) fn focused_destination(&self) -> Option<u64> {
        self.layout.snapshot().focused_window.map(|identity| identity.0)
    }

    pub(crate) fn active_tiles(&self) -> Vec<(&Window, bool)> {
        let snapshot = self.layout.snapshot();
        snapshot
            .outputs
            .iter()
            .filter_map(|output| output.workspaces.get(output.active_workspace))
            .flat_map(|workspace| &workspace.columns)
            .flat_map(|column| &column.tiles)
            .filter_map(|tile| {
                self.entries
                    .iter()
                    .find(|entry| entry.identity == tile.window)
                    .map(|entry| (&entry.window, snapshot.focused_window == Some(tile.window)))
            })
            .collect()
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        self.layout.snapshot()
    }

    pub(crate) fn resize(&mut self, geometry: OutputGeometry) -> Result<(), LayoutError> {
        if self.layout.snapshot().outputs.is_empty() {
            self.layout.add_output(OutputId(0), geometry, None)
        } else {
            self.layout.update_output(OutputId(0), geometry)
        }
    }

    pub(crate) fn map(&mut self, key: Key, window: Window, rules: WindowRules) -> Result<(), LayoutError> {
        let _effect = self.windows.step(tehuti_core::windows::Input::Map(key.clone()));
        let identity = WindowId(self.next_window);
        self.next_window = self.next_window.saturating_add(1);
        let fullscreen = self.early_fullscreen.contains(&key);
        self.early_fullscreen.retain(|candidate| candidate != &key);
        self.layout.add_window(identity, rules)?;
        if fullscreen && let Err(error) = self.layout.set_fullscreen(identity, true) {
            tracing::warn!(%error, "initial fullscreen request failed");
        }
        self.entries.push(Entry {
            key,
            window,
            identity,
        });
        Ok(())
    }

    pub(crate) fn unmap(&mut self, key: &Key) -> Option<Window> {
        self.early_fullscreen.retain(|candidate| candidate != key);
        let index = self.entries.iter().position(|entry| &entry.key == key)?;
        let entry = self.entries.remove(index);
        let _effect = self.windows.step(tehuti_core::windows::Input::Unmap(entry.key));
        if let Err(error) = self.layout.remove_window(entry.identity) {
            tracing::warn!(%error, "window removal failed");
        }
        Some(entry.window)
    }

    pub(crate) fn fullscreen(&mut self, key: &Key, fullscreen: bool) -> Result<(), LayoutError> {
        if let Some(entry) = self.entries.iter().find(|entry| &entry.key == key) {
            self.layout.set_fullscreen(entry.identity, fullscreen)
        } else {
            self.early_fullscreen.retain(|candidate| candidate != key);
            if fullscreen {
                self.early_fullscreen.push(key.clone());
            }
            Ok(())
        }
    }

    pub(crate) fn focus(&mut self, key: &Key) -> Result<(), LayoutError> {
        if let Some(entry) = self.entries.iter().find(|entry| &entry.key == key) {
            self.layout.focus_window(entry.identity)?;
        }
        Ok(())
    }

    pub(crate) fn focus_destination(&mut self, identity: u64) -> Result<(), LayoutError> {
        self.layout.focus_window(WindowId(identity))
    }

    pub(crate) fn focus_workspace_id(&mut self, workspace: WorkspaceId) -> Result<(), LayoutError> {
        self.layout.focus_workspace_id(workspace)
    }

    pub(crate) fn set_helper(&mut self, key: &Key, helper: bool) -> Result<bool, LayoutError> {
        self.entries
            .iter()
            .find(|entry| &entry.key == key)
            .map_or(Ok(false), |entry| self.layout.set_helper(entry.identity, helper))
    }

    pub(crate) fn apply(&mut self, command: Command) {
        self.layout.apply(command);
    }

    pub(crate) fn follow(&mut self, key: &Key) -> bool {
        let Some(entry) = self.entries.iter().find(|entry| &entry.key == key) else {
            return false;
        };
        let snapshot = self.layout.snapshot();
        let workspace = snapshot
            .outputs
            .iter()
            .flat_map(|output| &output.workspaces)
            .find(|workspace| {
                workspace
                    .columns
                    .iter()
                    .any(|column| column.tiles.iter().any(|tile| tile.window == entry.identity))
            })
            .map(|workspace| workspace.workspace);
        workspace.is_some_and(|workspace| self.layout.focus_workspace_id(workspace).is_ok())
    }

    pub(crate) fn preview(&self) -> Option<Rect> {
        self.layout.preview_window(WindowRules::default())
    }

    pub(crate) fn windows(&self) -> impl Iterator<Item = &Window> {
        self.entries.iter().map(|entry| &entry.window)
    }

    pub(crate) fn focused(&self) -> Option<&Key> {
        let focused = self.layout.snapshot().focused_window?;
        self.entries
            .iter()
            .find(|entry| entry.identity == focused)
            .map(|entry| &entry.key)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn window(&self, key: &Key) -> Option<&Window> {
        self.entries
            .iter()
            .find(|entry| &entry.key == key)
            .map(|entry| &entry.window)
    }

    pub(crate) fn placed(&self, width: impl Fn(&Window) -> f64) -> Vec<Placed<'_, Key, Window>> {
        let snapshot = self.layout.snapshot();
        self.entries
            .iter()
            .map(|entry| {
                let helper = snapshot
                    .outputs
                    .iter()
                    .flat_map(|output| &output.workspaces)
                    .any(|workspace| workspace.helpers.contains(&entry.identity));
                let helper_position = snapshot.outputs.iter().find_map(|output| {
                    let workspace = output.workspaces.get(output.active_workspace)?;
                    let index = workspace
                        .helpers
                        .iter()
                        .position(|helper| *helper == entry.identity)?;
                    let preceding = workspace
                        .helpers
                        .iter()
                        .take(index)
                        .filter_map(|helper| {
                            self.entries
                                .iter()
                                .find(|candidate| candidate.identity == *helper)
                                .map(|candidate| width(&candidate.window))
                        })
                        .sum::<f64>();
                    Some((
                        core::ops::Add::add(workspace.helper_position.0, preceding),
                        workspace.helper_position.1,
                    ))
                });
                Placed {
                    key: &entry.key,
                    window: &entry.window,
                    identity: entry.identity.0,
                    focused: snapshot.focused_window == Some(entry.identity),
                    helper,
                    helper_position,
                    tile: visible_tile(&snapshot, entry.identity)
                        .map(|(tile, border)| (tile.clone(), border)),
                }
            })
            .collect()
    }

    pub(crate) fn observe(
        &mut self,
        app_id: impl Fn(&Window) -> Option<String>,
    ) -> Option<tehuti_core::layout_trace::LayoutChange> {
        let entries = &self.entries;
        self.trace.observe(&self.layout.snapshot(), |identity| {
            entries
                .iter()
                .find(|entry| entry.identity == identity)
                .and_then(|entry| app_id(&entry.window))
        })
    }

    pub(crate) fn window_action(
        &mut self,
        input: tehuti_core::windows::Input<Key>,
    ) -> Option<tehuti_core::windows::Effect<Key>> {
        self.windows.step(input)
    }
}

fn visible_tile(snapshot: &Snapshot, identity: WindowId) -> Option<(&TileView, f64)> {
    snapshot.outputs.iter().find_map(|output| {
        output
            .workspaces
            .get(output.active_workspace)
            .and_then(|workspace| {
                workspace.columns.iter().find_map(|column| {
                    column.tiles.iter().enumerate().find_map(|(index, tile)| {
                        (tile.window == identity
                            && (column.display == tehuti_core::layout::ColumnDisplay::Normal
                                || index == column.focused_tile))
                            .then_some((tile, output.border))
                    })
                })
            })
    })
}
