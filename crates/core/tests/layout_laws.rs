//! Layout laws from `docs/research/niri-behaviour-spec.md`; each test names the statement it pins.
//!
//! Statements tagged [confirm] in the spec, and the parts of a statement tagged [confirm],
//! have no test until a verifier settles them by running niri:
//! NS.1 strip end behaviour is not assumed; NW.6 wrap and off-preset choice; NH.1; NH.2;
//! NH.7 wrap and off-preset choice; NW.11 under `center-focused-column "always"`; NF.1 end behaviour; NF.2; NF.7 `-or-top`, `-or-bottom`
//! and index details; NF.10; NF.11 consume neighbour; NM.6; NN.1; NO.9 on moving a
//! workspace; NV.1 over-wide column alignment; NV.6.
//!
//! Outside the pure layout model, so tested at the compositor edge or by hand:
//! NW.13, NA.2 to NA.6 (animation); NW.14, NW.15, NH.8, NF.14 to NF.18, NV.7 to NV.10,
//! NWS.13 (pointer, touchpad and bindings); NF.4 (binding table); NM.3, NM.4, NM.12
//! (rendering); NM.8, NN.6, NO.10, NV.11 (floating layout); NN.10 (client commit timing);
//! NN.7 is enforced by the API shape: rules are only an argument of `add_window`.

#[cfg(test)]
#[expect(
    clippy::float_arithmetic,
    reason = "layout geometry is fractional logical pixels"
)]
#[expect(
    clippy::arithmetic_side_effects,
    reason = "test counts and indices stay small"
)]
mod layout_laws {
    use proptest::prelude::*;
    use tehuti_core::layout::{
        CenterFocusedColumn, ColumnDisplay, ColumnView, Command, Direction, Layout, LayoutConfig,
        LayoutError, OutputGeometry, OutputId, OutputView, PresetSize, Rect, SizeChange, Snapshot, Struts,
        TileMode, TileView, WindowHeight, WindowId, WindowRules, WorkspaceId, WorkspaceView,
    };

    const EPSILON: f64 = 1e-6;

    fn near(left: f64, right: f64) -> bool {
        (left - right).abs() <= EPSILON
    }

    fn niri_presets() -> Vec<PresetSize> {
        vec![
            PresetSize::Proportion(1.0 / 3.0),
            PresetSize::Proportion(0.5),
            PresetSize::Proportion(2.0 / 3.0),
        ]
    }

    #[derive(Debug, Clone)]
    enum Step {
        Open,
        Close(usize),
        Run(Command),
    }

    fn struts_strategy() -> impl Strategy<Value = Struts> {
        (-40.0_f64..80.0, -40.0_f64..80.0, -40.0_f64..80.0, -40.0_f64..80.0).prop_map(
            |(left, right, top, bottom)| Struts {
                left,
                right,
                top,
                bottom,
            },
        )
    }

    fn default_width_strategy() -> impl Strategy<Value = Option<PresetSize>> {
        prop_oneof![
            Just(Some(PresetSize::Proportion(0.5))),
            (0.1_f64..1.0).prop_map(|proportion| Some(PresetSize::Proportion(proportion))),
            (50.0_f64..1200.0).prop_map(|width| Some(PresetSize::Fixed(width))),
        ]
    }

    fn center_strategy() -> impl Strategy<Value = CenterFocusedColumn> {
        prop_oneof![
            Just(CenterFocusedColumn::Never),
            Just(CenterFocusedColumn::Always),
            Just(CenterFocusedColumn::OnOverflow),
        ]
    }

