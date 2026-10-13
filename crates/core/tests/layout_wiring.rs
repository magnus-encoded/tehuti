#[cfg(test)]
mod layout_wiring {
    use tehuti_core::layout::{
        Layout, LayoutConfig, LayoutError, OutputGeometry, OutputId, Rect, WindowId, WindowRules,
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
}
