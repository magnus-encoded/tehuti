//! Helpers retain workspace membership without participating in columns.
#[cfg(test)]
mod layout_helpers {
    use tehuti_core::layout::{
        Command, Layout, LayoutConfig, LayoutError, OutputGeometry, OutputId, Rect, WindowId, WindowKind,
        WindowRules,
    };

    fn layout() -> Result<Layout, LayoutError> {
        let mut layout = Layout::try_new(LayoutConfig::default())?;
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width: 1200.0,
            height: 800.0,
        };
        layout.add_output(OutputId(0), OutputGeometry { area, usable: area }, None)?;
        Ok(layout)
    }

    #[test]
    fn helpers_can_be_clicked_but_never_take_a_width_preset() -> Result<(), LayoutError> {
        let mut layout = layout()?;
        layout.add_window(WindowId(0), WindowRules::default())?;
        layout.add_window(
            WindowId(1),
            WindowRules {
                kind: WindowKind::Helper,
                ..WindowRules::default()
            },
        )?;
        let before = layout.snapshot();
        assert_eq!(before.focused_window, Some(WindowId(0)));
        assert_eq!(before.outputs[0].workspaces[0].columns.len(), 1);
        assert_eq!(before.outputs[0].workspaces[0].helpers, vec![WindowId(1)]);
        layout.focus_window(WindowId(1))?;
        let clicked = layout.snapshot();
        assert_eq!(clicked.focused_window, Some(WindowId(1)));
        layout.apply(Command::SwitchPresetColumnWidth);
        assert_eq!(layout.snapshot(), clicked);
        layout.apply(Command::FocusColumnRight);
        assert_eq!(layout.snapshot(), before);
        assert!(layout.add_window(WindowId(1), WindowRules::default()).is_err());
        Ok(())
    }

    #[test]
    fn promoted_helpers_follow_the_last_column_in_their_own_workspace() -> Result<(), LayoutError> {
        let mut layout = layout()?;
        for identity in 0..3 {
            layout.add_window(WindowId(identity), WindowRules::default())?;
        }
        layout.add_window(
            WindowId(3),
            WindowRules {
                kind: WindowKind::Helper,
                ..WindowRules::default()
            },
        )?;
        layout.focus_window(WindowId(0))?;
        layout.apply(Command::FocusWorkspaceDown);
        assert!(layout.set_helper(WindowId(3), false)?);
        let snapshot = layout.snapshot();
        assert_eq!(snapshot.outputs[0].active_workspace, 1);
        let workspace = &snapshot.outputs[0].workspaces[0];
        assert!(workspace.helpers.is_empty());
        let order: Vec<_> = workspace
            .columns
            .iter()
            .flat_map(|column| &column.tiles)
            .map(|tile| tile.window)
            .collect();
        assert_eq!(order, vec![WindowId(0), WindowId(1), WindowId(2), WindowId(3)]);
        layout.apply(Command::FocusWorkspaceUp);
        assert_eq!(layout.snapshot().focused_window, Some(WindowId(0)));
        layout.apply(Command::FocusColumnLast);
        assert_eq!(layout.snapshot().focused_window, Some(WindowId(3)));
        Ok(())
    }

    #[test]
    fn helper_only_workspaces_survive_output_disconnect_and_close() -> Result<(), LayoutError> {
        let mut layout = layout()?;
        layout.add_window(
            WindowId(1),
            WindowRules {
                kind: WindowKind::Helper,
                ..WindowRules::default()
            },
        )?;
        let workspace_id = layout.snapshot().outputs[0].workspaces[0].workspace;
        layout.apply(Command::FocusWorkspaceDown);
        layout.remove_output(OutputId(0))?;
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width: 1200.0,
            height: 800.0,
        };
        layout.add_output(OutputId(0), OutputGeometry { area, usable: area }, None)?;
        let snapshot = layout.snapshot();
        assert_eq!(snapshot.outputs[0].workspaces[0].workspace, workspace_id);
        assert_eq!(snapshot.outputs[0].workspaces[0].helpers, vec![WindowId(1)]);
        layout.remove_output(OutputId(0))?;
        layout.remove_window(WindowId(1))?;
        layout.add_output(OutputId(0), OutputGeometry { area, usable: area }, None)?;
        assert!(
            layout.snapshot().outputs[0]
                .workspaces
                .iter()
                .all(|workspace| workspace.helpers.is_empty())
        );
        Ok(())
    }
}
