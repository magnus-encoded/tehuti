//! Pure scrollable-column layout: per output a vertical list of workspaces,
//! per workspace a horizontal strip of columns, per column a stack of tiles.
//!
//! Behaviour follows `docs/research/niri-behaviour-spec.md`. Coordinates are
//! fractional logical pixels. Tile rectangles in a [`Snapshot`] are relative
//! to the top-left corner of their output area, with the view offset applied.
use core::fmt;
use core::ops::{Add, AddAssign, Div, Mul, Sub};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutputId(pub u64);

/// Stable identity of a workspace, assigned by the layout; survives reorders and output moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkspaceId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

/// Negative values enlarge the working area.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Struts {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

/// `Proportion` is a fraction of the working area applied to the tile;
/// `Fixed` is logical pixels applied to the window inside the border.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PresetSize {
    Proportion(f64),
    Fixed(f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowHeight {
    Auto,
    Proportion(f64),
    Fixed(f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SizeChange {
    SetProportion(f64),
    SetFixed(f64),
    /// Percentage points of the working area, such as `10.0` for `"+10%"`.
    AdjustPercent(f64),
    AdjustFixed(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CenterFocusedColumn {
    #[default]
    Never,
    Always,
    OnOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColumnDisplay {
    #[default]
    Normal,
    Tabbed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Upward,
    Downward,
}

/// How a tile relates to the scrolling layout's normal geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TileMode {
    #[default]
    Normal,
    Fullscreen,
    /// The client is told it is fullscreen while its tile keeps its normal size.
    WindowedFullscreen,
    MaximizedToEdges,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutConfig {
    pub gaps: f64,
    pub border: f64,
    pub struts: Struts,
    pub preset_column_widths: Vec<PresetSize>,
    pub preset_window_heights: Vec<PresetSize>,
    /// `None` lets the client choose its width.
    pub default_column_width: Option<PresetSize>,
    pub default_column_display: ColumnDisplay,
    pub center_focused_column: CenterFocusedColumn,
    pub always_center_single_column: bool,
    pub empty_workspace_above_first: bool,
    pub workspace_auto_back_and_forth: bool,
}

impl Default for LayoutConfig {
    /// The shipped defaults of the spec.
    fn default() -> Self {
        Self {
            gaps: 16.0_f64,
            border: 0.0_f64,
            struts: Struts::default(),
            preset_column_widths: presets(),
            preset_window_heights: presets(),
            default_column_width: Some(PresetSize::Proportion(0.5_f64)),
            default_column_display: ColumnDisplay::Normal,
            center_focused_column: CenterFocusedColumn::Never,
            always_center_single_column: false,
            empty_workspace_above_first: false,
            workspace_auto_back_and_forth: false,
        }
    }
}

/// Opening properties; applied once, when the window is added.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WindowRules {
    /// `Some(None)` lets the client choose its width.
    pub default_column_width: Option<Option<PresetSize>>,
    pub default_window_height: Option<PresetSize>,
    pub default_column_display: Option<ColumnDisplay>,
    pub min_height: Option<f64>,
    pub max_height: Option<f64>,
    pub open_focused: Option<bool>,
    pub open_fullscreen: bool,
    pub open_maximized: bool,
    pub open_maximized_to_edges: bool,
    pub open_on_output: Option<OutputId>,
    pub open_on_workspace: Option<String>,
}

/// Rectangles in global logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputGeometry {
    pub area: Rect,
    /// The output area minus layer-shell exclusive zones.
    pub usable: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    FocusColumnLeft,
    FocusColumnRight,
    FocusColumnFirst,
    FocusColumnLast,
    MoveColumnLeft,
    MoveColumnRight,
    FocusWindowUp,
    FocusWindowDown,
    FocusWindowTop,
    FocusWindowBottom,
    FocusWindowDownOrTop,
    FocusWindowUpOrBottom,
    FocusWindowUpOrColumnRight,
    /// 1-based position in the focused column.
    FocusWindowInColumn(usize),
    MoveWindowUp,
    MoveWindowDown,
    ConsumeOrExpelWindowLeft,
    ConsumeOrExpelWindowRight,
    ConsumeWindowIntoColumn,
    ExpelWindowFromColumn,
    ToggleColumnTabbedDisplay,
    SwitchPresetColumnWidth,
    SwitchPresetColumnWidthBack,
    SetColumnWidth(SizeChange),
    MaximizeColumn,
    ExpandColumnToAvailableWidth,
    SwitchPresetWindowHeight,
    SwitchPresetWindowHeightBack,
    SetWindowHeight(SizeChange),
    ResetWindowHeight,
    CenterColumn,
    FocusWorkspaceDown,
    FocusWorkspaceUp,
    /// 0-based position on the focused output.
    FocusWorkspaceAt(usize),
    MoveColumnToWorkspaceDown,
    MoveColumnToWorkspaceUp,
    /// 0-based position on the focused output.
    MoveColumnToWorkspaceAt(usize),
    MoveWorkspaceDown,
    MoveWorkspaceUp,
    FocusMonitor(Direction),
    MoveColumnToMonitor(Direction),
    MoveWorkspaceToMonitor(Direction),
    ToggleFullscreen,
    ToggleWindowedFullscreen,
    ToggleMaximizeToEdges,
}

/// The layout command a context's tiling action stands for.
#[must_use]
pub const fn command_for(action: tehuti_protocol::LayoutAction) -> Command {
    match action {
        tehuti_protocol::LayoutAction::FocusColumnLeft => Command::FocusColumnLeft,
        tehuti_protocol::LayoutAction::FocusColumnRight => Command::FocusColumnRight,
        tehuti_protocol::LayoutAction::FocusWindowUp => Command::FocusWindowUp,
        tehuti_protocol::LayoutAction::FocusWindowDown => Command::FocusWindowDown,
        tehuti_protocol::LayoutAction::MoveColumnLeft => Command::MoveColumnLeft,
        tehuti_protocol::LayoutAction::MoveColumnRight => Command::MoveColumnRight,
        tehuti_protocol::LayoutAction::SwitchPresetColumnWidth => Command::SwitchPresetColumnWidth,
        tehuti_protocol::LayoutAction::FocusWorkspaceDown => Command::FocusWorkspaceDown,
        tehuti_protocol::LayoutAction::FocusWorkspaceUp => Command::FocusWorkspaceUp,
        tehuti_protocol::LayoutAction::MoveColumnToWorkspaceDown => Command::MoveColumnToWorkspaceDown,
        tehuti_protocol::LayoutAction::MoveColumnToWorkspaceUp => Command::MoveColumnToWorkspaceUp,
        tehuti_protocol::LayoutAction::ToggleFullscreen => Command::ToggleFullscreen,
        tehuti_protocol::LayoutAction::MaximizeColumn => Command::MaximizeColumn,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    UnknownWindow(WindowId),
    DuplicateWindow(WindowId),
    UnknownOutput(OutputId),
    DuplicateOutput(OutputId),
    DuplicateWorkspaceName,
    NoOutput,
    InvalidGeometry,
    InvalidConfig,
    InvalidSize,
}

impl fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownWindow(window) => {
                write!(formatter, "unknown window {}", window.0)
            }
            Self::DuplicateWindow(window) => {
                write!(formatter, "window {} already in the layout", window.0)
            }
            Self::UnknownOutput(output) => {
                write!(formatter, "unknown output {}", output.0)
            }
            Self::DuplicateOutput(output) => {
                write!(formatter, "output {} already in the layout", output.0)
            }
            Self::DuplicateWorkspaceName => formatter.write_str("workspace name already in use"),
            Self::InvalidGeometry => formatter.write_str("invalid output geometry"),
            Self::InvalidConfig => formatter.write_str("invalid layout configuration"),
            Self::InvalidSize => formatter.write_str("invalid window size"),
            Self::NoOutput => formatter.write_str("no output connected"),
        }
    }
}

impl core::error::Error for LayoutError {}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub outputs: Vec<OutputView>,
    pub focused_output: Option<OutputId>,
    /// The window that receives input.
    pub focused_window: Option<WindowId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputView {
    pub output: OutputId,
    pub geometry: OutputGeometry,
    /// Output-local: the usable area minus struts.
    pub working_area: Rect,
    pub gaps: f64,
    pub border: f64,
    pub workspaces: Vec<WorkspaceView>,
    pub active_workspace: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceView {
    pub workspace: WorkspaceId,
    pub name: Option<String>,
    pub original_output: Option<OutputId>,
    /// Strip x coordinate shown at the working-area left edge.
    pub view_offset: f64,
    pub columns: Vec<ColumnView>,
    pub focused_column: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnView {
    /// `None` while the client chooses its width.
    pub width: Option<PresetSize>,
    pub display: ColumnDisplay,
    pub tiles: Vec<TileView>,
    pub focused_tile: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TileView {
    pub window: WindowId,
    /// Output-local, including the border.
    pub tile: Rect,
    /// Client size requested in the configure; a zero width lets the client choose.
    pub requested_client: Size,
    /// The fullscreen state told to the client.
    pub fullscreen_requested: bool,
    pub mode: TileMode,
    pub height: WindowHeight,
}

fn presets() -> Vec<PresetSize> {
    vec![
        PresetSize::Proportion(1.0_f64.div(3.0)),
        PresetSize::Proportion(0.5_f64),
        PresetSize::Proportion(2.0_f64.div(3.0)),
    ]
}

#[derive(Debug, Clone)]
struct Tile {
    window: WindowId,
    height: WindowHeight,
    mode: TileMode,
    min_height: Option<f64>,
    max_height: Option<f64>,
    opened_view: Option<(f64, Option<WindowId>, f64)>,
    fullscreen_view: Option<(f64, WindowId)>,
}

#[derive(Debug, Clone)]
struct Column {
    width: Option<PresetSize>,
    client_width: f64,
    display: ColumnDisplay,
    tiles: Vec<Tile>,
    focused: usize,
    resized: Option<WindowId>,
}

#[derive(Debug, Clone)]
struct Workspace {
    identity: WorkspaceId,
    name: Option<String>,
    original_output: Option<OutputId>,
    columns: Vec<Column>,
    focused: Option<usize>,
    view: f64,
}

#[derive(Debug, Clone)]
struct Output {
    identity: OutputId,
    geometry: OutputGeometry,
    config: LayoutConfig,
    workspaces: Vec<Workspace>,
    active: usize,
    previous: Option<WorkspaceId>,
}

#[derive(Debug, Clone)]
pub struct Layout {
    config: LayoutConfig,
    outputs: Vec<Output>,
    focused: Option<OutputId>,
    next_workspace: u64,
    disconnected: Vec<(OutputId, WorkspaceId)>,
    detached: Vec<Workspace>,
}

impl Layout {
    #[must_use]
    pub const fn new(config: LayoutConfig) -> Self {
        Self {
            config,
            outputs: Vec::new(),
            focused: None,
            next_workspace: 0,
            disconnected: Vec::new(),
            detached: Vec::new(),
        }
    }
    /// Validating constructor: rejects non-finite or negative gaps and borders, and
    /// non-finite or non-positive sizes in the default and preset widths and heights.
    ///
    /// # Errors
    /// Returns an error when a float in `config` is out of range.
    pub fn try_new(config: LayoutConfig) -> Result<Self, LayoutError> {
        validate_config(&config)?;
        Ok(Self::new(config))
    }
    const fn workspace(&mut self, output: OutputId) -> Workspace {
        Workspace::empty(&mut self.next_workspace, output)
    }
    /// # Errors
    /// Returns an error if the output is already connected.
    pub fn add_output(
        &mut self,
        identity: OutputId,
        geometry: OutputGeometry,
        layout_override: Option<LayoutConfig>,
    ) -> Result<(), LayoutError> {
        if self.outputs.iter().any(|output| output.identity == identity) {
            return Err(LayoutError::DuplicateOutput(identity));
        }
        let config = layout_override.unwrap_or_else(|| self.config.clone());
        validate_output(geometry, &config)?;
        let mut workspaces = Vec::new();
        for output in &mut self.outputs {
            let mut index = 0;
            while index < output.workspaces.len() {
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                if output.workspaces[index].original_output == Some(identity) {
                    workspaces.push(output.workspaces.remove(index));
                } else {
                    index = index.saturating_add(1);
                }
            }
            output.active = output.active.min(output.workspaces.len().saturating_sub(1));
        }
        let mut index = 0;
        while index < self.detached.len() {
            #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
            if self.detached[index].original_output == Some(identity) || self.outputs.is_empty() {
                workspaces.push(self.detached.remove(index));
            } else {
                index = index.saturating_add(1);
            }
        }
        if workspaces.is_empty() {
            workspaces.push(self.workspace(identity));
        }
        let remembered = self
            .disconnected
            .iter()
            .find(|(output, _)| *output == identity)
            .map(|(_, workspace)| *workspace);
        let active = remembered
            .and_then(|workspace| {
                workspaces
                    .iter()
                    .position(|candidate| candidate.identity == workspace)
            })
            .unwrap_or(0);
        self.outputs.push(Output {
            identity,
            geometry,
            config,
            workspaces,
            active,
            previous: None,
        });
        if self.focused.is_none() {
            self.focused = Some(identity);
        }
        self.normalize();
        Ok(())
    }
    /// # Errors
    /// Returns an error if the output is not connected.
    pub fn remove_output(&mut self, identity: OutputId) -> Result<(), LayoutError> {
        let index = self
            .outputs
            .iter()
            .position(|output| output.identity == identity)
            .ok_or(LayoutError::UnknownOutput(identity))?;
        let output = self.outputs.remove(index);
        if let Some(workspace) = output.workspaces.get(output.active) {
            self.disconnected
                .retain(|(remembered, _)| *remembered != identity);
            self.disconnected.push((identity, workspace.identity));
        }
        if let Some(destination) = self.outputs.first_mut() {
            destination.workspaces.extend(output.workspaces);
            if self.focused == Some(identity) {
                self.focused = Some(destination.identity);
            }
        } else {
            self.detached.extend(output.workspaces);
            self.focused = None;
        }
        self.normalize();
        Ok(())
    }
    /// # Errors
    /// Returns an error for duplicate names or when there is no output.
    pub fn add_named_workspace(
        &mut self,
        name: String,
        output: Option<OutputId>,
    ) -> Result<WorkspaceId, LayoutError> {
        if self
            .outputs
            .iter()
            .flat_map(|candidate| &candidate.workspaces)
            .chain(&self.detached)
            .any(|workspace| workspace.name.as_ref() == Some(&name))
        {
            return Err(LayoutError::DuplicateWorkspaceName);
        }
        let index = output
            .or(self.focused)
            .and_then(|identity| {
                self.outputs
                    .iter()
                    .position(|candidate| candidate.identity == identity)
            })
            .ok_or(LayoutError::NoOutput)?;
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let mut workspace = self.workspace(self.outputs[index].identity);
        let identity = workspace.identity;
        workspace.name = Some(name);
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let position = self.outputs[index].workspaces.len().saturating_sub(1);
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        self.outputs[index].workspaces.insert(position, workspace);
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        {
            if self.outputs[index].active >= position {
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                {
                    self.outputs[index].active = self.outputs[index].active.saturating_add(1);
                }
            }
        }
        self.normalize();
        Ok(identity)
    }
    /// # Errors
    /// Returns an error for a duplicate window or when there is no output.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Opening rules are passed as an owned public command payload."
    )]
    pub fn add_window(&mut self, identity: WindowId, rules: WindowRules) -> Result<(), LayoutError> {
        validate_rules(&rules)?;
        if self.find_window(identity).is_some()
            || self
                .detached
                .iter()
                .flat_map(|workspace| &workspace.columns)
                .flat_map(|column| &column.tiles)
                .any(|tile| tile.window == identity)
        {
            return Err(LayoutError::DuplicateWindow(identity));
        }
        let (output, workspace, focus) = self.opening_target(&rules)?;
        let position = self.insert_window(identity, &rules, output, workspace);
        if focus {
            self.focus_window_for_open(output, workspace, position);
        }
        self.normalize();
        if focus {
            self.scroll(output, None, false);
        }
        Ok(())
    }
    fn opening_target(&self, rules: &WindowRules) -> Result<(usize, usize, bool), LayoutError> {
        let fallback = self.focused_index().ok_or(LayoutError::NoOutput)?;
        let named = rules.open_on_workspace.as_ref().and_then(|name| {
            self.outputs.iter().enumerate().find_map(|(output, candidate)| {
                candidate
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.name.as_ref() == Some(name))
                    .map(|workspace| (output, workspace))
            })
        });
        let output = named.map_or_else(
            || {
                rules
                    .open_on_output
                    .and_then(|target| {
                        self.outputs
                            .iter()
                            .position(|candidate| candidate.identity == target)
                    })
                    .unwrap_or(fallback)
            },
            |(output, _)| output,
        );
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let workspace = named.map_or(self.outputs[output].active, |(_, workspace)| workspace);
        Ok((
            output,
            workspace,
            rules.open_focused.unwrap_or(output == fallback),
        ))
    }
    fn insert_window(
        &mut self,
        identity: WindowId,
        rules: &WindowRules,
        output: usize,
        workspace: usize,
    ) -> usize {
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let config = self.outputs[output].config.clone();
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let previous = self.outputs[output].workspaces[workspace].focused_window();
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let view = self.outputs[output].workspaces[workspace].view;
        let mode = if rules.open_fullscreen {
            TileMode::Fullscreen
        } else if rules.open_maximized_to_edges {
            TileMode::MaximizedToEdges
        } else {
            TileMode::Normal
        };
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let anchor = self.outputs[output].column_x(
            workspace,
            self.outputs[output].workspaces[workspace].focused.unwrap_or(0),
        );
        let tile = Tile {
            window: identity,
            height: rules
                .default_window_height
                .map_or(WindowHeight::Auto, window_height),
            mode,
            min_height: rules.min_height,
            max_height: rules.max_height,
            opened_view: Some((view, previous, anchor)),
            fullscreen_view: None,
        };
        let width = if rules.open_maximized {
            Some(PresetSize::Proportion(1.0_f64))
        } else {
            rules.default_column_width.unwrap_or(config.default_column_width)
        };
        let column = Column {
            width,
            client_width: 0.0_f64,
            display: rules
                .default_column_display
                .unwrap_or(config.default_column_display),
            tiles: vec![tile],
            focused: 0,
            resized: rules.default_window_height.map(|_| identity),
        };
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let output_id = self.outputs[output].identity;
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let target = &mut self.outputs[output].workspaces[workspace];
        target.original_output = Some(output_id);
        let position = target
            .focused
            .map_or(target.columns.len(), |index| index.saturating_add(1));
        target.columns.insert(position, column);
        position
    }
    fn focus_window_for_open(&mut self, output: usize, workspace: usize, position: usize) {
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let target = &mut self.outputs[output];
        target.active = workspace;
        self.focused = Some(target.identity);
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let workspace = &mut target.workspaces[workspace];
        workspace.focused = Some(position);
    }
    /// # Errors
    /// Returns an error if the window does not exist.
    pub fn remove_window(&mut self, identity: WindowId) -> Result<(), LayoutError> {
        if let Some(workspace) = self.detached.iter_mut().find(|workspace| {
            workspace
                .columns
                .iter()
                .any(|column| column.tiles.iter().any(|tile| tile.window == identity))
        }) {
            workspace.remove_detached_window(identity);
            return Ok(());
        }
        let (output, workspace, column, tile) = self
            .find_window(identity)
            .ok_or(LayoutError::UnknownWindow(identity))?;
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let before = self.outputs[output].workspaces[workspace].focused_window();
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let old_x = self.outputs[output].column_x(
            workspace,
            self.outputs[output].workspaces[workspace].focused.unwrap_or(0),
        );
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let target = &mut self.outputs[output].workspaces[workspace];
        let removed = target.remove_tile(column, tile, identity);
        let restored_view = target.restorable_view(&removed, before);
        if let Some((view, previous, _)) = restored_view {
            target.view = view;
            target.restore_focus(previous);
        } else if before != Some(identity) {
            #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
            let new_x = self.outputs[output].column_x(
                workspace,
                self.outputs[output].workspaces[workspace].focused.unwrap_or(0),
            );
            #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
            self.outputs[output].workspaces[workspace]
                .view
                .add_assign(new_x.sub(old_x));
        }
        {
            if let Some((view, _, anchor)) = restored_view {
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                let current = self.outputs[output].column_x(
                    workspace,
                    self.outputs[output].workspaces[workspace].focused.unwrap_or(0),
                );
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                {
                    self.outputs[output].workspaces[workspace].view = view.add(current.sub(anchor));
                }
            }
        }
        self.normalize();
        if self.must_center(output) {
            self.scroll(output, None, false);
        }
        Ok(())
    }
    /// Updates logical output geometry after a mode or scale change.
    /// # Errors
    /// Returns an error for an unknown output or invalid geometry.
    pub fn update_output(&mut self, identity: OutputId, geometry: OutputGeometry) -> Result<(), LayoutError> {
        let output = self
            .outputs
            .iter_mut()
            .find(|output| output.identity == identity)
            .ok_or(LayoutError::UnknownOutput(identity))?;
        validate_output(geometry, &output.config)?;
        output.geometry = geometry;
        Ok(())
    }

    /// Records the committed client width of a column whose initial configure has width zero.
    /// # Errors
    /// Returns an error for an unknown window or invalid width.
    pub fn report_client_width(&mut self, identity: WindowId, width: f64) -> Result<(), LayoutError> {
        if !positive(width) {
            return Err(LayoutError::InvalidSize);
        }
        let (output, workspace, column, _) = self
            .find_window(identity)
            .ok_or(LayoutError::UnknownWindow(identity))?;
        let column = self
            .outputs
            .get_mut(output)
            .and_then(|output| output.workspaces.get_mut(workspace))
            .and_then(|workspace| workspace.columns.get_mut(column))
            .ok_or(LayoutError::UnknownWindow(identity))?;
        if column.width.is_none() {
            column.client_width = width;
        }
        self.scroll(output, None, false);
        Ok(())
    }

    /// Focuses a connected window and its workspace and output.
    /// # Errors
    /// Returns an error if the window is not on a connected output.
    pub fn focus_window(&mut self, identity: WindowId) -> Result<(), LayoutError> {
        let (output, workspace, column, tile) = self
            .find_window(identity)
            .ok_or(LayoutError::UnknownWindow(identity))?;
        if let Some(target) = self.outputs.get_mut(output) {
            self.focused = Some(target.identity);
            target.active = workspace;
            for candidate in &mut target.workspaces {
                for entry in candidate.columns.iter_mut().flat_map(|stack| &mut stack.tiles) {
                    entry.opened_view = None;
                    entry.fullscreen_view = None;
                }
            }
            if let Some(workspace) = target.workspaces.get_mut(workspace) {
                workspace.focused = Some(column);
                if let Some(column) = workspace.columns.get_mut(column) {
                    column.focused = tile;
                }
            }
        }
        self.normalize();
        self.scroll(output, None, false);
        Ok(())
    }

    fn focused_index(&self) -> Option<usize> {
        self.outputs
            .iter()
            .position(|output| Some(output.identity) == self.focused)
    }
    fn find_window(&self, identity: WindowId) -> Option<(usize, usize, usize, usize)> {
        self.outputs.iter().enumerate().find_map(|(output, candidate)| {
            candidate
                .workspaces
                .iter()
                .enumerate()
                .find_map(|(workspace, candidate)| {
                    candidate
                        .columns
                        .iter()
                        .enumerate()
                        .find_map(|(column, candidate)| {
                            candidate
                                .tiles
                                .iter()
                                .position(|tile| tile.window == identity)
                                .map(|tile| (output, workspace, column, tile))
                        })
                })
        })
    }
    fn normalize(&mut self) {
        for output in &mut self.outputs {
            let active = output
                .workspaces
                .get(output.active)
                .map(|workspace| workspace.identity);
            let above = output.config.empty_workspace_above_first;
            let last = output.workspaces.len().saturating_sub(1);
            output.workspaces = output
                .workspaces
                .drain(..)
                .enumerate()
                .filter_map(|(position, workspace)| {
                    (!workspace.empty_unnamed()
                        || Some(workspace.identity) == active
                        || position == last
                        || above && position == 0)
                        .then_some(workspace)
                })
                .collect();
            if output
                .workspaces
                .last()
                .is_none_or(|workspace| !workspace.empty_unnamed())
            {
                output
                    .workspaces
                    .push(Workspace::empty(&mut self.next_workspace, output.identity));
            }
            if above
                && (output.workspaces.len() == 1
                    || output
                        .workspaces
                        .first()
                        .is_none_or(|workspace| !workspace.empty_unnamed()))
            {
                output
                    .workspaces
                    .insert(0, Workspace::empty(&mut self.next_workspace, output.identity));
            }
            output.active = active
                .and_then(|identity| {
                    output
                        .workspaces
                        .iter()
                        .position(|workspace| workspace.identity == identity)
                })
                .unwrap_or(0);
        }
    }
    fn focus_workspace(&mut self, output: usize, mut target: usize, back: bool) {
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let current = self.outputs[output].active;
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        if target >= self.outputs[output].workspaces.len() {
            return;
        }
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        {
            if target == current && back && self.outputs[output].config.workspace_auto_back_and_forth {
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                {
                    target = self.outputs[output]
                        .previous
                        .and_then(|identity| {
                            self.outputs[output]
                                .workspaces
                                .iter()
                                .position(|workspace| workspace.identity == identity)
                        })
                        .unwrap_or(current);
                }
            }
        }
        {
            if target != current {
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                {
                    self.outputs[output].previous = Some(self.outputs[output].workspaces[current].identity);
                }
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                {
                    self.outputs[output].active = target;
                }
            }
        }
    }
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "Commands targeting other layout levels leave this level unchanged."
    )]
    fn monitor(&self, output: usize, direction: Direction) -> Option<usize> {
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let area = self.outputs[output].geometry.area;
        self.outputs
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != output)
            .filter_map(|(index, candidate)| {
                let other = candidate.geometry.area;
                let distance = match direction {
                    Direction::Left if other.x < area.x => area.x.sub(other.x),
                    Direction::Right if other.x > area.x => other.x.sub(area.x),
                    Direction::Upward if other.y < area.y => area.y.sub(other.y),
                    Direction::Downward if other.y > area.y => other.y.sub(area.y),
                    _ => return None,
                };
                Some((index, distance))
            })
            .min_by(|(_, left), (_, right)| left.total_cmp(right))
            .map(|(index, _)| index)
    }
    fn move_column(&mut self, output: usize, destination: usize, workspace: usize) {
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let source = self.outputs[output].active;
        if output == destination && source == workspace {
            return;
        }
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let Some(column) = self.outputs[output].workspaces[source].focused else {
            return;
        };
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let moved = self.outputs[output].workspaces[source].columns.remove(column);
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let source_workspace = &mut self.outputs[output].workspaces[source];
        source_workspace.focused = (!source_workspace.columns.is_empty())
            .then(|| column.min(source_workspace.columns.len().saturating_sub(1)));
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let target = &mut self.outputs[destination].workspaces[workspace];
        let position = target
            .focused
            .map_or(target.columns.len(), |focused| focused.saturating_add(1));
        target.columns.insert(position, moved);
        target.focused = Some(position);
    }
    #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
    fn must_center(&self, output: usize) -> bool {
        let output = &self.outputs[output];
        output.config.center_focused_column == CenterFocusedColumn::Always
            || output.config.always_center_single_column
                && output.workspaces[output.active].columns.len() == 1
    }
    fn scroll(&mut self, output: usize, previous: Option<usize>, center: bool) {
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let active = self.outputs[output].active;
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let Some(column) = self.outputs[output].workspaces[active].focused else {
            return;
        };
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let config = self.outputs[output].config.clone();
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let working = self.outputs[output].working();
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let width =
            self.outputs[output].column_width(&self.outputs[output].workspaces[active].columns[column]);
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let position = self.outputs[output].column_x(active, column);
        let overflow = previous.is_some_and(|previous| {
            #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
            let other =
                self.outputs[output].column_width(&self.outputs[output].workspaces[active].columns[previous]);
            previous != column && other.add(width).add(3.0_f64.mul(config.gaps)) > working.width.add(1e-7)
        });
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let centered = center
            || config.center_focused_column == CenterFocusedColumn::Always
            || config.center_focused_column == CenterFocusedColumn::OnOverflow && overflow
            || config.always_center_single_column
                && self.outputs[output].workspaces[active].columns.len() == 1;
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let fullscreen = {
            let focused = &self.outputs[output].workspaces[active].columns[column];
            focused.tiles[focused.focused].mode == TileMode::Fullscreen
        };
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let view = &mut self.outputs[output].workspaces[active].view;
        if fullscreen {
            // The tile starts at the output edge, so neighbours are fully off screen.
            *view = working.x.add(position);
        } else if width > working.width.sub(2.0_f64.mul(config.gaps)) {
            *view = position.sub(config.gaps);
        } else if centered {
            *view = position.add(width.div(2.0_f64)).sub(working.width.div(2.0_f64));
        } else if position.sub(*view) < config.gaps {
            *view = position.sub(config.gaps);
        } else if position.add(width).sub(*view) > working.width.sub(config.gaps) {
            *view = position.add(width).sub(working.width).add(config.gaps);
        }
    }
    /// A command without a focused target leaves the layout unchanged.
    pub fn apply(&mut self, command: Command) {
        if let Command::SetColumnWidth(change) | Command::SetWindowHeight(change) = command
            && !valid_change(change)
        {
            return;
        }
        let Some(output) = self.focused_index() else {
            return;
        };
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let active = self.outputs[output].active;
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let previous = self.outputs[output].workspaces[active].focused;
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let before = self.outputs[output].workspaces[active].focused_window();
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let restoring_fullscreen = command == Command::ToggleFullscreen
            && previous.is_some_and(|index| {
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                let column = &self.outputs[output].workspaces[active].columns[index];
                column.tiles[column.focused].mode == TileMode::Fullscreen
                    && column.tiles[column.focused].fullscreen_view.is_some()
            });
        self.apply_workspace_command(output, active, command);
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let after = self.outputs[output]
            .workspaces
            .get(self.outputs[output].active)
            .and_then(Workspace::focused_window);
        if after != before {
            #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
            for workspace in &mut self.outputs[output].workspaces {
                for tile in workspace.columns.iter_mut().flat_map(|column| &mut column.tiles) {
                    tile.opened_view = None;
                    tile.fullscreen_view = None;
                }
            }
        }
        self.normalize();
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        if (!restoring_fullscreen || self.must_center(output))
            && (command != Command::ExpandColumnToAvailableWidth
                || self.outputs[output].config.center_focused_column == CenterFocusedColumn::Always)
            && let Some(current) = self.focused_index()
        {
            #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
            let prior = if current == output && self.outputs[current].active == active {
                previous.filter(|column| *column < self.outputs[current].workspaces[active].columns.len())
            } else {
                None
            };
            self.scroll(current, prior, command == Command::CenterColumn);
        }
    }
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "Commands are dispatched by layout level."
    )]
    fn apply_workspace_command(&mut self, output: usize, active: usize, command: Command) {
        match command {
            Command::FocusWorkspaceDown => {
                self.focus_workspace(output, active.saturating_add(1), false);
            }
            Command::FocusWorkspaceUp => {
                self.focus_workspace(output, active.saturating_sub(1), false);
            }
            Command::FocusWorkspaceAt(index) => self.focus_workspace(output, index, true),
            Command::MoveColumnToWorkspaceDown => {
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                if active.saturating_add(1) < self.outputs[output].workspaces.len() {
                    self.move_column(output, output, active.saturating_add(1));
                }
            }
            Command::MoveColumnToWorkspaceUp => {
                if active > 0 {
                    self.move_column(output, output, active.saturating_sub(1));
                }
            }
            Command::MoveColumnToWorkspaceAt(index) =>
            {
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                if index < self.outputs[output].workspaces.len() {
                    self.move_column(output, output, index);
                }
            }
            _ => self.apply_workspace_movement(output, active, command),
        }
    }
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "Commands are dispatched by layout level."
    )]
    fn apply_workspace_movement(&mut self, output: usize, active: usize, command: Command) {
        match command {
            Command::MoveWorkspaceDown | Command::MoveWorkspaceUp => {
                let target = if command == Command::MoveWorkspaceDown {
                    active.saturating_add(1)
                } else {
                    active.saturating_sub(1)
                };
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                {
                    if target < self.outputs[output].workspaces.len()
                        && !self.outputs[output].workspaces[target].empty_unnamed()
                    {
                        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                        self.outputs[output].workspaces.swap(active, target);
                        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                        {
                            self.outputs[output].active = target;
                        }
                    }
                }
            }
            Command::FocusMonitor(direction) => {
                if let Some(target) = self.monitor(output, direction) {
                    #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                    {
                        self.focused = Some(self.outputs[target].identity);
                    }
                }
            }
            Command::MoveColumnToMonitor(direction) => {
                if let Some(target) = self.monitor(output, direction) {
                    #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                    self.move_column(output, target, self.outputs[target].active);
                }
            }
            _ => self.apply_monitor_movement(output, active, command),
        }
    }
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "Commands are dispatched by layout level."
    )]
    fn apply_monitor_movement(&mut self, output: usize, active: usize, command: Command) {
        match command {
            Command::MoveWorkspaceToMonitor(direction) => {
                if let Some(target) = self.monitor(output, direction) {
                    #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                    let moved = self.outputs[output].workspaces.remove(active);
                    #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                    {
                        self.outputs[output].active =
                            active.min(self.outputs[output].workspaces.len().saturating_sub(1));
                    }
                    #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                    let position = self.outputs[target].workspaces.len().saturating_sub(1);
                    #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                    self.outputs[target].workspaces.insert(position, moved);
                    #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                    {
                        if self.outputs[target].active >= position {
                            #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                            {
                                self.outputs[target].active = self.outputs[target].active.saturating_add(1);
                            }
                        }
                    }
                }
            }
            _ => self.apply_column(output, command),
        }
    }

    fn apply_column(&mut self, output: usize, command: Command) {
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let active = self.outputs[output].active;
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let Some(index) = self.outputs[output].workspaces[active].focused else {
            return;
        };
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let config = self.outputs[output].config.clone();
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let working = self.outputs[output].working();
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let height = self.outputs[output].geometry.area.height;
        if matches!(command, Command::ExpandColumnToAvailableWidth) {
            #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
            let workspace = &self.outputs[output].workspaces[active];
            let mut widths = 0.0_f64;
            let mut count = 0.0_f64;
            let mut first = None;
            for (position, column) in workspace.columns.iter().enumerate() {
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                let width = self.outputs[output].column_width(column);
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                let horizontal = self.outputs[output]
                    .column_x(active, position)
                    .sub(workspace.view);
                if horizontal >= 0.0_f64 && horizontal.add(width) <= working.width {
                    first = first.or(Some(position));
                    count.add_assign(1.0_f64);
                    if position != index {
                        widths.add_assign(width);
                    }
                }
            }
            {
                if let Some(first) = first {
                    let width = working.width.sub(count.add(1.0_f64).mul(config.gaps)).sub(widths);
                    #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                    {
                        self.outputs[output].workspaces[active].columns[index].width =
                            Some(PresetSize::Fixed(width.sub(2.0_f64.mul(config.border))));
                    }
                    #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                    let start = self.outputs[output].column_x(active, first);
                    #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                    {
                        self.outputs[output].workspaces[active].view = start.sub(config.gaps);
                    }
                }
            }
            return;
        }
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let workspace = &mut self.outputs[output].workspaces[active];
        workspace.apply_column_command(index, command, &config, working, height);
    }
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let focused_window = self
            .focused_index()
            .and_then(|index| self.outputs[index].workspaces.get(self.outputs[index].active))
            .and_then(Workspace::focused_window);
        Snapshot {
            outputs: self.outputs.iter().map(Output::snapshot).collect(),
            focused_output: self.focused,
            focused_window,
        }
    }
}

