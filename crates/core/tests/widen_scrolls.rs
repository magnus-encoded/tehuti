//! A width change scrolls the view so the focused column stays on the output.

#[cfg(test)]
mod widen_scrolls {
    use tehuti_core::layout::{
        Command, Layout, LayoutConfig, OutputGeometry, OutputId, Rect, WindowId, WindowRules,
    };

    const OUTPUT: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };

    fn focused_tile(layout: &Layout) -> Rect {
        let snapshot = layout.snapshot();
        let focused = snapshot.focused_window.unwrap();
        snapshot
            .outputs
            .iter()
            .flat_map(|output| &output.workspaces)
            .flat_map(|workspace| &workspace.columns)
            .flat_map(|column| &column.tiles)
            .find(|tile| tile.window == focused)
            .unwrap()
            .tile
    }

    #[test]
    fn widening_the_middle_of_three_columns_keeps_it_inside_the_output() {
        let mut layout = Layout::new(LayoutConfig::default());
        layout
            .add_output(
                OutputId(0),
                OutputGeometry {
                    area: OUTPUT,
                    usable: OUTPUT,
                },
                None,
            )
            .unwrap();
        for window in 0..3 {
            layout
                .add_window(WindowId(window), WindowRules::default())
                .unwrap();
        }
        layout.apply(Command::FocusColumnLeft);
        for _ in 0..4 {
            layout.apply(Command::SwitchPresetColumnWidth);
            let tile = focused_tile(&layout);
            assert!(tile.x >= OUTPUT.x, "{tile:?}");
            assert!(tile.x + tile.width <= OUTPUT.x + OUTPUT.width, "{tile:?}");
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::float_arithmetic,
    reason = "layout geometry is fractional logical pixels"
)]
mod widen_scrolls_laws {
    use proptest::prelude::*;
    use tehuti_core::layout::{
        CenterFocusedColumn, Command, Layout, LayoutConfig, OutputGeometry, OutputId, PresetSize, Rect,
        Struts, WindowId, WindowRules,
    };

    fn preset() -> impl Strategy<Value = PresetSize> {
        prop_oneof![
            (0.05_f64..1.0).prop_map(PresetSize::Proportion),
            (50.0_f64..2500.0).prop_map(PresetSize::Fixed),
        ]
    }

    fn center() -> impl Strategy<Value = CenterFocusedColumn> {
        prop_oneof![
            Just(CenterFocusedColumn::Never),
            Just(CenterFocusedColumn::Always),
            Just(CenterFocusedColumn::OnOverflow),
        ]
    }

    fn width_command() -> impl Strategy<Value = Command> {
        prop_oneof![
            Just(Command::SwitchPresetColumnWidth),
            Just(Command::SwitchPresetColumnWidthBack),
            Just(Command::FocusColumnLeft),
            Just(Command::FocusColumnRight),
        ]
    }

    proptest! {
        #[test]
        fn a_width_change_leaves_the_focused_column_visible(
            gaps in 0.0_f64..32.0,
            border in 0.0_f64..8.0,
            left in 0.0_f64..200.0,
            right in 0.0_f64..200.0,
            presets in prop::collection::vec(preset(), 1..5),
            default_width in prop::option::of(preset()),
            center in center(),
            single in any::<bool>(),
            client_widths in prop::collection::vec(50.0_f64..2500.0, 1..6),
            commands in prop::collection::vec(width_command(), 1..12),
        ) {
            let mut layout = Layout::try_new(LayoutConfig {
                gaps,
                border,
                struts: Struts { left, right, top: 0.0, bottom: 0.0 },
                preset_column_widths: presets,
                default_column_width: default_width,
                center_focused_column: center,
                always_center_single_column: single,
                ..LayoutConfig::default()
            }).unwrap();
            let area = Rect { x: 0.0, y: 0.0, width: 1920.0, height: 1080.0 };
            layout.add_output(OutputId(0), OutputGeometry { area, usable: area }, None).unwrap();
            for (window, width) in (0..).zip(&client_widths) {
                layout.add_window(WindowId(window), WindowRules::default()).unwrap();
                layout.report_client_width(WindowId(window), *width).unwrap();
            }
            for command in commands {
                layout.apply(command);
                let snapshot = layout.snapshot();
                let output = &snapshot.outputs[0];
                let working = output.working_area;
                let workspace = &output.workspaces[output.active_workspace];
                let column = &workspace.columns[workspace.focused_column.unwrap()];
                let tile = column.tiles[column.focused_tile].tile;
                prop_assert!(tile.x >= working.x - 1e-6, "{command:?} {tile:?} {working:?}");
                if tile.width <= 2.0_f64.mul_add(-gaps, working.width) {
                    prop_assert!(
                        tile.x + tile.width <= working.x + working.width + 1e-6,
                        "{command:?} {tile:?} {working:?}"
                    );
                }
            }
        }
    }
}