    fn config_strategy() -> impl Strategy<Value = LayoutConfig> {
        (
            0.0_f64..32.0,
            0.0_f64..8.0,
            struts_strategy(),
            default_width_strategy(),
            center_strategy(),
            any::<[bool; 3]>(),
        )
            .prop_map(
                |(gaps, border, struts, default_column_width, center, [single, above, back])| LayoutConfig {
                    gaps,
                    border,
                    struts,
                    preset_column_widths: niri_presets(),
                    preset_window_heights: niri_presets(),
                    default_column_width,
                    default_column_display: ColumnDisplay::Normal,
                    center_focused_column: center,
                    always_center_single_column: single,
                    empty_workspace_above_first: above,
                    workspace_auto_back_and_forth: back,
                },
            )
    }

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect { x, y, width, height }
    }

    fn geometry_at(x: f64, y: f64, width: f64, height: f64, bar: f64) -> OutputGeometry {
        OutputGeometry {
            area: rect(x, y, width, height),
            usable: rect(x, y + bar, width, height - bar),
        }
    }

    fn geometry_strategy() -> impl Strategy<Value = OutputGeometry> {
        (800.0_f64..3840.0, 600.0_f64..2160.0, 0.0_f64..48.0)
            .prop_map(|(width, height, bar)| geometry_at(0.0, 0.0, width, height, bar))
    }

    fn size_change_strategy() -> impl Strategy<Value = SizeChange> {
        prop_oneof![
            (0.05_f64..1.0).prop_map(SizeChange::SetProportion),
            (20.0_f64..2000.0).prop_map(SizeChange::SetFixed),
            prop_oneof![Just(10.0), Just(-10.0)].prop_map(SizeChange::AdjustPercent),
            (-200.0_f64..200.0).prop_map(SizeChange::AdjustFixed),
        ]
    }

    fn unit_commands() -> Vec<Command> {
        use Command::{
            CenterColumn, ConsumeOrExpelWindowLeft, ConsumeOrExpelWindowRight, ConsumeWindowIntoColumn,
            ExpandColumnToAvailableWidth, ExpelWindowFromColumn, FocusColumnFirst, FocusColumnLast,
            FocusColumnLeft, FocusColumnRight, FocusWindowBottom, FocusWindowDown, FocusWindowDownOrTop,
            FocusWindowTop, FocusWindowUp, FocusWindowUpOrBottom, FocusWindowUpOrColumnRight,
            FocusWorkspaceDown, FocusWorkspaceUp, MaximizeColumn, MoveColumnLeft, MoveColumnRight,
            MoveColumnToWorkspaceDown, MoveColumnToWorkspaceUp, MoveWindowDown, MoveWindowUp,
            MoveWorkspaceDown, MoveWorkspaceUp, ResetWindowHeight, SwitchPresetColumnWidth,
            SwitchPresetColumnWidthBack, SwitchPresetWindowHeight, SwitchPresetWindowHeightBack,
            ToggleColumnTabbedDisplay, ToggleFullscreen, ToggleMaximizeToEdges, ToggleWindowedFullscreen,
        };
        vec![
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
            MoveWindowUp,
            MoveWindowDown,
            ConsumeOrExpelWindowLeft,
            ConsumeOrExpelWindowRight,
            ConsumeWindowIntoColumn,
            ExpelWindowFromColumn,
            ToggleColumnTabbedDisplay,
            SwitchPresetColumnWidth,
            SwitchPresetColumnWidthBack,
            MaximizeColumn,
            ExpandColumnToAvailableWidth,
            SwitchPresetWindowHeight,
            SwitchPresetWindowHeightBack,
            ResetWindowHeight,
            CenterColumn,
            FocusWorkspaceDown,
            FocusWorkspaceUp,
            MoveColumnToWorkspaceDown,
            MoveColumnToWorkspaceUp,
            MoveWorkspaceDown,
            MoveWorkspaceUp,
            ToggleFullscreen,
            ToggleWindowedFullscreen,
            ToggleMaximizeToEdges,
        ]
    }

    fn direction_strategy() -> impl Strategy<Value = Direction> {
        prop_oneof![
            Just(Direction::Left),
            Just(Direction::Right),
            Just(Direction::Upward),
            Just(Direction::Downward),
        ]
    }

    fn command_strategy() -> impl Strategy<Value = Command> {
        prop_oneof![
            8 => proptest::sample::select(unit_commands()),
            1 => size_change_strategy().prop_map(Command::SetColumnWidth),
            1 => size_change_strategy().prop_map(Command::SetWindowHeight),
            1 => (0_usize..6).prop_map(Command::FocusWorkspaceAt),
            1 => (0_usize..6).prop_map(Command::MoveColumnToWorkspaceAt),
            1 => (0_usize..6).prop_map(Command::FocusWindowInColumn),
            1 => direction_strategy().prop_map(Command::FocusMonitor),
            1 => direction_strategy().prop_map(Command::MoveColumnToMonitor),
            1 => direction_strategy().prop_map(Command::MoveWorkspaceToMonitor),
        ]
    }

    fn ops_strategy() -> impl Strategy<Value = Vec<Step>> {
        prop::collection::vec(
            prop_oneof![
                3 => Just(Step::Open),
                1 => any::<usize>().prop_map(Step::Close),
                6 => command_strategy().prop_map(Step::Run),
            ],
            0..60,
        )
    }

    struct World {
        layout: Layout,
        open: Vec<WindowId>,
        next_window: u64,
    }

    impl World {
        fn with_outputs(config: LayoutConfig, outputs: &[OutputGeometry]) -> Result<Self, LayoutError> {
            let mut layout = Layout::new(config);
            for (output, geometry) in (0_u64..).zip(outputs) {
                layout.add_output(OutputId(output), *geometry, None)?;
            }
            Ok(Self {
                layout,
                open: Vec::new(),
                next_window: 0,
            })
        }

        fn single(config: LayoutConfig, geometry: OutputGeometry) -> Result<Self, LayoutError> {
            Self::with_outputs(config, &[geometry])
        }

        fn open(&mut self) -> Result<WindowId, LayoutError> {
            self.open_with(WindowRules::default())
        }

        fn open_with(&mut self, rules: WindowRules) -> Result<WindowId, LayoutError> {
            let window = WindowId(self.next_window);
            self.next_window += 1;
            self.layout.add_window(window, rules)?;
            self.open.push(window);
            Ok(window)
        }

        fn open_many(&mut self, count: usize) -> Result<(), LayoutError> {
            for _ in 0..count {
                let _window = self.open()?;
            }
            Ok(())
        }

        fn close(&mut self, window: WindowId) -> Result<(), LayoutError> {
            self.open.retain(|open| *open != window);
            self.layout.remove_window(window)
        }

        fn close_selected(&mut self, selector: usize) -> Result<(), LayoutError> {
            let selected = self.open.get(selector % self.open.len().max(1)).copied();
            selected.map_or(Ok(()), |window| self.close(window))
        }

        fn run(&mut self, ops: &[Step]) -> Result<(), LayoutError> {
            for step in ops {
                match step {
                    Step::Open => drop(self.open()?),
                    Step::Close(selector) => self.close_selected(*selector)?,
                    Step::Run(command) => self.layout.apply(*command),
                }
            }
            Ok(())
        }

        fn apply(&mut self, command: Command) {
            self.layout.apply(command);
        }

        fn apply_times(&mut self, command: Command, times: usize) {
            for _ in 0..times {
                self.layout.apply(command);
            }
        }

        fn snap(&self) -> Snapshot {
            self.layout.snapshot()
        }

        /// Walks focus one column at a time to `target` on the focused workspace.
        fn focus_column(&mut self, target: usize) {
            let current = focused_column_index(&self.snap()).unwrap_or(0);
            if current > target {
                self.apply_times(Command::FocusColumnLeft, current - target);
            } else {
                self.apply_times(Command::FocusColumnRight, target - current);
            }
        }

        /// Opens `count` windows and consumes them into the first column.
        fn stack(&mut self, count: usize) -> Result<(), LayoutError> {
            self.open_many(count)?;
            self.focus_column(0);
            self.apply_times(Command::ConsumeWindowIntoColumn, count.saturating_sub(1));
            Ok(())
        }

        /// Opens one window on each of `count` workspaces and ends on the trailing empty one.
        fn fill_workspaces(&mut self, count: usize) -> Result<(), LayoutError> {
            for _ in 0..count {
                let _window = self.open()?;
                self.apply(Command::FocusWorkspaceDown);
            }
            Ok(())
        }
    }

    fn focused_output(snap: &Snapshot) -> Option<&OutputView> {
        snap.outputs
            .iter()
            .find(|output| Some(output.output) == snap.focused_output)
    }

    fn active_workspace(output: &OutputView) -> Option<&WorkspaceView> {
        output.workspaces.get(output.active_workspace)
    }

    fn focused_workspace(snap: &Snapshot) -> Option<&WorkspaceView> {
        focused_output(snap).and_then(active_workspace)
    }

    fn focused_column_index(snap: &Snapshot) -> Option<usize> {
        focused_workspace(snap).and_then(|workspace| workspace.focused_column)
    }

    fn focused_column(snap: &Snapshot) -> Option<&ColumnView> {
        let workspace = focused_workspace(snap)?;
        workspace.columns.get(workspace.focused_column?)
    }

    fn all_tiles(snap: &Snapshot) -> impl Iterator<Item = &TileView> {
        snap.outputs
            .iter()
            .flat_map(|output| &output.workspaces)
            .flat_map(|workspace| &workspace.columns)
            .flat_map(|column| &column.tiles)
    }

    fn tile_of(snap: &Snapshot, window: WindowId) -> Option<&TileView> {
        all_tiles(snap).find(|tile| tile.window == window)
    }

    fn focused_tile(snap: &Snapshot) -> Option<&TileView> {
        tile_of(snap, snap.focused_window?)
    }

    fn workspace_of(snap: &Snapshot, window: WindowId) -> Option<&WorkspaceView> {
        snap.outputs
            .iter()
            .flat_map(|output| &output.workspaces)
            .find(|workspace| {
                workspace
                    .columns
                    .iter()
                    .any(|column| column.tiles.iter().any(|tile| tile.window == window))
            })
    }

    fn output_of_workspace(snap: &Snapshot, workspace: WorkspaceId) -> Option<OutputId> {
        snap.outputs
            .iter()
            .find(|output| {
                output
                    .workspaces
                    .iter()
                    .any(|candidate| candidate.workspace == workspace)
            })
            .map(|output| output.output)
    }

    fn workspace_ids(output: &OutputView) -> Vec<WorkspaceId> {
        output
            .workspaces
            .iter()
            .map(|workspace| workspace.workspace)
            .collect()
    }

    fn is_empty_unnamed(workspace: &WorkspaceView) -> bool {
        workspace.columns.is_empty() && workspace.name.is_none()
    }

    fn column_is_normal(column: &ColumnView) -> bool {
        column.tiles.iter().all(|tile| tile.mode == TileMode::Normal)
    }

    fn column_rect(column: &ColumnView) -> Option<Rect> {
        column.tiles.first().map(|tile| tile.tile)
    }

    fn fully_visible(tile: Rect, working: Rect) -> bool {
        tile.x >= working.x - EPSILON && tile.x + tile.width <= working.x + working.width + EPSILON
    }

    fn fits(tile: Rect, working: Rect, gaps: f64) -> bool {
        tile.width <= 2.0f64.mul_add(-gaps, working.width) + EPSILON
    }

    fn centred(tile: Rect, working: Rect) -> bool {
        near(tile.x + tile.width / 2.0, working.x + working.width / 2.0)
    }

    fn single_centred_config() -> impl Strategy<Value = LayoutConfig> {
        config_strategy().prop_map(|config| LayoutConfig {
            always_center_single_column: true,
            ..config
        })
    }

    fn proportion_config(proportion: f64) -> impl Strategy<Value = LayoutConfig> {
        config_strategy().prop_map(move |mut config| {
            config.default_column_width = Some(PresetSize::Proportion(proportion));
            config
        })
    }

    fn with_center(center: CenterFocusedColumn) -> impl Strategy<Value = LayoutConfig> {
        config_strategy().prop_map(move |mut config| {
            config.center_focused_column = center;
            config.always_center_single_column = false;
            config
        })
    }

    fn with_display(display: ColumnDisplay) -> impl Strategy<Value = LayoutConfig> {
        config_strategy().prop_map(move |mut config| {
            config.default_column_display = display;
            config
        })
    }

    fn snapshot_tile_sizes(snap: &Snapshot) -> Vec<(WindowId, Rect, f64, f64)> {
        all_tiles(snap)
            .map(|tile| {
                (
                    tile.window,
                    tile.tile,
                    tile.requested_client.width,
                    tile.requested_client.height,
                )
            })
            .collect()
    }

    /// Whether centring the focused column would push another currently fully visible column out
    /// of view (the NW.11 [confirm] case).
    fn displaces_on_centring(snap: &Snapshot) -> bool {
        let (Some(output), Some(workspace), Some(focused)) = (
            focused_output(snap),
            focused_workspace(snap),
            focused_column(snap).and_then(column_rect),
        ) else {
            return false;
        };
        let working = output.working_area;
        let shift = working.width.mul_add(0.5, working.x) - focused.width.mul_add(0.5, focused.x);
        workspace.columns.iter().enumerate().any(|(index, column)| {
            Some(index) != workspace.focused_column
                && column_rect(column).is_some_and(|tile| {
                    fully_visible(tile, working)
                        && !fully_visible(
                            Rect {
                                x: tile.x + shift,
                                ..tile
                            },
                            working,
                        )
                })
        })
    }

    // NS: the strip of columns.

    proptest! {
        #[test]
        fn ns_1_every_column_of_the_strip_is_reachable_one_step_at_a_time(
            config in config_strategy(),
            geometry in geometry_strategy(),
            count in 2_usize..12,
        ) {
            let mut world = World::single(config, geometry)?;
            world.open_many(count)?;
            let snap = world.snap();
            let workspace = focused_workspace(&snap).ok_or_else(|| TestCaseError::fail("no workspace"))?;
            prop_assert_eq!(workspace.columns.len(), count);
            world.focus_column(0);
            prop_assert_eq!(focused_column_index(&world.snap()), Some(0));
            for expected in 1..count {
                world.apply(Command::FocusColumnRight);
                prop_assert_eq!(focused_column_index(&world.snap()), Some(expected));
            }
        }

        #[test]
        fn ns_1_columns_extend_past_the_visible_area(
            config in proportion_config(1.0),
            geometry in geometry_strategy(),
            count in 2_usize..6,
        ) {
            let mut world = World::single(config, geometry)?;
            world.open_many(count)?;
            let snap = world.snap();
            let output = focused_output(&snap).ok_or_else(|| TestCaseError::fail("no output"))?;
            // NS.6: struts may leave a neighbour partly visible outside the working area.
            let off_screen = all_tiles(&snap)
                .filter(|tile| tile.tile.x + tile.tile.width <= output.working_area.x + EPSILON
                    || tile.tile.x >= output.working_area.x + output.working_area.width - EPSILON)
                .count();
            prop_assert!(off_screen >= count - 1);
        }

        #[test]
        fn ns_2_every_column_holds_a_vertical_stack_of_windows(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            let snap = world.snap();
            for column in snap.outputs.iter().flat_map(|output| &output.workspaces).flat_map(|workspace| &workspace.columns) {
                prop_assert!(!column.tiles.is_empty());
                if column.display == ColumnDisplay::Normal && column_is_normal(column) {
                    for pair in column.tiles.windows(2) {
                        prop_assert!(near(pair[0].tile.x, pair[1].tile.x));
                        prop_assert!(pair[0].tile.y + pair[0].tile.height <= pair[1].tile.y + EPSILON);
                    }
                }
            }
            prop_assert_eq!(all_tiles(&snap).count(), world.open.len());
        }

        #[test]
        fn ns_3_opening_a_window_never_resizes_existing_windows(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            let before = snapshot_tile_sizes(&world.snap());
            let _window = world.open()?;
            let after = world.snap();
            for (window, tile, client_width, client_height) in before {
                let now = tile_of(&after, window).ok_or_else(|| TestCaseError::fail("window lost"))?;
                prop_assert!(near(now.tile.width, tile.width) && near(now.tile.height, tile.height));
                prop_assert!(near(now.requested_client.width, client_width));
                prop_assert!(near(now.requested_client.height, client_height));
            }
        }

        #[test]
        fn ns_4_every_window_belongs_to_exactly_one_output(
            config in config_strategy(),
            first in geometry_strategy(),
            second in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let second = geometry_at(first.area.width, 0.0, second.area.width, second.area.height, 0.0);
            let mut world = World::with_outputs(config, &[first, second])?;
            world.run(&ops)?;
            let snap = world.snap();
            for window in &world.open {
                let owners = snap.outputs.iter().filter(|output| {
                    output.workspaces.iter().flat_map(|workspace| &workspace.columns)
                        .any(|column| column.tiles.iter().any(|tile| tile.window == *window))
                }).count();
                prop_assert_eq!(owners, 1);
            }
        }

        #[test]
        fn ns_5_and_ns_7_adjacent_columns_are_separated_by_exactly_the_fractional_gap(
            config in config_strategy(),
            half_pixels in 0_u32..64,
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let gaps = f64::from(half_pixels) / 2.0 + 0.25;
            let mut world = World::single(LayoutConfig { gaps, ..config }, geometry)?;
            world.run(&ops)?;
            let snap = world.snap();
            for workspace in snap.outputs.iter().flat_map(|output| &output.workspaces) {
                for pair in workspace.columns.windows(2) {
                    if column_is_normal(&pair[0]) && column_is_normal(&pair[1]) {
                        let (left, right) = (column_rect(&pair[0]), column_rect(&pair[1]));
                        if let (Some(left), Some(right)) = (left, right) {
                            prop_assert!(near(right.x - (left.x + left.width), gaps));
                        }
                    }
                }
            }
        }

        #[test]
        fn ns_6_struts_shrink_or_enlarge_the_working_area(
            config in config_strategy(),
            geometry in geometry_strategy(),
        ) {
            let struts = config.struts;
            let world = World::single(config, geometry)?;
            let snap = world.snap();
            let output = focused_output(&snap).ok_or_else(|| TestCaseError::fail("no output"))?;
            let usable = geometry.usable;
            prop_assert!(near(output.working_area.x, usable.x - geometry.area.x + struts.left));
            prop_assert!(near(output.working_area.y, usable.y - geometry.area.y + struts.top));
            prop_assert!(near(output.working_area.width, usable.width - struts.left - struts.right));
            prop_assert!(near(output.working_area.height, usable.height - struts.top - struts.bottom));
        }

        #[test]
        fn ns_8_a_window_wider_than_the_working_area_keeps_its_top_left_visible(
            config in config_strategy(),
            geometry in geometry_strategy(),
            extra in 1.0_f64..2000.0,
            ops in ops_strategy(),
        ) {
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            let working = focused_output(&world.snap()).ok_or_else(|| TestCaseError::fail("no output"))?.working_area;
            let gaps = focused_output(&world.snap()).map_or(0.0, |output| output.gaps);
            let _window = world.open()?;
            world.apply(Command::SetColumnWidth(SizeChange::SetFixed(working.width + extra)));
            let snap = world.snap();
            let tile = focused_tile(&snap).ok_or_else(|| TestCaseError::fail("nothing focused"))?;
            prop_assert!(tile.tile.x >= working.x - EPSILON && tile.tile.x <= working.x + gaps + EPSILON);
        }
    }

    // NW: column widths and presets.

    proptest! {
        #[test]
        fn nw_1_a_column_width_is_a_proportion_or_a_fixed_size(
            config in config_strategy(),
            geometry in geometry_strategy(),
            change in prop_oneof![(0.05_f64..1.0).prop_map(SizeChange::SetProportion), (20.0_f64..2000.0).prop_map(SizeChange::SetFixed)],
        ) {
            let mut world = World::single(config, geometry)?;
            let _window = world.open()?;
            world.apply(Command::SetColumnWidth(change));
            let width = focused_column(&world.snap()).and_then(|column| column.width);
            match change {
                SizeChange::SetProportion(proportion) => prop_assert_eq!(width, Some(PresetSize::Proportion(proportion))),
                SizeChange::SetFixed(pixels) => prop_assert_eq!(width, Some(PresetSize::Fixed(pixels))),
                SizeChange::AdjustPercent(_) | SizeChange::AdjustFixed(_) => {}
            }
        }

        #[test]
        fn nw_2_n_columns_of_one_nth_fill_the_working_area_for_any_gap(
            count in 1_u32..7,
            config in config_strategy(),
            geometry in geometry_strategy(),
        ) {
            let config = LayoutConfig { default_column_width: Some(PresetSize::Proportion(1.0 / f64::from(count))), ..config };
            let mut world = World::single(config, geometry)?;
            world.open_many(usize::try_from(count).unwrap_or(1))?;
            let snap = world.snap();
            let output = focused_output(&snap).ok_or_else(|| TestCaseError::fail("no output"))?;
            let widths: f64 = all_tiles(&snap).map(|tile| tile.tile.width).sum();
            prop_assert!(near(f64::from(count + 1).mul_add(output.gaps, widths), output.working_area.width));
        }

        #[test]
        fn nw_3_fixed_widths_apply_to_the_window_and_proportions_to_the_tile(
            config in config_strategy(),
            geometry in geometry_strategy(),
            other_border in 0.0_f64..8.0,
            proportion in 0.1_f64..1.0,
            pixels in 50.0_f64..1200.0,
        ) {
            let border = config.border;
            let mut fixed = World::single(LayoutConfig { default_column_width: Some(PresetSize::Fixed(pixels)), ..config.clone() }, geometry)?;
            let _window = fixed.open()?;
            let fixed_width = focused_tile(&fixed.snap()).map(|tile| tile.tile.width);
            prop_assert!(fixed_width.is_some_and(|width| near(width, 2.0f64.mul_add(border, pixels))));
            let proportional = LayoutConfig { default_column_width: Some(PresetSize::Proportion(proportion)), ..config };
            let mut thin = World::single(proportional.clone(), geometry)?;
            let mut thick = World::single(LayoutConfig { border: other_border, ..proportional }, geometry)?;
            let (_thin, _thick) = (thin.open()?, thick.open()?);
            let widths = (focused_tile(&thin.snap()).map(|tile| tile.tile.width), focused_tile(&thick.snap()).map(|tile| tile.tile.width));
            prop_assert!(matches!(widths, (Some(thin_width), Some(thick_width)) if near(thin_width, thick_width)));
        }

        #[test]
        fn nw_4_the_border_shrinks_the_window_inside_its_tile(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let border = config.border;
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            for tile in all_tiles(&world.snap()).filter(|tile| tile.mode == TileMode::Normal) {
                prop_assert!(near(tile.requested_client.width, 2.0f64.mul_add(-border, tile.tile.width)));
                prop_assert!(near(tile.requested_client.height, 2.0f64.mul_add(-border, tile.tile.height)));
            }
        }

        #[test]
        fn nw_6_switching_presets_steps_forward_and_back_through_the_list(
            config in config_strategy(),
            geometry in geometry_strategy(),
            mut presets in prop::collection::btree_set(1_u32..20, 2..6),
            pick in any::<prop::sample::Index>(),
        ) {
            let presets: Vec<f64> = core::mem::take(&mut presets).into_iter().map(|twentieth| f64::from(twentieth) / 20.0).collect();
            let config = LayoutConfig { preset_column_widths: presets.iter().copied().map(PresetSize::Proportion).collect(), ..config };
            let mut world = World::single(config, geometry)?;
            let _window = world.open()?;
            let index = pick.index(presets.len() - 1);
            world.apply(Command::SetColumnWidth(SizeChange::SetProportion(presets[index])));
            world.apply(Command::SwitchPresetColumnWidth);
            prop_assert_eq!(focused_column(&world.snap()).and_then(|column| column.width), Some(PresetSize::Proportion(presets[index + 1])));
            world.apply(Command::SwitchPresetColumnWidthBack);
            prop_assert_eq!(focused_column(&world.snap()).and_then(|column| column.width), Some(PresetSize::Proportion(presets[index])));
        }

        #[test]
        fn nw_7_a_new_column_takes_the_default_column_width(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let expected = config.default_column_width;
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            let window = world.open()?;
            let snap = world.snap();
            let workspace = workspace_of(&snap, window).ok_or_else(|| TestCaseError::fail("window lost"))?;
            let column = workspace.columns.iter().find(|column| column.tiles.iter().any(|tile| tile.window == window));
            prop_assert_eq!(column.and_then(|column| column.width), expected);
        }

        #[test]
        fn nw_8_an_empty_default_width_lets_the_client_choose(
            config in config_strategy(),
            geometry in geometry_strategy(),
        ) {
            let mut world = World::single(LayoutConfig { default_column_width: None, ..config }, geometry)?;
            let window = world.open()?;
            let tile = tile_of(&world.snap(), window).cloned().ok_or_else(|| TestCaseError::fail("window lost"))?;
            prop_assert!(near(tile.requested_client.width, 0.0));
            prop_assert!(tile.requested_client.height > 0.0);
        }

        #[test]
        fn nw_9_ten_percent_steps_change_the_proportion_by_ten_points(
            config in config_strategy(),
            geometry in geometry_strategy(),
            proportion in 0.2_f64..0.8,
            step in prop_oneof![Just(10.0), Just(-10.0)],
        ) {
            let mut world = World::single(config, geometry)?;
            let _window = world.open()?;
            world.apply(Command::SetColumnWidth(SizeChange::SetProportion(proportion)));
            world.apply(Command::SetColumnWidth(SizeChange::AdjustPercent(step)));
            let width = focused_column(&world.snap()).and_then(|column| column.width);
            prop_assert!(matches!(width, Some(PresetSize::Proportion(now)) if near(now, proportion + step / 100.0)));
        }

        #[test]
        fn nw_10_maximize_column_fills_the_working_area_minus_gaps_and_keeps_borders(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let border = config.border;
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            let _window = world.open()?;
            world.apply(Command::MaximizeColumn);
            let snap = world.snap();
            let output = focused_output(&snap).ok_or_else(|| TestCaseError::fail("no output"))?;
            let column = focused_column(&snap).ok_or_else(|| TestCaseError::fail("no column"))?;
            for tile in &column.tiles {
                prop_assert!(near(2.0f64.mul_add(output.gaps, tile.tile.width), output.working_area.width));
                prop_assert!(near(tile.requested_client.width, 2.0f64.mul_add(-border, tile.tile.width)));
            }
        }

        #[test]
        fn nw_11_expanding_fills_free_space_and_keeps_visible_columns_visible(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let always = config.center_focused_column == CenterFocusedColumn::Always;
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            let before = world.snap();
            let output = focused_output(&before).ok_or_else(|| TestCaseError::fail("no output"))?.clone();
            let workspace = focused_workspace(&before).ok_or_else(|| TestCaseError::fail("no workspace"))?;
            prop_assume!(workspace.columns.iter().all(column_is_normal));
            let visible: Vec<WindowId> = workspace.columns.iter()
                .filter(|column| column_rect(column).is_some_and(|tile| fully_visible(tile, output.working_area)))
                .filter_map(|column| column.tiles.first().map(|tile| tile.window)).collect();
            prop_assume!(focused_tile(&before).is_some_and(|tile| visible.contains(&tile.window)));
            // NW.11 [confirm]: under "always" the case where centring the expanded column would push
            // another visible column out of view is open. The expected layout packs the visible
            // columns edge to edge; centring then displaces something unless the focused column
            // already sits in the middle.
            let focused_window = focused_tile(&before).map(|tile| tile.window);
            let others: f64 = visible.iter().filter(|window| Some(**window) != focused_window)
                .filter_map(|window| tile_of(&before, *window)).map(|tile| tile.tile.width).sum();
            let count = f64::from(u32::try_from(visible.len()).unwrap_or(u32::MAX));
            let expanded = (count + 1.0).mul_add(-output.gaps, output.working_area.width) - others;
            let mut x = output.working_area.x + output.gaps;
            let mut predicted = None;
            for window in &visible {
                let width = if Some(*window) == focused_window { expanded } else { tile_of(&before, *window).map_or(0.0, |tile| tile.tile.width) };
                if Some(*window) == focused_window {
                    predicted = Some(Rect { x, width, ..output.working_area });
                }
                x += width + output.gaps;
            }
            let displaces = visible.len() > 1 && !predicted.is_some_and(|rect| centred(rect, output.working_area));
            prop_assume!(!always || !displaces);
            world.apply(Command::ExpandColumnToAvailableWidth);
            let after = world.snap();
            let widths: f64 = visible.iter().filter_map(|window| tile_of(&after, *window)).map(|tile| tile.tile.width).sum();
            for window in &visible {
                prop_assert!(tile_of(&after, *window).is_some_and(|tile| fully_visible(tile.tile, output.working_area)));
            }
            prop_assert!(near((count + 1.0).mul_add(output.gaps, widths), output.working_area.width));
        }

        #[test]
        fn nw_12_windows_in_a_normal_column_share_its_width(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            let snap = world.snap();
            for column in snap.outputs.iter().flat_map(|output| &output.workspaces).flat_map(|workspace| &workspace.columns) {
                if column.display == ColumnDisplay::Normal && column_is_normal(column) {
                    let first = column_rect(column).map_or(0.0, |tile| tile.width);
                    prop_assert!(column.tiles.iter().all(|tile| near(tile.tile.width, first)));
                }
            }
        }
    }

    #[test]
    fn nw_5_nw_7_nh_7_shipped_defaults() {
        let config = LayoutConfig::default();
        assert_eq!(config.preset_column_widths, niri_presets());
        assert_eq!(config.preset_window_heights, niri_presets());
        assert_eq!(config.default_column_width, Some(PresetSize::Proportion(0.5)));
    }

    // NH: window heights within a column.

    proptest! {
        #[test]
        fn nh_3_resizing_one_window_scales_the_others_proportionally(
            config in config_strategy(),
            geometry in geometry_strategy(),
            count in 3_usize..6,
            height in 50.0_f64..400.0,
        ) {
            let mut world = World::single(config, geometry)?;
            world.stack(count)?;
            let before = focused_column(&world.snap()).cloned().ok_or_else(|| TestCaseError::fail("no column"))?;
            prop_assume!(before.tiles.len() >= 3 && before.display == ColumnDisplay::Normal);
            let resized = world.snap().focused_window.ok_or_else(|| TestCaseError::fail("nothing focused"))?;
            world.apply(Command::SetWindowHeight(SizeChange::SetFixed(height)));
            let snap = world.snap();
            prop_assert!(tile_of(&snap, resized).is_some_and(|tile| near(tile.requested_client.height, height)));
            let others: Vec<(f64, f64)> = before.tiles.iter().filter(|tile| tile.window != resized)
                .filter_map(|tile| tile_of(&snap, tile.window).map(|now| (tile.tile.height, now.tile.height))).collect();
            for pair in others.windows(2) {
                prop_assert!(near(pair[0].0 * pair[1].1, pair[1].0 * pair[0].1));
            }
        }

        #[test]
        fn nh_4_a_single_window_height_is_clamped_to_the_output(
            config in config_strategy(),
            geometry in geometry_strategy(),
            height in 20.0_f64..6000.0,
        ) {
            let mut world = World::single(config, geometry)?;
            let _window = world.open()?;
            world.apply(Command::SetWindowHeight(SizeChange::SetFixed(height)));
            let tile = focused_tile(&world.snap()).map(|tile| tile.tile.height);
            prop_assert!(tile.is_some_and(|tile| tile <= geometry.area.height + EPSILON));
        }

        #[test]
        fn nh_5_closing_a_window_gives_its_height_to_the_rest_of_the_column(
            config in config_strategy(),
            geometry in geometry_strategy(),
            count in 2_usize..6,
        ) {
            let mut world = World::single(config, geometry)?;
            world.stack(count)?;
            let column = focused_column(&world.snap()).cloned().ok_or_else(|| TestCaseError::fail("no column"))?;
            prop_assume!(column.tiles.len() >= 2 && column.display == ColumnDisplay::Normal);
            let span = |stack: &ColumnView| stack.tiles.first().zip(stack.tiles.last())
                .map(|(top, bottom)| bottom.tile.y + bottom.tile.height - top.tile.y);
            let survivor = column.tiles.first().map(|tile| tile.window).ok_or_else(|| TestCaseError::fail("empty"))?;
            let closed = column.tiles.last().map(|tile| tile.window).ok_or_else(|| TestCaseError::fail("empty"))?;
            world.close(closed)?;
            let snap = world.snap();
            let after = workspace_of(&snap, survivor).and_then(|workspace| workspace.columns.iter()
                .find(|candidate| candidate.tiles.iter().any(|tile| tile.window == survivor)));
            prop_assert!(matches!((span(&column), after.and_then(span)), (Some(old_x), Some(new_x)) if near(old_x, new_x)));
        }

        #[test]
        fn nh_6_ten_percent_steps_change_height_and_reset_returns_to_automatic(
            config in config_strategy(),
            geometry in geometry_strategy(),
            count in 2_usize..5,
        ) {
            let mut world = World::single(config, geometry)?;
            world.stack(count)?;
            let before = focused_tile(&world.snap()).cloned().ok_or_else(|| TestCaseError::fail("nothing focused"))?;
            prop_assert_eq!(before.height, WindowHeight::Auto);
            world.apply(Command::SetWindowHeight(SizeChange::AdjustPercent(-10.0)));
            let shrunk = focused_tile(&world.snap()).map(|tile| tile.tile.height);
            prop_assert!(shrunk.is_some_and(|height| height < before.tile.height));
            world.apply(Command::ResetWindowHeight);
            let reset = focused_tile(&world.snap()).cloned().ok_or_else(|| TestCaseError::fail("nothing focused"))?;
            prop_assert_eq!(reset.height, WindowHeight::Auto);
            prop_assert!(near(reset.tile.height, before.tile.height));
        }

        #[test]
        fn nh_7_switching_height_presets_steps_forward_and_back(
            config in config_strategy(),
            geometry in geometry_strategy(),
            pick in 0_usize..2,
        ) {
            let presets = niri_presets();
            let mut world = World::single(config, geometry)?;
            world.stack(3)?;
            let PresetSize::Proportion(start) = presets[pick] else { return Err(TestCaseError::fail("preset")) };
            world.apply(Command::SetWindowHeight(SizeChange::SetProportion(start)));
            world.apply(Command::SwitchPresetWindowHeight);
            let height = focused_tile(&world.snap()).map(|tile| tile.height);
            prop_assert_eq!(height, Some(match presets[pick + 1] { PresetSize::Proportion(next) => WindowHeight::Proportion(next), PresetSize::Fixed(next) => WindowHeight::Fixed(next) }));
            world.apply(Command::SwitchPresetWindowHeightBack);
            prop_assert_eq!(focused_tile(&world.snap()).map(|tile| tile.height), Some(WindowHeight::Proportion(start)));
        }

        #[test]
        fn nh_9_equal_min_and_max_height_rules_fix_the_requested_height(
            config in config_strategy(),
            geometry in geometry_strategy(),
            bound in 100.0_f64..400.0,
            minimum in 100.0_f64..4000.0,
        ) {
            let mut world = World::single(config, geometry)?;
            let fixed = world.open_with(WindowRules { min_height: Some(bound), max_height: Some(bound), ..WindowRules::default() })?;
            let floor = world.open_with(WindowRules { min_height: Some(minimum), ..WindowRules::default() })?;
            let snap = world.snap();
            prop_assert!(tile_of(&snap, fixed).is_some_and(|tile| near(tile.requested_client.height, bound)));
            prop_assert!(tile_of(&snap, floor).is_some_and(|tile| tile.requested_client.height >= minimum - EPSILON));
        }

        #[test]
        fn nh_10_every_tab_has_the_full_column_size(
            config in with_display(ColumnDisplay::Tabbed),
            geometry in geometry_strategy(),
            count in 2_usize..5,
        ) {
            let mut world = World::single(config, geometry)?;
            world.stack(count)?;
            let column = focused_column(&world.snap()).cloned().ok_or_else(|| TestCaseError::fail("no column"))?;
            prop_assume!(column.tiles.len() >= 2 && column.display == ColumnDisplay::Tabbed);
            let lone = world.open()?;
            let full = tile_of(&world.snap(), lone).map(|tile| tile.tile.height).ok_or_else(|| TestCaseError::fail("window lost"))?;
            for tile in &column.tiles {
                prop_assert!(near(tile.tile.height, full));
                prop_assert_eq!(tile.requested_client, column.tiles[0].requested_client);
            }
        }
    }

    // NF: focus and movement.

    fn interior_column(world: &mut World, count: usize, pick: prop::sample::Index) -> Option<usize> {
        world.open_many(count).ok()?;
        let target = 1 + pick.index(count.checked_sub(2)?);
        world.focus_column(target);
        Some(target)
    }

    proptest! {
        #[test]
        fn nf_1_focus_column_moves_to_the_adjacent_column(
            config in config_strategy(),
            geometry in geometry_strategy(),
            count in 3_usize..8,
            pick in any::<prop::sample::Index>(),
            rightward in any::<bool>(),
        ) {
            let mut world = World::single(config, geometry)?;
            let target = interior_column(&mut world, count, pick).ok_or_else(|| TestCaseError::fail("setup"))?;
            world.apply(if rightward { Command::FocusColumnRight } else { Command::FocusColumnLeft });
            prop_assert_eq!(focused_column_index(&world.snap()), Some(if rightward { target + 1 } else { target - 1 }));
        }

        #[test]
        fn nf_3_moving_a_column_swaps_it_with_its_neighbour_and_keeps_focus(
            config in config_strategy(),
            geometry in geometry_strategy(),
            count in 3_usize..8,
            pick in any::<prop::sample::Index>(),
            rightward in any::<bool>(),
        ) {
            let mut world = World::single(config, geometry)?;
            let target = interior_column(&mut world, count, pick).ok_or_else(|| TestCaseError::fail("setup"))?;
            let before = world.snap();
            let order = |snap: &Snapshot| focused_workspace(snap).map(|workspace| workspace.columns.iter()
                .filter_map(|column| column.tiles.first().map(|tile| tile.window)).collect::<Vec<_>>()).unwrap_or_default();
            world.apply(if rightward { Command::MoveColumnRight } else { Command::MoveColumnLeft });
            let after = world.snap();
            let neighbour = if rightward { target + 1 } else { target - 1 };
            let mut expected = order(&before);
            expected.swap(target, neighbour);
            prop_assert_eq!(order(&after), expected);
            prop_assert_eq!(after.focused_window, before.focused_window);
            prop_assert_eq!(focused_column_index(&after), Some(neighbour));
        }

        #[test]
        fn nf_5_a_column_regains_focus_on_its_last_focused_window(
            config in config_strategy(),
            geometry in geometry_strategy(),
            count in 3_usize..6,
            pick in any::<prop::sample::Index>(),
        ) {
            let mut world = World::single(config, geometry)?;
            world.stack(count)?;
            let open = world.open()?;
            world.focus_column(0);
            let height = focused_column(&world.snap()).map_or(0, |column| column.tiles.len());
            prop_assume!(height >= 2);
            world.apply(Command::FocusWindowTop);
            world.apply_times(Command::FocusWindowDown, pick.index(height));
            let chosen = world.snap().focused_window;
            world.apply(Command::FocusColumnRight);
            prop_assert_eq!(world.snap().focused_window, Some(open));
            world.apply(Command::FocusColumnLeft);
            prop_assert_eq!(world.snap().focused_window, chosen);
        }

        #[test]
        fn nf_6_and_nf_7_focus_moves_within_the_column_and_selects_the_tab(
            config in config_strategy(),
            display in prop_oneof![Just(ColumnDisplay::Normal), Just(ColumnDisplay::Tabbed)],
            geometry in geometry_strategy(),
            count in 2_usize..6,
            pick in any::<prop::sample::Index>(),
        ) {
            let mut world = World::single(LayoutConfig { default_column_display: display, ..config }, geometry)?;
            world.stack(count)?;
            let stacked = focused_column(&world.snap()).cloned().ok_or_else(|| TestCaseError::fail("no column"))?;
            let last = stacked.tiles.len().saturating_sub(1);
            prop_assume!(last >= 1);
            world.apply(Command::FocusWindowTop);
            prop_assert_eq!(focused_column(&world.snap()).map(|column| column.focused_tile), Some(0));
            let target = pick.index(last);
            world.apply_times(Command::FocusWindowDown, target);
            let snap = world.snap();
            prop_assert_eq!(focused_column(&snap).map(|column| column.focused_tile), Some(target));
            prop_assert_eq!(snap.focused_window, stacked.tiles.get(target).map(|tile| tile.window));
            world.apply(Command::FocusWindowBottom);
            prop_assert_eq!(focused_column(&world.snap()).map(|column| column.focused_tile), Some(last));
            world.apply(Command::FocusWindowUp);
            prop_assert_eq!(focused_column(&world.snap()).map(|column| column.focused_tile), Some(last - 1));
        }

        #[test]
        fn nf_8_moving_a_window_reorders_it_within_its_column(
            config in config_strategy(),
            geometry in geometry_strategy(),
            count in 2_usize..6,
        ) {
            let mut world = World::single(config, geometry)?;
            world.stack(count)?;
            prop_assume!(focused_column(&world.snap()).is_some_and(|column| column.tiles.len() >= 2));
            world.apply(Command::FocusWindowTop);
            let order = |snap: &Snapshot| focused_column(snap).map(|column| column.tiles.iter().map(|tile| tile.window).collect::<Vec<_>>()).unwrap_or_default();
            let before = world.snap();
            world.apply(Command::MoveWindowDown);
            let after = world.snap();
            let mut expected = order(&before);
            expected.swap(0, 1);
            prop_assert_eq!(order(&after), expected);
            prop_assert_eq!(after.focused_window, before.focused_window);
            world.apply(Command::MoveWindowUp);
            prop_assert_eq!(order(&world.snap()), order(&before));
        }

        #[test]
        fn nf_9_up_or_column_right_moves_up_then_right(
            config in config_strategy(),
            geometry in geometry_strategy(),
            count in 2_usize..5,
        ) {
            let mut world = World::single(config, geometry)?;
            world.stack(count)?;
            let _right = world.open()?;
            world.focus_column(0);
            prop_assume!(focused_column(&world.snap()).is_some_and(|column| column.tiles.len() >= 2));
            world.apply(Command::FocusWindowBottom);
            let bottom = focused_column(&world.snap()).map(|column| column.focused_tile);
            world.apply(Command::FocusWindowUpOrColumnRight);
            prop_assert_eq!(focused_column(&world.snap()).map(|column| column.focused_tile), bottom.map(|index| index - 1));
            world.apply(Command::FocusWindowTop);
            world.apply(Command::FocusWindowUpOrColumnRight);
            prop_assert_eq!(focused_column_index(&world.snap()), Some(1));
        }

        #[test]
        fn nf_11_and_nf_13_expel_gives_the_window_its_own_column(
            config in config_strategy(),
            display in prop_oneof![Just(ColumnDisplay::Normal), Just(ColumnDisplay::Tabbed)],
            geometry in geometry_strategy(),
            count in 2_usize..6,
        ) {
            let mut world = World::single(LayoutConfig { default_column_display: display, ..config }, geometry)?;
            world.stack(count)?;
            let before = world.snap();
            let height = focused_column(&before).map_or(0, |column| column.tiles.len());
            prop_assume!(height >= 2);
            let columns = focused_workspace(&before).map_or(0, |workspace| workspace.columns.len());
            world.apply(Command::ExpelWindowFromColumn);
            let after = world.snap();
            prop_assert_eq!(focused_workspace(&after).map(|workspace| workspace.columns.len()), Some(columns + 1));
            let own = focused_workspace(&after).and_then(|workspace| workspace.columns.iter()
                .find(|column| column.tiles.iter().any(|tile| Some(tile.window) == before.focused_window)));
            prop_assert!(own.is_some_and(|column| column.tiles.len() == 1 && column.display == display));
        }

        #[test]
        fn nf_12_consuming_resizes_the_window_at_once(
            config in config_strategy(),
            geometry in geometry_strategy(),
            host_width in 0.1_f64..0.45,
            guest_width in 0.55_f64..0.95,
        ) {
            let mut world = World::single(LayoutConfig { default_column_display: ColumnDisplay::Normal, ..config }, geometry)?;
            let host = world.open()?;
            world.apply(Command::SetColumnWidth(SizeChange::SetProportion(host_width)));
            let guest = world.open()?;
            world.apply(Command::SetColumnWidth(SizeChange::SetProportion(guest_width)));
            let before = world.snap();
            let host_tile = tile_of(&before, host).cloned().ok_or_else(|| TestCaseError::fail("host lost"))?;
            let guest_tile = tile_of(&before, guest).cloned().ok_or_else(|| TestCaseError::fail("guest lost"))?;
            prop_assert!(!near(host_tile.tile.width, guest_tile.tile.width));
            world.focus_column(0);
            world.apply(Command::ConsumeWindowIntoColumn);
            let snap = world.snap();
            let column = focused_column(&snap).cloned().ok_or_else(|| TestCaseError::fail("no column"))?;
            prop_assert_eq!(column.tiles.iter().map(|tile| tile.window).collect::<Vec<_>>(), vec![host, guest]);
            let consumed = tile_of(&snap, guest).cloned().ok_or_else(|| TestCaseError::fail("guest lost"))?;
            prop_assert!(near(consumed.tile.width, host_tile.tile.width), "tile {} expected {}", consumed.tile.width, host_tile.tile.width);
            prop_assert!(near(consumed.requested_client.width, host_tile.requested_client.width));
            prop_assert!(!near(consumed.requested_client.width, guest_tile.requested_client.width));
        }

        #[test]
        fn nf_13_a_new_window_takes_the_default_column_display(
            config in config_strategy(),
            display in prop_oneof![Just(ColumnDisplay::Normal), Just(ColumnDisplay::Tabbed)],
            override_display in prop::option::of(prop_oneof![Just(ColumnDisplay::Normal), Just(ColumnDisplay::Tabbed)]),
            geometry in geometry_strategy(),
        ) {
            let mut world = World::single(LayoutConfig { default_column_display: display, ..config }, geometry)?;
            let _window = world.open_with(WindowRules { default_column_display: override_display, ..WindowRules::default() })?;
            prop_assert_eq!(focused_column(&world.snap()).map(|column| column.display), Some(override_display.unwrap_or(display)));
        }
    }

    // NWS: workspaces stacked vertically.

    proptest! {
        #[test]
        fn nws_1_to_nws_5_each_output_keeps_its_empty_workspaces_in_place(
            config in config_strategy(),
            first in geometry_strategy(),
            second in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let above = config.empty_workspace_above_first;
            let second = geometry_at(first.area.width, 0.0, second.area.width, second.area.height, 0.0);
            let mut world = World::with_outputs(config, &[first, second])?;
            world.run(&ops)?;
            let snap = world.snap();
            for output in &snap.outputs {
                prop_assert!(output.workspaces.last().is_some_and(is_empty_unnamed), "NWS.2");
                prop_assert!(!above || output.workspaces.first().is_some_and(is_empty_unnamed), "NWS.5");
                let last = output.workspaces.len() - 1;
                for (index, workspace) in output.workspaces.iter().enumerate() {
                    let allowed = index == last || index == output.active_workspace || (above && index == 0);
                    prop_assert!(allowed || !is_empty_unnamed(workspace), "NWS.4");
                }
            }
        }

        #[test]
        fn nws_3_opening_on_the_trailing_workspace_appends_a_new_one(
            config in config_strategy(),
            geometry in geometry_strategy(),
            count in 0_usize..4,
        ) {
            let mut world = World::single(config, geometry)?;
            world.fill_workspaces(count)?;
            let before = focused_output(&world.snap()).map(|output| output.workspaces.len()).unwrap_or_default();
            let window = world.open()?;
            let snap = world.snap();
            let output = focused_output(&snap).ok_or_else(|| TestCaseError::fail("no output"))?;
            prop_assert_eq!(output.workspaces.len(), before + 1);
            prop_assert!(output.workspaces.last().is_some_and(is_empty_unnamed));
            prop_assert_eq!(workspace_of(&snap, window).map(|workspace| workspace.workspace), active_workspace(output).map(|workspace| workspace.workspace));
        }

        #[test]
        fn nws_6_named_workspaces_persist_while_empty(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let mut world = World::single(config, geometry)?;
            let named = world.layout.add_named_workspace("web".into(), None)?;
            world.run(&ops)?;
            let snap = world.snap();
            let found = snap.outputs.iter().flat_map(|output| &output.workspaces).find(|workspace| workspace.workspace == named);
            prop_assert!(found.is_some_and(|workspace| workspace.name.as_deref() == Some("web")));
        }

        #[test]
        fn nws_7_focus_workspace_switches_to_the_adjacent_one(
            config in config_strategy(),
            geometry in geometry_strategy(),
            count in 2_usize..5,
            pick in any::<prop::sample::Index>(),
            downward in any::<bool>(),
        ) {
            let mut world = World::single(config, geometry)?;
            world.fill_workspaces(count)?;
            let ids = focused_output(&world.snap()).map(workspace_ids).unwrap_or_default();
            let start = 1 + pick.index(count - 1);
            world.apply(Command::FocusWorkspaceAt(start));
            world.apply(if downward { Command::FocusWorkspaceDown } else { Command::FocusWorkspaceUp });
            let expected = ids.get(if downward { start + 1 } else { start - 1 }).copied();
            prop_assert_eq!(focused_workspace(&world.snap()).map(|workspace| workspace.workspace), expected);
        }

        #[test]
        fn nws_8_move_column_to_workspace_moves_its_windows(
            config in config_strategy(),
            geometry in geometry_strategy(),
            count in 2_usize..5,
            pick in any::<prop::sample::Index>(),
            target in any::<prop::sample::Index>(),
            extra in 0_usize..4,
        ) {
            let mut world = World::single(config, geometry)?;
            world.fill_workspaces(count)?;
            let ids = focused_output(&world.snap()).map(workspace_ids).unwrap_or_default();
            let (start, target) = (pick.index(count), target.index(count + 1));
            prop_assume!(start != target);
            world.apply(Command::FocusWorkspaceAt(start));
            // NWS.8 moves the focused column; an empty workspace (NWS.5 top) has none.
            prop_assume!(world.snap().focused_window.is_some());
            // Stack `extra` more windows into the focused column so it has several members.
            world.open_many(extra)?;
            world.focus_column(0);
            world.apply_times(Command::ConsumeWindowIntoColumn, extra);
            let members: Vec<WindowId> = focused_column(&world.snap()).map(|column| column.tiles.iter().map(|tile| tile.window).collect()).unwrap_or_default();
            prop_assert_eq!(members.len(), extra + 1);
            world.apply(Command::MoveColumnToWorkspaceAt(target));
            let snap = world.snap();
            for window in &members {
                prop_assert_eq!(workspace_of(&snap, *window).map(|workspace| workspace.workspace), ids.get(target).copied(), "window {:?}", window);
            }
            // The column moves whole: one column, members in the same order.
            let moved: Vec<Vec<WindowId>> = members.first().and_then(|first| workspace_of(&snap, *first))
                .map(|workspace| workspace.columns.iter()
                    .map(|column| column.tiles.iter().map(|tile| tile.window).collect::<Vec<_>>())
                    .filter(|column| column.iter().any(|window| members.contains(window)))
                    .collect())
                .unwrap_or_default();
            prop_assert_eq!(moved, vec![members]);
        }

        #[test]
        fn nws_8_move_column_down_and_up_moves_to_the_adjacent_workspace(
            config in config_strategy(),
            geometry in geometry_strategy(),
            count in 2_usize..5,
            pick in any::<prop::sample::Index>(),
        ) {
            let mut world = World::single(config, geometry)?;
            world.fill_workspaces(count)?;
            let ids = focused_output(&world.snap()).map(workspace_ids).unwrap_or_default();
            let start = 1 + pick.index(count - 1);
            world.apply(Command::FocusWorkspaceAt(start));
            let window = world.snap().focused_window.ok_or_else(|| TestCaseError::fail("nothing focused"))?;
            world.apply(Command::MoveColumnToWorkspaceUp);
            prop_assert_eq!(workspace_of(&world.snap(), window).map(|workspace| workspace.workspace), ids.get(start - 1).copied());
            // NWS.4 may remove the emptied workspace, so the target is read after the first move.
            let (position, column) = locate(&world.snap(), window).ok_or_else(|| TestCaseError::fail("window lost"))?;
            world.apply(Command::FocusWorkspaceAt(position));
            world.focus_column(column);
            let below = focused_output(&world.snap()).map(workspace_ids).and_then(|current| current.get(position + 1).copied());
            world.apply(Command::MoveColumnToWorkspaceDown);
            prop_assert_eq!(workspace_of(&world.snap(), window).map(|workspace| workspace.workspace), below);
        }

        #[test]
        fn nws_9_and_nws_10_moving_a_workspace_reorders_and_indices_follow_position(
            config in config_strategy(),
            geometry in geometry_strategy(),
            count in 3_usize..6,
            pick in any::<prop::sample::Index>(),
        ) {
            let config = LayoutConfig { empty_workspace_above_first: false, ..config };
            let mut world = World::single(config, geometry)?;
            world.fill_workspaces(count)?;
            let ids = focused_output(&world.snap()).map(workspace_ids).unwrap_or_default();
            let start = pick.index(count - 1);
            world.apply(Command::FocusWorkspaceAt(start));
            world.apply(Command::MoveWorkspaceDown);
            let mut expected = ids;
            expected.swap(start, start + 1);
            prop_assert_eq!(focused_output(&world.snap()).map(workspace_ids), Some(expected.clone()));
            world.apply(Command::FocusWorkspaceAt(start));
            prop_assert_eq!(focused_workspace(&world.snap()).map(|workspace| workspace.workspace), expected.get(start).copied());
        }

        #[test]
        fn nws_11_focusing_the_current_workspace_by_index_goes_back_or_does_nothing(
            config in config_strategy(),
            geometry in geometry_strategy(),
            count in 2_usize..5,
            first in any::<prop::sample::Index>(),
            second in any::<prop::sample::Index>(),
        ) {
            let back = config.workspace_auto_back_and_forth;
            let mut world = World::single(config, geometry)?;
            world.fill_workspaces(count)?;
            let ids = focused_output(&world.snap()).map(workspace_ids).unwrap_or_default();
            let (origin, destination) = (first.index(count), second.index(count));
            prop_assume!(origin != destination);
            world.apply(Command::FocusWorkspaceAt(origin));
            world.apply(Command::FocusWorkspaceAt(destination));
            world.apply(Command::FocusWorkspaceAt(destination));
            let expected = ids.get(if back { origin } else { destination }).copied();
            prop_assert_eq!(focused_workspace(&world.snap()).map(|workspace| workspace.workspace), expected);
        }

        #[test]
        fn nws_12_and_na_1_input_targets_the_destination_workspace_at_once(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            for step in &ops {
                if let Step::Run(command) = step {
                    world.apply(*command);
                    let snap = world.snap();
                    let workspace = focused_workspace(&snap).ok_or_else(|| TestCaseError::fail("no workspace"))?;
                    let target = snap.focused_window.and_then(|window| workspace_of(&snap, window)).map(|found| found.workspace);
                    prop_assert!(target.is_none() || target == Some(workspace.workspace));
                    prop_assert_eq!(snap.focused_window.is_none(), workspace.columns.is_empty());
                }
            }
        }
    }

    /// Position of the window's workspace on the focused output and of its column there.
    fn locate(snap: &Snapshot, window: WindowId) -> Option<(usize, usize)> {
        focused_output(snap)?
            .workspaces
            .iter()
            .enumerate()
            .find_map(|(position, workspace)| {
                let column = workspace
                    .columns
                    .iter()
                    .position(|column| column.tiles.iter().any(|tile| tile.window == window))?;
                Some((position, column))
            })
    }

    // NO: per-output behaviour.

    fn pair_strategy() -> impl Strategy<Value = (OutputGeometry, OutputGeometry, bool)> {
        (geometry_strategy(), geometry_strategy(), any::<bool>()).prop_map(|(first, second, horizontal)| {
            let second = if horizontal {
                geometry_at(first.area.width, 0.0, second.area.width, second.area.height, 0.0)
            } else {
                geometry_at(0.0, first.area.height, second.area.width, second.area.height, 0.0)
            };
            (first, second, horizontal)
        })
    }

    fn towards(from: OutputId, horizontal: bool) -> Direction {
        match (from == OutputId(0), horizontal) {
            (true, true) => Direction::Right,
            (false, true) => Direction::Left,
            (true, false) => Direction::Downward,
            (false, false) => Direction::Upward,
        }
    }

    fn other(output: OutputId) -> OutputId {
        if output == OutputId(0) {
            OutputId(1)
        } else {
            OutputId(0)
        }
    }

    fn active_ids(snap: &Snapshot) -> Vec<(OutputId, Option<WorkspaceId>)> {
        snap.outputs
            .iter()
            .map(|output| {
                (
                    output.output,
                    active_workspace(output).map(|workspace| workspace.workspace),
                )
            })
            .collect()
    }

    proptest! {
        #[test]
        fn no_1_no_2_no_4_focus_monitor_follows_global_positions_and_keeps_each_active_workspace(
            config in config_strategy(),
            (first, second, horizontal) in pair_strategy(),
            ops in ops_strategy(),
        ) {
            let mut world = World::with_outputs(config, &[first, second])?;
            world.run(&ops)?;
            let before = world.snap();
            let from = before.focused_output.ok_or_else(|| TestCaseError::fail("no output"))?;
            let sideways = if horizontal { Direction::Downward } else { Direction::Right };
            world.apply(Command::FocusMonitor(sideways));
            prop_assert_eq!(world.snap().focused_output, Some(from));
            world.apply(Command::FocusMonitor(towards(from, horizontal)));
            let after = world.snap();
            prop_assert_eq!(after.focused_output, Some(other(from)));
            prop_assert_eq!(active_ids(&after), active_ids(&before));
        }

        #[test]
        fn no_2_move_column_to_monitor_moves_it_to_that_outputs_active_workspace(
            config in config_strategy(),
            (first, second, horizontal) in pair_strategy(),
            ops in ops_strategy(),
        ) {
            let mut world = World::with_outputs(config, &[first, second])?;
            world.run(&ops)?;
            let window = world.open()?;
            let from = world.snap().focused_output.ok_or_else(|| TestCaseError::fail("no output"))?;
            world.apply(Command::MoveColumnToMonitor(towards(from, horizontal)));
            let snap = world.snap();
            let destination = snap.outputs.iter().find(|output| output.output == other(from)).and_then(active_workspace);
            prop_assert_eq!(workspace_of(&snap, window).map(|workspace| workspace.workspace), destination.map(|workspace| workspace.workspace));
        }

        #[test]
        fn no_3_move_workspace_to_monitor_moves_the_whole_workspace(
            config in config_strategy(),
            (first, second, horizontal) in pair_strategy(),
            ops in ops_strategy(),
        ) {
            let mut world = World::with_outputs(config, &[first, second])?;
            world.run(&ops)?;
            let window = world.open()?;
            let snap = world.snap();
            let from = snap.focused_output.ok_or_else(|| TestCaseError::fail("no output"))?;
            let workspace = workspace_of(&snap, window).cloned().ok_or_else(|| TestCaseError::fail("window lost"))?;
            world.apply(Command::MoveWorkspaceToMonitor(towards(from, horizontal)));
            let after = world.snap();
            prop_assert_eq!(output_of_workspace(&after, workspace.workspace), Some(other(from)));
            let moved = workspace_of(&after, window).map(|found| found.columns.iter().map(|column| column.tiles.len()).collect::<Vec<_>>());
            prop_assert_eq!(moved, Some(workspace.columns.iter().map(|column| column.tiles.len()).collect()));
        }

        #[test]
        fn no_5_and_no_7_workspaces_return_to_a_reconnected_output_with_its_active_one(
            config in config_strategy(),
            (first, second, horizontal) in pair_strategy(),
        ) {
            let mut world = World::with_outputs(config, &[first, second])?;
            if world.snap().focused_output != Some(OutputId(1)) {
                world.apply(Command::FocusMonitor(towards(OutputId(0), horizontal)));
            }
            world.fill_workspaces(2)?;
            // With NWS.5 the top workspace is empty, so focus the first one holding a window.
            let first_filled = focused_output(&world.snap())
                .and_then(|output| output.workspaces.iter().position(|workspace| !workspace.columns.is_empty()))
                .ok_or_else(|| TestCaseError::fail("no filled workspace"))?;
            world.apply(Command::FocusWorkspaceAt(first_filled));
            let before = world.snap();
            let output = before.outputs.iter().find(|output| output.output == OutputId(1)).cloned().ok_or_else(|| TestCaseError::fail("no output"))?;
            let kept: Vec<WorkspaceId> = output.workspaces.iter().filter(|workspace| !workspace.columns.is_empty()).map(|workspace| workspace.workspace).collect();
            world.layout.remove_output(OutputId(1))?;
            let gone = world.snap();
            prop_assert!(kept.iter().all(|workspace| output_of_workspace(&gone, *workspace) == Some(OutputId(0))));
            world.layout.add_output(OutputId(1), second, None)?;
            let back = world.snap();
            prop_assert!(kept.iter().all(|workspace| output_of_workspace(&back, *workspace) == Some(OutputId(1))));
            let active = back.outputs.iter().find(|candidate| candidate.output == OutputId(1)).and_then(active_workspace).map(|workspace| workspace.workspace);
            prop_assert_eq!(active, active_workspace(&output).map(|workspace| workspace.workspace));
        }

        #[test]
        fn no_5_and_no_7_the_remembered_active_workspace_is_the_one_by_identity(
            config in config_strategy(),
            (first, second, horizontal) in pair_strategy(),
            filled in 2_usize..5,
            pick in any::<prop::sample::Index>(),
        ) {
            let mut world = World::with_outputs(config, &[first, second])?;
            if world.snap().focused_output != Some(OutputId(1)) {
                world.apply(Command::FocusMonitor(towards(OutputId(0), horizontal)));
            }
            world.fill_workspaces(filled)?;
            let named = world.layout.add_named_workspace("keep".into(), Some(OutputId(1)))?;
            // Candidates: every populated workspace plus the empty named one, in output order.
            let focused = focused_output(&world.snap()).cloned().ok_or_else(|| TestCaseError::fail("no output"))?;
            let candidates: Vec<usize> = focused.workspaces.iter().enumerate()
                .filter(|(_, workspace)| !workspace.columns.is_empty() || workspace.workspace == named)
                .map(|(position, _)| position).collect();
            let position = candidates[pick.index(candidates.len())];
            world.apply(Command::FocusWorkspaceAt(position));
            let before = world.snap();
            let output = before.outputs.iter().find(|output| output.output == OutputId(1)).cloned().ok_or_else(|| TestCaseError::fail("no output"))?;
            let remembered = active_workspace(&output).map(|workspace| workspace.workspace);
            let kept: Vec<WorkspaceId> = output.workspaces.iter().filter(|workspace| !is_empty_unnamed(workspace)).map(|workspace| workspace.workspace).collect();
            prop_assert!(kept.contains(&named));
            world.layout.remove_output(OutputId(1))?;
            let gone = world.snap();
            prop_assert!(kept.iter().all(|workspace| output_of_workspace(&gone, *workspace) == Some(OutputId(0))), "NWS.6 named survives");
            world.layout.add_output(OutputId(1), second, None)?;
            let back = world.snap();
            prop_assert!(kept.iter().all(|workspace| output_of_workspace(&back, *workspace) == Some(OutputId(1))));
            let active = back.outputs.iter().find(|candidate| candidate.output == OutputId(1)).and_then(active_workspace).map(|workspace| workspace.workspace);
            prop_assert_eq!(active, remembered);
        }

        #[test]
        fn no_6_a_new_window_rehomes_its_workspace(
            config in config_strategy(),
            (first, second, horizontal) in pair_strategy(),
        ) {
            let mut world = World::with_outputs(config, &[first, second])?;
            if world.snap().focused_output != Some(OutputId(1)) {
                world.apply(Command::FocusMonitor(towards(OutputId(0), horizontal)));
            }
            let window = world.open()?;
            let workspace = workspace_of(&world.snap(), window).map(|found| found.workspace).ok_or_else(|| TestCaseError::fail("window lost"))?;
            world.layout.remove_output(OutputId(1))?;
            let _rehoming = world.open_with(WindowRules { open_on_output: Some(OutputId(0)), ..WindowRules::default() })?;
            let position = world.snap().outputs.iter().find(|output| output.output == OutputId(0))
                .and_then(|output| output.workspaces.iter().position(|found| found.workspace == workspace));
            world.apply(Command::FocusWorkspaceAt(position.ok_or_else(|| TestCaseError::fail("workspace lost"))?));
            let _new = world.open()?;
            let snap = world.snap();
            let original = snap.outputs.iter().flat_map(|output| &output.workspaces).find(|found| found.workspace == workspace).and_then(|found| found.original_output);
            prop_assert_eq!(original, Some(OutputId(0)));
            world.layout.add_output(OutputId(1), second, None)?;
            prop_assert_eq!(output_of_workspace(&world.snap(), workspace), Some(OutputId(0)));
        }

        #[test]
        fn no_8_a_per_output_layout_overrides_the_gaps(
            config in config_strategy(),
            override_gaps in 0.0_f64..48.0,
            (first, second, horizontal) in pair_strategy(),
        ) {
            let mut layout = Layout::new(config.clone());
            layout.add_output(OutputId(0), first, None)?;
            layout.add_output(OutputId(1), second, Some(LayoutConfig { gaps: override_gaps, ..config }))?;
            let mut world = World { layout, open: Vec::new(), next_window: 0 };
            if world.snap().focused_output != Some(OutputId(1)) {
                world.apply(Command::FocusMonitor(towards(OutputId(0), horizontal)));
            }
            world.open_many(2)?;
            world.apply(Command::CenterColumn);
            let snap = world.snap();
            let output = focused_output(&snap).ok_or_else(|| TestCaseError::fail("no output"))?;
            prop_assert!(near(output.gaps, override_gaps));
            let columns = &focused_workspace(&snap).ok_or_else(|| TestCaseError::fail("no workspace"))?.columns;
            let rects: Vec<Rect> = columns.iter().filter_map(column_rect).collect();
            prop_assert!(rects.len() == 2 && near(rects[1].x - (rects[0].x + rects[0].width), override_gaps));
        }

        #[test]
        fn no_9_proportions_use_the_working_area_of_their_own_output(
            config in proportion_config(1.0),
            (first, second, horizontal) in pair_strategy(),
        ) {
            let mut world = World::with_outputs(config, &[first, second])?;
            let _first = world.open()?;
            world.apply(Command::FocusMonitor(towards(world.snap().focused_output.unwrap_or(OutputId(0)), horizontal)));
            let _second = world.open()?;
            let snap = world.snap();
            for output in &snap.outputs {
                let tile = active_workspace(output).and_then(|workspace| workspace.columns.first()).and_then(column_rect);
                prop_assert!(tile.is_some_and(|tile| near(2.0f64.mul_add(output.gaps, tile.width), output.working_area.width)));
            }
        }
    }

    // NM: fullscreen and maximize.

    fn on_screen(rect: Rect, geometry: OutputGeometry) -> bool {
        rect.x < geometry.area.width
            && rect.x + rect.width > 0.0
            && rect.y < geometry.area.height
            && rect.y + rect.height > 0.0
    }

    #[test]
    fn fullscreen_left_column_puts_the_right_column_off_screen() -> Result<(), TestCaseError> {
        let geometry = OutputGeometry {
            area: Rect {
                x: 0.0,
                y: 0.0,
                width: 1000.0,
                height: 600.0,
            },
            usable: Rect {
                x: 0.0,
                y: 0.0,
                width: 1000.0,
                height: 600.0,
            },
        };
        let mut world = World::single(LayoutConfig::default(), geometry)?;
        let left = world.open()?;
        let right = world.open()?;
        world.focus_column(0);
        world.apply(Command::ToggleFullscreen);
        let snap = world.snap();
        let covering = tile_of(&snap, left).ok_or_else(|| TestCaseError::fail("left lost"))?;
        prop_assert!(near(covering.tile.x, 0.0) && near(covering.tile.width, 1000.0));
        let other = tile_of(&snap, right).ok_or_else(|| TestCaseError::fail("right lost"))?;
        prop_assert!(!on_screen(other.tile, geometry));
        world.apply(Command::ToggleFullscreen);
        let restored = world.snap();
        prop_assert!(tile_of(&restored, right).is_some_and(|tile| on_screen(tile.tile, geometry)));
        Ok(())
    }

    proptest! {
        #[test]
        fn nm_12_a_fullscreen_tile_is_the_only_visible_tile_on_its_output(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            let window = world.open()?;
            world.apply(Command::ToggleFullscreen);
            let snap = world.snap();
            for output in &snap.outputs {
                for workspace in output.workspaces.iter().skip(output.active_workspace).take(1) {
                    for tile in workspace.columns.iter().flat_map(|column| &column.tiles) {
                        if tile.mode != TileMode::MaximizedToEdges {
                            prop_assert_eq!(on_screen(tile.tile, geometry), tile.window == window);
                        }
                    }
                }
            }
        }

        #[test]
        fn nm_1_nm_2_fullscreen_requests_the_output_size_and_stays_in_the_strip(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            let window = world.open()?;
            let columns = focused_workspace(&world.snap()).map(|workspace| workspace.columns.len());
            world.apply(Command::ToggleFullscreen);
            let snap = world.snap();
            let tile = tile_of(&snap, window).cloned().ok_or_else(|| TestCaseError::fail("window lost"))?;
            prop_assert_eq!(tile.mode, TileMode::Fullscreen);
            prop_assert!(tile.fullscreen_requested);
            prop_assert!(near(tile.requested_client.width, geometry.area.width));
            prop_assert!(near(tile.requested_client.height, geometry.area.height));
            prop_assert_eq!(focused_workspace(&snap).map(|workspace| workspace.columns.len()), columns);
        }

        #[test]
        fn nm_2_and_nm_5_neighbours_stay_reachable_and_focusing_away_keeps_fullscreen(
            config in config_strategy(),
            geometry in geometry_strategy(),
        ) {
            let mut world = World::single(config, geometry)?;
            let neighbour = world.open()?;
            let window = world.open()?;
            world.apply(Command::ToggleFullscreen);
            world.focus_column(0);
            let snap = world.snap();
            prop_assert_eq!(snap.focused_window, Some(neighbour));
            prop_assert_eq!(tile_of(&snap, window).map(|tile| tile.mode), Some(TileMode::Fullscreen));
        }

        #[test]
        fn nm_7_maximize_to_edges_fills_the_usable_area(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            let window = world.open()?;
            world.apply(Command::ToggleMaximizeToEdges);
            let snap = world.snap();
            let tile = tile_of(&snap, window).cloned().ok_or_else(|| TestCaseError::fail("window lost"))?;
            let usable = geometry.usable;
            prop_assert_eq!(tile.mode, TileMode::MaximizedToEdges);
            prop_assert!(near(tile.tile.x, usable.x - geometry.area.x) && near(tile.tile.y, usable.y - geometry.area.y));
            prop_assert!(near(tile.tile.width, usable.width) && near(tile.tile.height, usable.height));
            prop_assert!(near(tile.requested_client.width, usable.width) && near(tile.requested_client.height, usable.height));
        }

        #[test]
        fn nm_9_open_rules_set_only_the_initial_state(
            config in config_strategy(),
            geometry in geometry_strategy(),
            rule in 0_u8..2,
        ) {
            let mut world = World::single(config, geometry)?;
            let rules = WindowRules { open_fullscreen: rule == 0, open_maximized_to_edges: rule == 1, ..WindowRules::default() };
            let window = world.open_with(rules)?;
            let expected = if rule == 0 { TileMode::Fullscreen } else { TileMode::MaximizedToEdges };
            prop_assert_eq!(tile_of(&world.snap(), window).map(|tile| tile.mode), Some(expected));
            world.apply(if rule == 0 { Command::ToggleFullscreen } else { Command::ToggleMaximizeToEdges });
            prop_assert_eq!(tile_of(&world.snap(), window).map(|tile| tile.mode), Some(TileMode::Normal));
        }

        #[test]
        fn nm_9_open_maximized_maximizes_the_column(
            config in config_strategy(),
            geometry in geometry_strategy(),
        ) {
            let mut world = World::single(config, geometry)?;
            let window = world.open_with(WindowRules { open_maximized: true, ..WindowRules::default() })?;
            let snap = world.snap();
            let output = focused_output(&snap).ok_or_else(|| TestCaseError::fail("no output"))?;
            prop_assert!(tile_of(&snap, window).is_some_and(|tile| near(2.0f64.mul_add(output.gaps, tile.tile.width), output.working_area.width)));
        }

        #[test]
        fn nm_10_windowed_fullscreen_keeps_the_tile_size(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            let window = world.open()?;
            let before = tile_of(&world.snap(), window).cloned().ok_or_else(|| TestCaseError::fail("window lost"))?;
            world.apply(Command::ToggleWindowedFullscreen);
            let after = tile_of(&world.snap(), window).cloned().ok_or_else(|| TestCaseError::fail("window lost"))?;
            prop_assert!(after.fullscreen_requested);
            prop_assert_eq!(after.mode, TileMode::WindowedFullscreen);
            prop_assert!(near(after.tile.width, before.tile.width) && near(after.tile.height, before.tile.height));
        }

        #[test]
        fn nm_11_leaving_fullscreen_restores_the_view(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let centring = centring_rules(&config);
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            let _window = world.open()?;
            let before = focused_workspace(&world.snap()).map(|workspace| workspace.view_offset);
            world.apply(Command::ToggleFullscreen);
            world.apply(Command::ToggleFullscreen);
            let snap = world.snap();
            restored_view(centring, &snap, before)?;
        }
    }

    // NN: new-window placement.

    proptest! {
        #[test]
        fn nn_2_a_new_window_always_opens_its_own_column(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            let window = world.open()?;
            let snap = world.snap();
            let workspace = workspace_of(&snap, window).ok_or_else(|| TestCaseError::fail("window lost"))?;
            let column = workspace.columns.iter().find(|column| column.tiles.iter().any(|tile| tile.window == window));
            prop_assert!(column.is_some_and(|column| column.tiles.len() == 1));
        }

        #[test]
        fn nn_3_rules_override_the_new_column_width_and_window_height(
            config in config_strategy(),
            geometry in geometry_strategy(),
            width in default_width_strategy(),
            height in (0.1_f64..1.0).prop_map(PresetSize::Proportion),
        ) {
            let mut world = World::single(config, geometry)?;
            let rules = WindowRules { default_column_width: Some(width), default_window_height: Some(height), ..WindowRules::default() };
            let window = world.open_with(rules)?;
            let snap = world.snap();
            prop_assert_eq!(focused_column(&snap).and_then(|column| column.width), width);
            let PresetSize::Proportion(proportion) = height else { return Err(TestCaseError::fail("height")) };
            prop_assert_eq!(tile_of(&snap, window).map(|tile| tile.height), Some(WindowHeight::Proportion(proportion)));
        }

        #[test]
        fn nn_4_open_focused_false_keeps_focus(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            let before = world.snap().focused_window;
            let window = world.open_with(WindowRules { open_focused: Some(false), ..WindowRules::default() })?;
            let snap = world.snap();
            prop_assert_eq!(snap.focused_window, before);
            prop_assert!(tile_of(&snap, window).is_some());
        }

        #[test]
        fn nn_4_and_nn_5_windows_opened_on_another_output_take_focus_only_when_forced(
            config in config_strategy(),
            (first, second, _horizontal) in pair_strategy(),
            forced in any::<bool>(),
        ) {
            let mut world = World::with_outputs(config, &[first, second])?;
            let _focused = world.open()?;
            let before = world.snap();
            let target = other(before.focused_output.ok_or_else(|| TestCaseError::fail("no output"))?);
            let rules = WindowRules { open_on_output: Some(target), open_focused: forced.then_some(true), ..WindowRules::default() };
            let window = world.open_with(rules)?;
            let snap = world.snap();
            let workspace = workspace_of(&snap, window).map(|found| found.workspace).ok_or_else(|| TestCaseError::fail("window lost"))?;
            prop_assert_eq!(output_of_workspace(&snap, workspace), Some(target));
            prop_assert_eq!(snap.focused_window, if forced { Some(window) } else { before.focused_window });
        }

        #[test]
        fn nn_5_open_targets_fall_back_to_the_focused_output(
            config in config_strategy(),
            (first, second, _horizontal) in pair_strategy(),
            missing_name in any::<bool>(),
        ) {
            let mut world = World::with_outputs(config, &[first, second])?;
            let focused = world.snap().focused_output.ok_or_else(|| TestCaseError::fail("no output"))?;
            let rules = if missing_name {
                WindowRules { open_on_workspace: Some("missing".into()), ..WindowRules::default() }
            } else {
                WindowRules { open_on_output: Some(OutputId(99)), ..WindowRules::default() }
            };
            let window = world.open_with(rules)?;
            let snap = world.snap();
            let workspace = workspace_of(&snap, window).map(|found| found.workspace).ok_or_else(|| TestCaseError::fail("window lost"))?;
            prop_assert_eq!(output_of_workspace(&snap, workspace), Some(focused));
        }

        #[test]
        fn nn_5_open_on_workspace_places_the_window_there(
            config in config_strategy(),
            (first, second, _horizontal) in pair_strategy(),
            on_other in any::<bool>(),
        ) {
            let mut world = World::with_outputs(config, &[first, second])?;
            let focused = world.snap().focused_output.ok_or_else(|| TestCaseError::fail("no output"))?;
            let named = world.layout.add_named_workspace("web".into(), Some(if on_other { other(focused) } else { focused }))?;
            let window = world.open_with(WindowRules { open_on_workspace: Some("web".into()), ..WindowRules::default() })?;
            prop_assert_eq!(workspace_of(&world.snap(), window).map(|found| found.workspace), Some(named));
        }

        #[test]
        fn nn_8_closing_a_window_to_the_left_keeps_the_focused_window_in_place(
            config in config_strategy(),
            geometry in geometry_strategy(),
            count in 3_usize..8,
            pick in any::<prop::sample::Index>(),
        ) {
            let mut world = World::single(config, geometry)?;
            let target = interior_column(&mut world, count, pick).ok_or_else(|| TestCaseError::fail("setup"))?;
            let before = world.snap();
            let left = focused_workspace(&before).and_then(|workspace| workspace.columns.get(target - 1))
                .and_then(|column| column.tiles.first()).map(|tile| tile.window).ok_or_else(|| TestCaseError::fail("no left column"))?;
            let x = focused_tile(&before).map(|tile| tile.tile.x);
            world.close(left)?;
            let after = world.snap();
            prop_assert_eq!(after.focused_window, before.focused_window);
            prop_assert!(matches!((x, focused_tile(&after).map(|tile| tile.tile.x)), (Some(old_x), Some(new_x)) if near(old_x, new_x)));
        }

        #[test]
        fn nn_9_closing_a_window_restores_the_view_from_when_it_opened(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
            focused in any::<bool>(),
        ) {
            let centring = centring_rules(&config);
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            let before = focused_workspace(&world.snap()).map(|workspace| (workspace.workspace, workspace.view_offset));
            prop_assume!(before.is_some_and(|_| world.snap().focused_window.is_some()));
            let window = world.open_with(WindowRules { open_focused: Some(focused), ..WindowRules::default() })?;
            world.close(window)?;
            let snap = world.snap();
            let after = focused_workspace(&snap).map(|workspace| workspace.workspace);
            prop_assert_eq!(before.map(|saved| saved.0), after);
            restored_view(centring, &snap, before.map(|saved| saved.1))?;
        }
    }

    /// The centring rules a config enables: NV.2 ("always") and NV.4 (lone column).
    fn centring_rules(config: &LayoutConfig) -> (bool, bool) {
        (
            config.center_focused_column == CenterFocusedColumn::Always,
            config.always_center_single_column,
        )
    }

    /// Whether NV.2 or NV.4 governs the view of the focused workspace in `snap`.
    fn centring_applies((always, single): (bool, bool), snap: &Snapshot) -> bool {
        always || (single && focused_workspace(snap).is_some_and(|workspace| workspace.columns.len() == 1))
    }

    /// After a restore the view is the centred one when NV.2 or NV.4 applies, else the saved one.
    fn restored_view(
        centring: (bool, bool),
        snap: &Snapshot,
        saved: Option<f64>,
    ) -> Result<(), TestCaseError> {
        if centring_applies(centring, snap) {
            let geometry = focused_geometry(snap)
                .filter(|_| focused_column(snap).is_some_and(column_is_normal))
                // A column wider than the working area is left-aligned per NV.1 [confirm]; any narrower
                // column, even one wider than the gap-inset area, is centred.
                .filter(|&(tile, working, _gaps)| tile.width <= working.width + EPSILON);
            if let Some((tile, working, _gaps)) = geometry {
                prop_assert!(
                    centred(tile, working),
                    "tile {:?} not centred in {:?}",
                    tile,
                    working
                );
            }
        } else {
            let after = focused_workspace(snap).map(|workspace| workspace.view_offset);
            prop_assert!(
                matches!((saved, after), (Some(saved), Some(after)) if near(saved, after)),
                "view {:?} -> {:?}",
                saved,
                after
            );
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn nv_4_a_lone_column_is_centred_after_close_or_unfullscreen(
            config in single_centred_config(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
            fullscreen in any::<bool>(),
        ) {
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            let snap = world.snap();
            let windows: Vec<WindowId> = all_tiles(&snap).map(|tile| tile.window).collect();
            prop_assume!(focused_workspace(&snap).is_some());
            let lone = world.open()?;
            if fullscreen {
                for window in windows {
                    world.close(window)?;
                }
                world.apply(Command::ToggleFullscreen);
                world.apply(Command::ToggleFullscreen);
            } else {
                let extra = world.open()?;
                for window in windows {
                    world.close(window)?;
                }
                world.close(extra)?;
            }
            let after = world.snap();
            prop_assume!(after.focused_window == Some(lone));
            prop_assume!(focused_workspace(&after).is_some_and(|workspace| workspace.columns.len() == 1));
            prop_assume!(focused_column(&after).is_some_and(column_is_normal));
            let (tile, working, gaps) = focused_geometry(&after).ok_or_else(|| TestCaseError::fail("nothing focused"))?;
            prop_assume!(fits(tile, working, gaps));
            prop_assert!(centred(tile, working), "tile {:?} in {:?}", tile, working);
        }
    }

    // NV: view scrolling and centering.

    fn focused_geometry(snap: &Snapshot) -> Option<(Rect, Rect, f64)> {
        let output = focused_output(snap)?;
        Some((focused_tile(snap)?.tile, output.working_area, output.gaps))
    }

    proptest! {
        #[test]
        fn nv_1_never_scrolls_the_minimum_to_the_nearest_edge(
            config in with_center(CenterFocusedColumn::Never),
            geometry in geometry_strategy(),
            widths in prop::collection::vec(0.1_f64..0.9, 2..8),
            pick in any::<prop::sample::Index>(),
            rightward in any::<bool>(),
        ) {
            let mut world = World::single(config, geometry)?;
            for width in &widths {
                let _window = world.open()?;
                world.apply(Command::SetColumnWidth(SizeChange::SetProportion(*width)));
            }
            world.focus_column(pick.index(widths.len()));
            let before = world.snap();
            let workspace = focused_workspace(&before).ok_or_else(|| TestCaseError::fail("no workspace"))?;
            prop_assume!(workspace.columns.iter().all(column_is_normal));
            // NV.1 governs focusing a column; an empty workspace has none.
            let index = workspace.focused_column.ok_or_else(|| TestCaseError::reject("no column"))?;
            let target = if rightward { index + 1 } else { index.checked_sub(1).ok_or_else(|| TestCaseError::reject("first"))? };
            let old = workspace.columns.get(target).and_then(column_rect).ok_or_else(|| TestCaseError::reject("last"))?;
            world.apply(if rightward { Command::FocusColumnRight } else { Command::FocusColumnLeft });
            let (tile, working, gaps) = focused_geometry(&world.snap()).ok_or_else(|| TestCaseError::fail("nothing focused"))?;
            // A column already fully visible is NV.6, which is [confirm].
            prop_assume!(fits(tile, working, gaps) && !fully_visible(old, working));
            let old_view = workspace.view_offset;
            let new_view = focused_workspace(&world.snap()).map(|found| found.view_offset).ok_or_else(|| TestCaseError::fail("no workspace"))?;
            prop_assert!(fully_visible(tile, working));
            let right_edge = working.x + working.width;
            // Minimum scroll: the column lands exactly one gap inside the nearest edge.
            let expected_x = if old.x + old.width > right_edge {
                right_edge - gaps - old.width
            } else {
                working.x + gaps
            };
            prop_assert!(near(tile.x, expected_x), "tile at {} expected {}", tile.x, expected_x);
            prop_assert!(near(new_view - old_view, old.x - expected_x), "view {} -> {}", old_view, new_view);
        }

        #[test]
        fn nv_2_always_centres_the_focused_column(
            config in with_center(CenterFocusedColumn::Always),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            // NW.11 [confirm]: exempt only a state where centring would push another fully visible
            // column out of view.
            // Exempt only when an expansion made the state (until focus or width changes) AND
            // centring would displace another fully visible column.
            let mut world = World::single(config, geometry)?;
            let mut expanded: Option<(Option<WindowId>, Option<PresetSize>)> = None;
            for step in &ops {
                world.run(core::slice::from_ref(step))?;
                let snap = world.snap();
                let key = (snap.focused_window, focused_column(&snap).and_then(|column| column.width));
                if matches!(step, Step::Run(Command::ExpandColumnToAvailableWidth)) {
                    expanded = Some(key);
                } else if expanded.is_some_and(|held| held != key) {
                    expanded = None;
                }
                let exempt = expanded.is_some() && displaces_on_centring(&snap);
                if exempt || !focused_column(&snap).is_some_and(column_is_normal) {
                    continue;
                }
                let Some((tile, working, gaps)) = focused_geometry(&snap) else { continue };
                if fits(tile, working, gaps) {
                    prop_assert!(centred(tile, working), "after {:?}", step);
                }
            }
        }

        #[test]
        fn nv_3_on_overflow_centres_only_when_the_previous_column_cannot_stay_visible(
            config in with_center(CenterFocusedColumn::OnOverflow),
            geometry in geometry_strategy(),
            widths in prop::collection::vec(0.1_f64..0.9, 2..8),
            pick in any::<prop::sample::Index>(),
        ) {
            let mut world = World::single(config, geometry)?;
            for width in &widths {
                let _window = world.open()?;
                world.apply(Command::SetColumnWidth(SizeChange::SetProportion(*width)));
            }
            world.focus_column(pick.index(widths.len() - 1));
            let start = world.snap();
            let previous = start.focused_window;
            let index = focused_column_index(&start).ok_or_else(|| TestCaseError::fail("no column"))?;
            let target_before = focused_workspace(&start).and_then(|workspace| workspace.columns.get(index + 1)).and_then(column_rect)
                .ok_or_else(|| TestCaseError::fail("no right column"))?;
            let old_view = focused_workspace(&start).map_or(0.0, |workspace| workspace.view_offset);
            world.apply(Command::FocusColumnRight);
            let snap = world.snap();
            let (tile, working, gaps) = focused_geometry(&snap).ok_or_else(|| TestCaseError::fail("nothing focused"))?;
            let old = previous.and_then(|window| tile_of(&snap, window)).map(|found| found.tile).ok_or_else(|| TestCaseError::fail("window lost"))?;
            prop_assume!(fits(tile, working, gaps));
            if 3.0f64.mul_add(gaps, old.width + tile.width) <= working.width + EPSILON {
                prop_assert!(fully_visible(tile, working) && fully_visible(old, working));
                // Behaves like "never": the minimum scroll to the right edge (a visible target is NV.6 [confirm]).
                if !fully_visible(target_before, working) {
                let expected_x = working.x + working.width - gaps - tile.width;
                prop_assert!(near(tile.x, expected_x), "tile at {} expected {}", tile.x, expected_x);
                let new_view = focused_workspace(&snap).map_or(0.0, |workspace| workspace.view_offset);
                prop_assert!(near(new_view - old_view, target_before.x - expected_x));
                }
            } else {
                prop_assert!(centred(tile, working));
            }
        }

        #[test]
        fn nv_4_a_single_column_is_centred_when_configured(
            config in single_centred_config(),
            geometry in geometry_strategy(),
        ) {
            let mut world = World::single(config, geometry)?;
            let _window = world.open()?;
            let (tile, working, gaps) = focused_geometry(&world.snap()).ok_or_else(|| TestCaseError::fail("nothing focused"))?;
            prop_assume!(fits(tile, working, gaps));
            prop_assert!(centred(tile, working));
        }

        #[test]
        fn nv_5_center_column_centres_the_focused_column(
            config in config_strategy(),
            geometry in geometry_strategy(),
            ops in ops_strategy(),
        ) {
            let mut world = World::single(config, geometry)?;
            world.run(&ops)?;
            world.apply(Command::CenterColumn);
            let snap = world.snap();
            prop_assume!(focused_column(&snap).is_some_and(column_is_normal));
            let (tile, working, gaps) = focused_geometry(&snap).ok_or_else(|| TestCaseError::reject("nothing focused"))?;
            prop_assume!(fits(tile, working, gaps));
            prop_assert!(centred(tile, working));
        }
    }

    // Detached workspaces and float boundaries (ADR 0001: invalid input never reaches the model).

    #[test]
    fn removing_a_window_while_every_output_is_gone_succeeds() -> Result<(), LayoutError> {
        let mut world = World::single(
            LayoutConfig::default(),
            geometry_at(0.0, 0.0, 1920.0, 1080.0, 0.0),
        )?;
        let kept = world.open()?;
        let closed = world.open()?;
        world.layout.remove_output(OutputId(0))?;
        assert_eq!(world.layout.remove_window(closed), Ok(()));
        assert_eq!(
            world.layout.remove_window(closed),
            Err(LayoutError::UnknownWindow(closed))
        );
        world
            .layout
            .add_output(OutputId(0), geometry_at(0.0, 0.0, 1920.0, 1080.0, 0.0), None)?;
        let snap = world.snap();
        assert!(tile_of(&snap, closed).is_none());
        assert!(tile_of(&snap, kept).is_some());
        Ok(())
    }

    /// Shrunk NV.2 counterexample: after closes, the NN.9 restored view leaves the focused
    /// column off centre under "always".
    #[test]
    fn nv_2_replay_close_restores_a_stale_view_under_always() -> Result<(), LayoutError> {
        let config = LayoutConfig {
            gaps: 0.0,
            center_focused_column: CenterFocusedColumn::Always,
            ..LayoutConfig::default()
        };
        let mut world = World::single(config, geometry_at(0.0, 0.0, 3_179.118_585_605_785, 600.0, 0.0))?;
        let ops = [
            Step::Open,
            Step::Open,
            Step::Open,
            Step::Close(0),
            Step::Open,
            Step::Close(0),
            Step::Open,
            Step::Open,
            Step::Open,
            Step::Open,
            Step::Close(1_192_730_189_438_836_716),
            Step::Close(10_320_365_627_896_110_324),
        ];
        world.run(&ops)?;
        let snap = world.snap();
        let (tile, working, _gaps) = focused_geometry(&snap).ok_or(LayoutError::NoOutput)?;
        assert!(
            centred(tile, working),
            "focused tile {tile:?} not centred in {working:?}"
        );
        Ok(())
    }

    fn bad_float() -> impl Strategy<Value = f64> {
        prop_oneof![
            Just(f64::NAN),
            Just(f64::INFINITY),
            Just(f64::NEG_INFINITY),
            Just(0.0),
            (-5000.0_f64..-0.001),
        ]
    }

    fn all_finite(snap: &Snapshot) -> bool {
        snap.outputs.iter().all(|output| {
            let rect_ok = |rect: Rect| {
                rect.x.is_finite() && rect.y.is_finite() && rect.width.is_finite() && rect.height.is_finite()
            };
            rect_ok(output.working_area)
                && output
                    .workspaces
                    .iter()
                    .all(|workspace| workspace.view_offset.is_finite())
                && all_tiles(snap).all(|tile| {
                    rect_ok(tile.tile)
                        && tile.requested_client.width.is_finite()
                        && tile.requested_client.height.is_finite()
                })
        })
    }

    proptest! {
        #[test]
        fn output_geometry_with_a_bad_size_is_rejected(
            bad in bad_float(),
            on_width in any::<bool>(),
        ) {
            let (width, height) = if on_width { (bad, 1080.0) } else { (1920.0, bad) };
            let mut layout = Layout::new(LayoutConfig::default());
            prop_assert!(layout.add_output(OutputId(0), geometry_at(0.0, 0.0, width, height, 0.0), None).is_err());
            prop_assert!(layout.snapshot().outputs.is_empty());
        }

        #[test]
        fn struts_that_leave_no_working_width_are_rejected(
            width in 800.0_f64..3840.0,
            extra in 0.0_f64..400.0,
        ) {
            let struts = Struts { left: width / 2.0, right: width / 2.0 + extra, top: 0.0, bottom: 0.0 };
            let mut layout = Layout::new(LayoutConfig { struts, ..LayoutConfig::default() });
            prop_assert!(layout.add_output(OutputId(0), geometry_at(0.0, 0.0, width, 1080.0, 0.0), None).is_err());
        }

        #[test]
        fn a_non_finite_or_negative_config_is_rejected(
            bad in bad_float(),
            field in 0_u8..4,
        ) {
            prop_assume!(!(bad == 0.0 && field < 2), "zero gaps and borders are valid");
            let mut config = LayoutConfig::default();
            match field {
                0 => config.gaps = bad,
                1 => config.border = bad,
                2 => config.default_column_width = Some(PresetSize::Proportion(bad)),
                _ => config.preset_column_widths = vec![PresetSize::Fixed(bad)],
            }
            prop_assert!(Layout::try_new(config).is_err());
        }

        #[test]
        fn bad_size_changes_leave_the_layout_unchanged_and_finite(
            bad in bad_float(),
            ops in ops_strategy(),
            which in 0_u8..6,
        ) {
            let mut world = World::single(LayoutConfig::default(), geometry_at(0.0, 0.0, 1920.0, 1080.0, 0.0))?;
            world.run(&ops)?;
            let _window = world.open()?;
            let before = world.snap();
            let command = match which {
                0 => Command::SetColumnWidth(SizeChange::SetProportion(bad)),
                1 => Command::SetColumnWidth(SizeChange::SetFixed(bad)),
                2 => Command::SetWindowHeight(SizeChange::SetProportion(bad)),
                3 => Command::SetWindowHeight(SizeChange::SetFixed(bad)),
                4 => Command::SetColumnWidth(SizeChange::AdjustFixed(if bad.is_finite() { f64::NAN } else { bad })),
                _ => Command::SetWindowHeight(SizeChange::AdjustPercent(if bad.is_finite() { f64::NAN } else { bad })),
            };
            world.apply(command);
            let after = world.snap();
            prop_assert!(all_finite(&after));
            prop_assert_eq!(after, before);
        }

        #[test]
        fn bad_window_rules_are_rejected(
            bad in bad_float(),
            which in 0_u8..3,
        ) {
            let mut world = World::single(LayoutConfig::default(), geometry_at(0.0, 0.0, 1920.0, 1080.0, 0.0))?;
            let rules = match which {
                0 => WindowRules { min_height: Some(bad), ..WindowRules::default() },
                1 => WindowRules { max_height: Some(bad), ..WindowRules::default() },
                _ => WindowRules { default_column_width: Some(Some(PresetSize::Fixed(bad))), ..WindowRules::default() },
            };
            prop_assert!(world.open_with(rules).is_err());
            prop_assert!(all_finite(&world.snap()));
        }
    }
}