impl Workspace {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "Commands are dispatched by layout level."
    )]
    fn apply_column_command(
        &mut self,
        index: usize,
        command: Command,
        config: &LayoutConfig,
        working: Rect,
        height: f64,
    ) {
        let last = self.columns.len().saturating_sub(1);
        match command {
            Command::FocusColumnLeft => self.focused = Some(index.saturating_sub(1)),
            Command::FocusColumnRight => {
                self.focused = Some(index.saturating_add(1).min(last));
            }
            Command::FocusColumnFirst => self.focused = Some(0),
            Command::FocusColumnLast => self.focused = Some(last),
            Command::MoveColumnLeft | Command::MoveColumnRight => {
                let target = if command == Command::MoveColumnLeft {
                    index.saturating_sub(1)
                } else {
                    index.saturating_add(1).min(last)
                };
                self.columns.swap(index, target);
                self.focused = Some(target);
            }
            _ => self.consume_column_window(index, command, config, working, height),
        }
    }
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "Commands are dispatched by layout level."
    )]
    fn consume_column_window(
        &mut self,
        index: usize,
        command: Command,
        config: &LayoutConfig,
        working: Rect,
        height: f64,
    ) {
        let last = self.columns.len().saturating_sub(1);
        match command {
            Command::ConsumeWindowIntoColumn => {
                if index < last {
                    #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                    let tile = self.columns[index.saturating_add(1)].tiles.remove(0);
                    #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                    self.columns[index].tiles.push(tile);
                    #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                    {
                        self.columns[index].resized = None;
                    }
                    #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                    {
                        if self.columns[index.saturating_add(1)].tiles.is_empty() {
                            drop(self.columns.remove(index.saturating_add(1)));
                        } else {
                            #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                            {
                                self.columns[index.saturating_add(1)].focused =
                                    self.columns[index.saturating_add(1)].focused.saturating_sub(1);
                            }
                        }
                    }
                }
            }
            _ => self.expel_or_consume_window(index, command, config, working, height),
        }
    }
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "Commands are dispatched by layout level."
    )]
    fn expel_or_consume_window(
        &mut self,
        index: usize,
        command: Command,
        config: &LayoutConfig,
        working: Rect,
        height: f64,
    ) {
        let last = self.columns.len().saturating_sub(1);
        match command {
            Command::ExpelWindowFromColumn
            | Command::ConsumeOrExpelWindowLeft
            | Command::ConsumeOrExpelWindowRight => {
                let left = command == Command::ConsumeOrExpelWindowLeft;
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                {
                    if self.columns[index].tiles.len() > 1 {
                        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                        let stack = &mut self.columns[index];
                        let tile = stack.tiles.remove(stack.focused);
                        stack.focused = stack.focused.min(stack.tiles.len().saturating_sub(1));
                        stack.resized = None;
                        let column = Column {
                            client_width: 0.0_f64,
                            width: stack.width,
                            display: config.default_column_display,
                            tiles: vec![tile],
                            focused: 0,
                            resized: None,
                        };
                        let position = if left { index } else { index.saturating_add(1) };
                        self.columns.insert(position, column);
                        self.focused = Some(position);
                    } else if command != Command::ExpelWindowFromColumn
                        && (left && index > 0 || !left && index < last)
                    {
                        let mut column = self.columns.remove(index);
                        let target = if left { index.saturating_sub(1) } else { index };
                        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                        {
                            self.columns[target].focused = self.columns[target].tiles.len();
                        }
                        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                        self.columns[target].tiles.append(&mut column.tiles);
                        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                        {
                            self.columns[target].resized = None;
                        }
                        self.focused = Some(target);
                    }
                }
            }
            _ => self.apply_window_target(index, command, config, working, height),
        }
    }
    fn apply_window_target(
        &mut self,
        index: usize,
        command: Command,
        config: &LayoutConfig,
        working: Rect,
        height: f64,
    ) {
        self.apply_window_command(index, command, config, working, height);
    }

    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "Commands are dispatched by layout level."
    )]
    fn apply_window_command(
        &mut self,
        index: usize,
        command: Command,
        config: &LayoutConfig,
        working: Rect,
        height: f64,
    ) {
        let Some(column) = self.columns.get_mut(index) else {
            return;
        };
        let tile_index = column.focused;
        let last_tile = column.tiles.len().saturating_sub(1);
        match command {
            Command::FocusWindowUp => {
                column.focused = tile_index.saturating_sub(1);
            }
            Command::FocusWindowDown => {
                column.focused = tile_index.saturating_add(1).min(last_tile);
            }
            Command::FocusWindowTop => column.focused = 0,
            Command::FocusWindowBottom => column.focused = last_tile,
            Command::FocusWindowDownOrTop => {
                column.focused = if tile_index == last_tile {
                    0
                } else {
                    tile_index.saturating_add(1)
                };
            }
            Command::FocusWindowUpOrBottom => {
                column.focused = if tile_index == 0 {
                    last_tile
                } else {
                    tile_index.saturating_sub(1)
                };
            }
            Command::FocusWindowInColumn(position) => {
                if position > 0 && position <= column.tiles.len() {
                    column.focused = position.saturating_sub(1);
                }
            }
            _ => self.apply_window_movement(index, command, config, working, height),
        }
    }
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "Commands are dispatched by layout level."
    )]
    fn apply_window_movement(
        &mut self,
        index: usize,
        command: Command,
        config: &LayoutConfig,
        working: Rect,
        height: f64,
    ) {
        let Some(column) = self.columns.get_mut(index) else {
            return;
        };
        let tile_index = column.focused;
        let last_tile = column.tiles.len().saturating_sub(1);
        match command {
            Command::FocusWindowUpOrColumnRight => {
                if tile_index > 0 {
                    column.focused = column.focused.saturating_sub(1);
                } else {
                    self.focused = Some(index.saturating_add(1).min(self.columns.len().saturating_sub(1)));
                }
            }
            Command::MoveWindowUp | Command::MoveWindowDown => {
                let target = if command == Command::MoveWindowUp {
                    tile_index.saturating_sub(1)
                } else {
                    tile_index.saturating_add(1).min(last_tile)
                };
                column.tiles.swap(tile_index, target);
                column.focused = target;
            }
            Command::ToggleColumnTabbedDisplay => {
                column.display = if column.display == ColumnDisplay::Normal {
                    ColumnDisplay::Tabbed
                } else {
                    ColumnDisplay::Normal
                };
            }
            _ => self.apply_column_size(index, command, config, working, height),
        }
    }
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "Commands are dispatched by layout level."
    )]
    fn apply_column_size(
        &mut self,
        index: usize,
        command: Command,
        config: &LayoutConfig,
        working: Rect,
        height: f64,
    ) {
        let Some(column) = self.columns.get_mut(index) else {
            return;
        };
        match command {
            Command::SetColumnWidth(change) => {
                column.width = Some(change_size(
                    column.width,
                    change,
                    working.width,
                    config.gaps,
                    config.border,
                ));
            }
            Command::MaximizeColumn => {
                column.width = Some(PresetSize::Proportion(1.0_f64));
            }
            Command::SwitchPresetColumnWidth | Command::SwitchPresetColumnWidthBack => {
                column.width = cycle(
                    column.width,
                    &config.preset_column_widths,
                    command == Command::SwitchPresetColumnWidthBack,
                );
            }
            _ => self.apply_window_height(index, command, config, working, height),
        }
    }
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "Commands are dispatched by layout level."
    )]
    fn apply_window_height(
        &mut self,
        index: usize,
        command: Command,
        config: &LayoutConfig,
        working: Rect,
        height: f64,
    ) {
        let Some(column) = self.columns.get_mut(index) else {
            return;
        };
        let tile_index = column.focused;
        match command {
            Command::SetWindowHeight(change) => {
                let heights = column.heights(working.height, height, config);
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                let current = match column.tiles[tile_index].height {
                    WindowHeight::Auto => Some(PresetSize::Fixed(
                        heights[tile_index].sub(2.0_f64.mul(config.border)),
                    )),
                    WindowHeight::Proportion(value) => Some(PresetSize::Proportion(value)),
                    WindowHeight::Fixed(value) => Some(PresetSize::Fixed(value)),
                };
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                {
                    column.tiles[tile_index].height = window_height(change_size(
                        current,
                        change,
                        working.height,
                        config.gaps,
                        config.border,
                    ));
                }
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                {
                    column.resized = Some(column.tiles[tile_index].window);
                }
            }
            Command::ResetWindowHeight => {
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                {
                    column.tiles[tile_index].height = WindowHeight::Auto;
                }
                column.resized = None;
            }
            _ => self.apply_height_preset(index, command, config, working, height),
        }
    }
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "Commands are dispatched by layout level."
    )]
    fn apply_height_preset(
        &mut self,
        index: usize,
        command: Command,
        config: &LayoutConfig,
        working: Rect,
        height: f64,
    ) {
        let Some(column) = self.columns.get_mut(index) else {
            return;
        };
        let tile_index = column.focused;
        match command {
            Command::SwitchPresetWindowHeight | Command::SwitchPresetWindowHeightBack => {
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                let current = match column.tiles[tile_index].height {
                    WindowHeight::Auto => None,
                    WindowHeight::Proportion(value) => Some(PresetSize::Proportion(value)),
                    WindowHeight::Fixed(value) => Some(PresetSize::Fixed(value)),
                };
                {
                    if let Some(size) = cycle(
                        current,
                        &config.preset_window_heights,
                        command == Command::SwitchPresetWindowHeightBack,
                    ) {
                        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                        {
                            column.tiles[tile_index].height = window_height(size);
                        }
                        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                        {
                            column.resized = Some(column.tiles[tile_index].window);
                        }
                    }
                }
            }
            _ => self.apply_tile_mode(index, command, config, working, height),
        }
    }
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "Commands are dispatched by layout level."
    )]
    fn apply_tile_mode(
        &mut self,
        index: usize,
        command: Command,
        _config: &LayoutConfig,
        _working: Rect,
        _height: f64,
    ) {
        let Some(column) = self.columns.get_mut(index) else {
            return;
        };
        let tile_index = column.focused;
        match command {
            Command::ToggleFullscreen
            | Command::ToggleWindowedFullscreen
            | Command::ToggleMaximizeToEdges => {
                #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
                let tile = &mut column.tiles[tile_index];
                let mode = match command {
                    Command::ToggleFullscreen => TileMode::Fullscreen,
                    Command::ToggleWindowedFullscreen => TileMode::WindowedFullscreen,
                    _ => TileMode::MaximizedToEdges,
                };
                if tile.mode == mode {
                    tile.mode = TileMode::Normal;
                    if let Some((view, _)) = tile.fullscreen_view.take() {
                        self.view = view;
                    }
                } else {
                    tile.fullscreen_view = Some((self.view, tile.window));
                    tile.mode = mode;
                }
            }
            _ => {}
        }
    }

    const fn empty(next_workspace: &mut u64, output: OutputId) -> Self {
        let identity = WorkspaceId(*next_workspace);
        *next_workspace = next_workspace.wrapping_add(1);
        Self {
            identity,
            name: None,
            original_output: Some(output),
            columns: Vec::new(),
            focused: None,
            view: 0.0_f64,
        }
    }
    fn remove_detached_window(&mut self, identity: WindowId) {
        let position = self.columns.iter().enumerate().find_map(|(column, stack)| {
            stack
                .tiles
                .iter()
                .position(|tile| tile.window == identity)
                .map(|tile| (column, tile))
        });
        if let Some((column, tile)) = position {
            let _removed = self.remove_tile(column, tile, identity);
        }
    }
    fn remove_tile(&mut self, column: usize, tile: usize, identity: WindowId) -> Tile {
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let stack = &mut self.columns[column];
        let removed = stack.tiles.remove(tile);
        stack.focused = stack.focused.min(stack.tiles.len().saturating_sub(1));
        if stack.resized == Some(identity) {
            stack.resized = None;
        }
        if stack.tiles.is_empty() {
            drop(self.columns.remove(column));
            self.focused = self.focused.and_then(|focused| {
                if self.columns.is_empty() {
                    None
                } else {
                    Some(if focused > column {
                        focused.saturating_sub(1)
                    } else {
                        focused.min(self.columns.len().saturating_sub(1))
                    })
                }
            });
        }
        removed
    }
    fn restore_focus(&mut self, previous: Option<WindowId>) {
        if let Some(previous) = previous
            && let Some(position) = self
                .columns
                .iter()
                .position(|column| column.tiles.iter().any(|tile| tile.window == previous))
        {
            self.focused = Some(position);
        }
    }
    fn restorable_view(
        &self,
        removed: &Tile,
        before: Option<WindowId>,
    ) -> Option<(f64, Option<WindowId>, f64)> {
        removed.opened_view.filter(|(_, previous, _)| {
            (before == Some(removed.window) || before == *previous)
                && previous.map_or(self.columns.is_empty(), |window| {
                    self.columns
                        .iter()
                        .any(|column| column.tiles.iter().any(|tile| tile.window == window))
                })
        })
    }
    fn focused_window(&self) -> Option<WindowId> {
        self.focused
            .and_then(|index| self.columns.get(index))
            .and_then(|column| column.tiles.get(column.focused))
            .map(|tile| tile.window)
    }
    const fn empty_unnamed(&self) -> bool {
        self.name.is_none() && self.columns.is_empty()
    }
}

