//! Pure scrollable-column layout: per output a vertical list of workspaces,
//! per workspace a horizontal strip of columns, per column a stack of tiles.
//!
//! Behaviour follows `docs/research/niri-behaviour-spec.md`. Coordinates are
//! fractional logical pixels. Tile rectangles in a [`Snapshot`] are relative
//! to the top-left corner of their output area, with the view offset applied.

use core::fmt;

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
    #[expect(clippy::todo, reason = "awaiting #51 implementation")]
    fn default() -> Self {
        todo!()
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    UnknownWindow(WindowId),
    DuplicateWindow(WindowId),
    UnknownOutput(OutputId),
    DuplicateOutput(OutputId),
    DuplicateWorkspaceName,
    NoOutput,
}

impl fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownWindow(window) => write!(formatter, "unknown window {}", window.0),
            Self::DuplicateWindow(window) => write!(formatter, "window {} already in the layout", window.0),
            Self::UnknownOutput(output) => write!(formatter, "unknown output {}", output.0),
            Self::DuplicateOutput(output) => write!(formatter, "output {} already in the layout", output.0),
            Self::DuplicateWorkspaceName => formatter.write_str("workspace name already in use"),
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

#[derive(Debug, Clone)]
pub struct Layout {
    #[expect(dead_code, reason = "read by the #51 implementation")]
    config: LayoutConfig,
}

#[expect(clippy::todo, reason = "awaiting #51 implementation")]
impl Layout {
    #[must_use]
    pub const fn new(config: LayoutConfig) -> Self {
        Self { config }
    }

    /// `layout_override` replaces the top-level layout settings on this output.
    ///
    /// # Errors
    /// [`LayoutError::DuplicateOutput`] when the id is already connected.
    pub fn add_output(
        &mut self,
        _id: OutputId,
        _geometry: OutputGeometry,
        _layout_override: Option<LayoutConfig>,
    ) -> Result<(), LayoutError> {
        todo!()
    }

    /// # Errors
    /// [`LayoutError::UnknownOutput`] when the id is not connected.
    pub fn remove_output(&mut self, _id: OutputId) -> Result<(), LayoutError> {
        todo!()
    }

    /// Creates a named workspace that persists while empty, on `output` or the focused output.
    ///
    /// # Errors
    /// [`LayoutError::DuplicateWorkspaceName`] or [`LayoutError::NoOutput`].
    pub fn add_named_workspace(
        &mut self,
        _name: String,
        _output: Option<OutputId>,
    ) -> Result<WorkspaceId, LayoutError> {
        todo!()
    }

    /// Opens a tiled window.
    ///
    /// # Errors
    /// [`LayoutError::DuplicateWindow`] or [`LayoutError::NoOutput`].
    pub fn add_window(&mut self, _id: WindowId, _rules: WindowRules) -> Result<(), LayoutError> {
        todo!()
    }

    /// # Errors
    /// [`LayoutError::UnknownWindow`] when the id is not in the layout.
    pub fn remove_window(&mut self, _id: WindowId) -> Result<(), LayoutError> {
        todo!()
    }

    /// A command with no focused target leaves the layout unchanged.
    pub fn apply(&mut self, _command: Command) {
        todo!()
    }

    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        todo!()
    }
}
