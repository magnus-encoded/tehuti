#[cfg(test)]
mod layout_wiring {
    use tehuti_core::layout::{
        CenterFocusedColumn, Command, Direction, Layout, LayoutConfig, LayoutError, OutputGeometry, OutputId,
        PresetSize, Rect, WindowId, WindowRules,
    };

    fn geometry(width: f64) -> OutputGeometry {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width,
            height: 600.0,
        };
        OutputGeometry { area, usable: area }
    }

    #[test]
    fn widening_a_third_column_settles_before_a_focus_round_trip() -> Result<(), LayoutError> {
        let mut layout = Layout::try_new(LayoutConfig {
            gaps: 10.0,
            border: 0.0,
            center_focused_column: CenterFocusedColumn::OnOverflow,
            always_center_single_column: false,
            default_column_width: Some(PresetSize::Proportion(1.0 / 3.0)),
            preset_column_widths: vec![PresetSize::Proportion(1.0 / 3.0), PresetSize::Proportion(0.75)],
            ..LayoutConfig::default()
        })?;
        layout.add_output(OutputId(0), geometry(1200.0), None)?;
        layout.add_window(WindowId(0), WindowRules::default())?;
        layout.add_window(WindowId(1), WindowRules::default())?;
        layout.apply(Command::SwitchPresetColumnWidth);
        let widened = layout.snapshot().outputs[0].workspaces[0].view_offset;
        layout.report_client_width(WindowId(1), 885.0)?;
        assert_eq!(layout.snapshot().outputs[0].workspaces[0].view_offset, widened);
        layout.apply(Command::FocusColumnLeft);
        layout.apply(Command::FocusColumnRight);
        assert_eq!(layout.snapshot().outputs[0].workspaces[0].view_offset, widened);
        Ok(())
    }

    #[test]
    fn committed_width_settles_before_a_focus_round_trip() -> Result<(), LayoutError> {
        let mut layout = Layout::try_new(LayoutConfig {
            gaps: 10.0,
            border: 0.0,
            center_focused_column: CenterFocusedColumn::OnOverflow,
            always_center_single_column: false,
            default_column_width: None,
            ..LayoutConfig::default()
        })?;
        layout.add_output(OutputId(0), geometry(1200.0), None)?;
        layout.add_window(WindowId(0), WindowRules::default())?;
        layout.report_client_width(WindowId(0), 400.0)?;
        layout.add_window(WindowId(1), WindowRules::default())?;
        layout.report_client_width(WindowId(1), 400.0)?;
        layout.report_client_width(WindowId(1), 885.0)?;
        let widened = layout.snapshot().outputs[0].workspaces[0].view_offset;
        layout.apply(Command::FocusColumnLeft);
        layout.apply(Command::FocusColumnRight);
        assert_eq!(layout.snapshot().outputs[0].workspaces[0].view_offset, widened);
        Ok(())
    }

    #[test]
    fn smaller_clients_are_centred_inside_their_slot() -> Result<(), LayoutError> {
        let mut layout = Layout::try_new(LayoutConfig {
            gaps: 0.0,
            border: 0.0,
            ..LayoutConfig::default()
        })?;
        layout.add_output(OutputId(0), geometry(1200.0), None)?;
        layout.add_window(WindowId(0), WindowRules::default())?;
        let snapshot = layout.snapshot();
        let tile = &snapshot.outputs[0].workspaces[0].columns[0].tiles[0];
        assert_eq!(
            tile.place_client(
                tehuti_core::layout::Size {
                    width: 200.0,
                    height: 100.0
                },
                0.0
            ),
            Rect {
                x: 100.0,
                y: 250.0,
                width: 200.0,
                height: 100.0
            }
        );
        Ok(())
    }

    #[test]
    fn committed_client_width_places_the_next_column() -> Result<(), LayoutError> {
        let mut layout = Layout::try_new(LayoutConfig {
            gaps: 10.0,
            default_column_width: None,
            ..LayoutConfig::default()
        })?;
        layout.add_output(OutputId(0), geometry(1200.0), None)?;
        layout.add_window(WindowId(0), WindowRules::default())?;
        layout.report_client_width(WindowId(0), 300.0)?;
        layout.add_window(WindowId(1), WindowRules::default())?;
        let snap = layout.snapshot();
        let columns = &snap.outputs[0].workspaces[0].columns;
        assert_eq!(columns[0].width, None);
        assert_eq!(columns[0].tiles[0].requested_client.width, 0.0);
        assert_eq!(columns[1].tiles[0].tile.x - columns[0].tiles[0].tile.x, 310.0);
        let before = layout.snapshot();
        assert!(layout.report_client_width(WindowId(0), f64::NAN).is_err());
        assert_eq!(layout.snapshot(), before);
        Ok(())
    }

    #[test]
    fn output_resize_preserves_workspaces_windows_and_focus() -> Result<(), LayoutError> {
        let mut layout = Layout::try_new(LayoutConfig::default())?;
        layout.add_output(OutputId(0), geometry(1200.0), None)?;
        layout.add_window(WindowId(0), WindowRules::default())?;
        let before = layout.snapshot();
        layout.update_output(OutputId(0), geometry(1600.0))?;
        let after = layout.snapshot();
        assert_eq!(
            after.outputs[0].workspaces[0].workspace,
            before.outputs[0].workspaces[0].workspace
        );
        assert_eq!(after.focused_window, before.focused_window);
        assert!(
            after.outputs[0].workspaces[0].columns[0].tiles[0].tile.width
                > before.outputs[0].workspaces[0].columns[0].tiles[0].tile.width
        );
        assert!(
            layout
                .update_output(OutputId(0), geometry(f64::INFINITY))
                .is_err()
        );
        assert_eq!(layout.snapshot(), after);
        Ok(())
    }

    #[test]
    fn focusing_by_id_selects_the_windows_output_and_workspace() -> Result<(), LayoutError> {
        let mut layout = Layout::try_new(LayoutConfig::default())?;
        layout.add_output(OutputId(0), geometry(1200.0), None)?;
        layout.add_output(OutputId(1), geometry(1200.0), None)?;
        layout.add_window(
            WindowId(0),
            WindowRules {
                open_on_output: Some(OutputId(1)),
                ..WindowRules::default()
            },
        )?;
        layout.add_window(WindowId(1), WindowRules::default())?;
        layout.focus_window(WindowId(0))?;
        assert_eq!(layout.snapshot().focused_output, Some(OutputId(1)));
        assert_eq!(layout.snapshot().focused_window, Some(WindowId(0)));
        let before = layout.snapshot();
        assert_eq!(
            layout.focus_window(WindowId(99)),
            Err(LayoutError::UnknownWindow(WindowId(99)))
        );
        assert_eq!(layout.snapshot(), before);
        Ok(())
    }

    const fn placed(x: f64, y: f64) -> OutputGeometry {
        let area = Rect {
            x,
            y,
            width: 800.0,
            height: 600.0,
        };
        OutputGeometry { area, usable: area }
    }

    #[test]
    fn monitor_focus_prefers_the_output_aligned_on_the_other_axis() -> Result<(), LayoutError> {
        let mut layout = Layout::try_new(LayoutConfig::default())?;
        layout.add_output(OutputId(0), placed(0.0, 0.0), None)?;
        layout.add_output(OutputId(1), placed(800.0, 5000.0), None)?;
        layout.add_output(OutputId(2), placed(800.0, 0.0), None)?;
        layout.apply(Command::FocusMonitor(Direction::Right));
        assert_eq!(layout.snapshot().focused_output, Some(OutputId(2)));
        Ok(())
    }

    #[test]
    fn removing_a_window_from_a_detached_workspace_forgets_it() -> Result<(), LayoutError> {
        let mut layout = Layout::try_new(LayoutConfig::default())?;
        layout.add_output(OutputId(0), geometry(1200.0), None)?;
        layout.add_window(WindowId(0), WindowRules::default())?;
        layout.add_window(WindowId(1), WindowRules::default())?;
        layout.remove_output(OutputId(0))?;
        layout.remove_window(WindowId(0))?;
        assert_eq!(
            layout.remove_window(WindowId(0)),
            Err(LayoutError::UnknownWindow(WindowId(0)))
        );
        layout.add_output(OutputId(0), geometry(1200.0), None)?;
        assert_eq!(layout.snapshot().focused_window, Some(WindowId(1)));
        Ok(())
    }

    #[test]
    fn shrinking_an_output_keeps_the_focused_column_in_view() -> Result<(), LayoutError> {
        let mut layout = Layout::try_new(LayoutConfig::default())?;
        layout.add_output(OutputId(0), geometry(1600.0), None)?;
        for id in 0..4 {
            layout.add_window(WindowId(id), WindowRules::default())?;
        }
        layout.update_output(OutputId(0), geometry(500.0))?;
        let snapshot = layout.snapshot();
        let workspace = &snapshot.outputs[0].workspaces[0];
        let focused = workspace
            .focused_column
            .and_then(|column| workspace.columns.get(column))
            .and_then(|column| column.tiles.get(column.focused_tile));
        assert!(focused.is_some_and(|tile| tile.tile.x >= 0.0 && tile.tile.x < 500.0));
        Ok(())
    }
}