const fn window_height(size: PresetSize) -> WindowHeight {
    match size {
        PresetSize::Proportion(value) => WindowHeight::Proportion(value),
        PresetSize::Fixed(value) => WindowHeight::Fixed(value),
    }
}

fn resolve(size: PresetSize, area: f64, gaps: f64, border: f64) -> f64 {
    match size {
        PresetSize::Proportion(value) => value.mul(area.sub(gaps)).sub(gaps),
        PresetSize::Fixed(value) => value.add(2.0_f64.mul(border)),
    }
}

fn change_size(
    current: Option<PresetSize>,
    change: SizeChange,
    area: f64,
    gaps: f64,
    border: f64,
) -> PresetSize {
    match change {
        SizeChange::SetProportion(value) => PresetSize::Proportion(value),
        SizeChange::SetFixed(value) => PresetSize::Fixed(value),
        SizeChange::AdjustPercent(value) => {
            let proportion = match current {
                Some(PresetSize::Proportion(proportion)) => proportion,
                Some(size) => resolve(size, area, gaps, border).add(gaps).div(area.sub(gaps)),
                None => 0.5_f64,
            };
            PresetSize::Proportion(proportion.add(value.div(100.0_f64)).max(0.01_f64))
        }
        SizeChange::AdjustFixed(value) => PresetSize::Fixed(
            current
                .map_or_else(|| area.div(2.0_f64), |size| resolve(size, area, gaps, border))
                .sub(2.0_f64.mul(border))
                .add(value)
                .max(1.0_f64),
        ),
    }
}

fn cycle(current: Option<PresetSize>, sizes: &[PresetSize], back: bool) -> Option<PresetSize> {
    if sizes.is_empty() {
        return current;
    }
    let next = current
        .and_then(|current| sizes.iter().position(|size| *size == current))
        .map_or(0, |index| {
            if back {
                if index == 0 {
                    sizes.len().saturating_sub(1)
                } else {
                    index.saturating_sub(1)
                }
            } else {
                index.saturating_add(1).checked_rem(sizes.len()).unwrap_or(0)
            }
        });
    sizes.get(next).copied()
}

impl Tile {
    fn snapshot(
        &self,
        normal: Rect,
        geometry: OutputGeometry,
        border_width: f64,
        client_chooses_width: bool,
    ) -> TileView {
        let tile_rect = match self.mode {
            TileMode::Normal | TileMode::WindowedFullscreen => normal,
            TileMode::Fullscreen => Rect {
                x: normal.x,
                y: 0.0_f64,
                width: geometry.area.width,
                height: geometry.area.height,
            },
            TileMode::MaximizedToEdges => Rect {
                x: geometry.usable.x.sub(geometry.area.x),
                y: geometry.usable.y.sub(geometry.area.y),
                width: geometry.usable.width,
                height: geometry.usable.height,
            },
        };
        let border = if matches!(self.mode, TileMode::Fullscreen | TileMode::MaximizedToEdges) {
            0.0_f64
        } else {
            border_width
        };
        TileView {
            window: self.window,
            tile: tile_rect,
            requested_client: Size {
                width: if client_chooses_width && self.mode == TileMode::Normal {
                    0.0_f64
                } else {
                    tile_rect.width.sub(2.0_f64.mul(border))
                },
                height: tile_rect.height.sub(2.0_f64.mul(border)),
            },
            fullscreen_requested: matches!(self.mode, TileMode::Fullscreen | TileMode::WindowedFullscreen),
            mode: self.mode,
            height: self.height,
        }
    }
}

impl Column {
    fn heights(&self, area: f64, output_height: f64, config: &LayoutConfig) -> Vec<f64> {
        let available = area.sub(2.0_f64.mul(config.gaps)).max(2.0_f64.mul(config.border));
        if self.display == ColumnDisplay::Tabbed {
            return vec![available; self.tiles.len()];
        }
        let count = f64::from(u32::try_from(self.tiles.len()).unwrap_or(u32::MAX));
        let space = available
            .sub(count.sub(1.0_f64).mul(config.gaps))
            .max(count.mul(2.0_f64).mul(config.border));
        let resized = self
            .resized
            .and_then(|identity| self.tiles.iter().position(|tile| tile.window == identity));
        #[expect(clippy::indexing_slicing, reason = "Layout-owned index.")]
        let requested = resized
            .map(|index| match self.tiles[index].height {
                WindowHeight::Auto => space.div(count),
                WindowHeight::Proportion(value) => {
                    resolve(PresetSize::Proportion(value), area, config.gaps, config.border)
                }
                WindowHeight::Fixed(value) => {
                    resolve(PresetSize::Fixed(value), area, config.gaps, config.border)
                }
            })
            .map(|height| height.max(2.0_f64.mul(config.border)).min(output_height));
        self.tiles
            .iter()
            .enumerate()
            .map(|(index, tile)| {
                let height = requested.map_or_else(
                    || space.div(count),
                    |height| {
                        if resized == Some(index) || self.tiles.len() == 1 {
                            height
                        } else {
                            space.sub(height).div(count.sub(1.0_f64))
                        }
                    },
                );
                let client = height.sub(2.0_f64.mul(config.border));
                let client = if tile.min_height == tile.max_height && tile.min_height.is_some() {
                    tile.min_height.unwrap_or(client)
                } else {
                    client.max(tile.min_height.unwrap_or(0.0_f64))
                };
                client.add(2.0_f64.mul(config.border))
            })
            .collect()
    }
}

impl Output {
    fn working(&self) -> Rect {
        let usable = self.geometry.usable;
        let struts = self.config.struts;
        Rect {
            x: usable.x.sub(self.geometry.area.x).add(struts.left),
            y: usable.y.sub(self.geometry.area.y).add(struts.top),
            width: usable.width.sub(struts.left).sub(struts.right),
            height: usable.height.sub(struts.top).sub(struts.bottom),
        }
    }
    fn column_width(&self, column: &Column) -> f64 {
        if column.tiles.iter().any(|tile| tile.mode == TileMode::Fullscreen) {
            return self.geometry.area.width;
        }
        column
            .width
            .map_or_else(
                || column.client_width.add(2.0_f64.mul(self.config.border)),
                |size| resolve(size, self.working().width, self.config.gaps, self.config.border),
            )
            .max(2.0_f64.mul(self.config.border))
    }
    fn column_x(&self, workspace: usize, column: usize) -> f64 {
        self.config.gaps.add(
            self.workspaces
                .get(workspace)
                .map_or(&[][..], |workspace| workspace.columns.as_slice())
                .iter()
                .take(column)
                .map(|column| self.column_width(column).add(self.config.gaps))
                .sum::<f64>(),
        )
    }
    fn workspace_view(&self, workspace: &Workspace, working: Rect) -> WorkspaceView {
        let mut horizontal = working.x.add(self.config.gaps).sub(workspace.view);
        let columns = workspace
            .columns
            .iter()
            .map(|column| {
                let width = self.column_width(column);
                let heights = column.heights(working.height, self.geometry.area.height, &self.config);
                let mut vertical = working.y.add(self.config.gaps);
                let tiles = column
                    .tiles
                    .iter()
                    .zip(heights)
                    .map(|(tile, height)| {
                        let normal = Rect {
                            x: horizontal,
                            y: vertical,
                            width,
                            height,
                        };
                        if column.display == ColumnDisplay::Normal {
                            vertical.add_assign(height.add(self.config.gaps));
                        }
                        tile.snapshot(normal, self.geometry, self.config.border, column.width.is_none())
                    })
                    .collect();
                horizontal.add_assign(width.add(self.config.gaps));
                ColumnView {
                    width: column.width,
                    display: column.display,
                    tiles,
                    focused_tile: column.focused,
                }
            })
            .collect();
        WorkspaceView {
            workspace: workspace.identity,
            name: workspace.name.clone(),
            original_output: workspace.original_output,
            view_offset: workspace.view,
            columns,
            focused_column: workspace.focused,
        }
    }
    fn snapshot(&self) -> OutputView {
        let working = self.working();
        let workspaces = self
            .workspaces
            .iter()
            .map(|workspace| self.workspace_view(workspace, working))
            .collect();
        OutputView {
            output: self.identity,
            geometry: self.geometry,
            working_area: working,
            gaps: self.config.gaps,
            border: self.config.border,
            workspaces,
            active_workspace: self.active,
        }
    }
}

fn positive(value: f64) -> bool {
    value.is_finite() && value > 0.0_f64
}

fn valid_size(size: PresetSize) -> bool {
    match size {
        PresetSize::Proportion(value) | PresetSize::Fixed(value) => positive(value),
    }
}

fn valid_change(change: SizeChange) -> bool {
    match change {
        SizeChange::SetProportion(value) | SizeChange::SetFixed(value) => positive(value),
        SizeChange::AdjustPercent(value) | SizeChange::AdjustFixed(value) => value.is_finite(),
    }
}

fn validate_config(config: &LayoutConfig) -> Result<(), LayoutError> {
    let struts = config.struts;
    if ![config.gaps, config.border]
        .into_iter()
        .all(|value| value.is_finite() && value >= 0.0_f64)
        || ![struts.left, struts.right, struts.top, struts.bottom]
            .into_iter()
            .all(f64::is_finite)
        || !config.default_column_width.is_none_or(valid_size)
        || !config
            .preset_column_widths
            .iter()
            .chain(&config.preset_window_heights)
            .copied()
            .all(valid_size)
    {
        return Err(LayoutError::InvalidConfig);
    }
    Ok(())
}

fn validate_output(geometry: OutputGeometry, config: &LayoutConfig) -> Result<(), LayoutError> {
    validate_config(config)?;
    for rect in [geometry.area, geometry.usable] {
        if !rect.x.is_finite() || !rect.y.is_finite() || !positive(rect.width) || !positive(rect.height) {
            return Err(LayoutError::InvalidGeometry);
        }
    }
    let struts = config.struts;
    if !positive(geometry.usable.width.sub(struts.left).sub(struts.right))
        || !positive(geometry.usable.height.sub(struts.top).sub(struts.bottom))
    {
        return Err(LayoutError::InvalidGeometry);
    }
    Ok(())
}

fn validate_rules(rules: &WindowRules) -> Result<(), LayoutError> {
    if !rules.default_column_width.flatten().is_none_or(valid_size)
        || !rules.default_window_height.is_none_or(valid_size)
        || !rules.min_height.is_none_or(positive)
        || !rules.max_height.is_none_or(positive)
        || rules
            .min_height
            .zip(rules.max_height)
            .is_some_and(|(min, max)| min > max)
    {
        return Err(LayoutError::InvalidSize);
    }
    Ok(())
}
