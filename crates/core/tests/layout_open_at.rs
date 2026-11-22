//! `WindowRules::open_at` places a window without taking focus or changing the workspace.

#[cfg(test)]
mod layout_open_at {
    use tehuti_core::layout::{
        Command, Layout, LayoutConfig, OpenAt, OutputGeometry, OutputId, Rect, WindowId, WindowRules,
    };

    fn layout_with_two_workspaces() -> Layout {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        let mut layout = Layout::new(LayoutConfig::default());
        layout
            .add_output(OutputId(0), OutputGeometry { area, usable: area }, None)
            .expect("output");
        layout.add_window(WindowId(1), WindowRules::default()).expect("one");
        layout.add_window(WindowId(2), WindowRules::default()).expect("two");
        layout.apply(Command::FocusWorkspaceDown);
        layout.add_window(WindowId(3), WindowRules::default()).expect("three");
        layout.apply(Command::FocusWorkspaceUp);
        layout
    }

    fn placed(workspace: usize, column: usize) -> WindowRules {
        WindowRules {
            open_at: Some(OpenAt { workspace, column }),
            ..WindowRules::default()
        }
    }

    fn columns(layout: &Layout, workspace: usize) -> Vec<u64> {
        layout.snapshot().outputs[0].workspaces[workspace]
            .columns
            .iter()
            .map(|column| column.tiles[0].window.0)
            .collect()
    }

    #[test]
    fn open_at_inserts_at_the_column_and_keeps_focus() {
        let mut layout = layout_with_two_workspaces();
        let before = layout.snapshot();
        layout.add_window(WindowId(10), placed(0, 1)).expect("placed");
        let after = layout.snapshot();
        assert_eq!(columns(&layout, 0), vec![1, 10, 2]);
        assert_eq!(after.focused_window, before.focused_window);
        assert_eq!(after.outputs[0].active_workspace, before.outputs[0].active_workspace);
    }

    #[test]
    fn open_at_can_target_another_workspace_without_switching() {
        let mut layout = layout_with_two_workspaces();
        let before = layout.snapshot();
        layout.add_window(WindowId(10), placed(1, 0)).expect("placed");
        assert_eq!(columns(&layout, 1)[0], 10);
        assert_eq!(
            layout.snapshot().outputs[0].active_workspace,
            before.outputs[0].active_workspace
        );
        assert_eq!(layout.snapshot().focused_window, before.focused_window);
    }

    #[test]
    fn open_at_clamps_to_what_exists() {
        let mut layout = layout_with_two_workspaces();
        layout.add_window(WindowId(10), placed(0, 99)).expect("column");
        assert_eq!(columns(&layout, 0), vec![1, 2, 10]);
        layout.add_window(WindowId(11), placed(99, 0)).expect("workspace");
        let all: Vec<u64> = layout.snapshot().outputs[0]
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.columns.iter().map(|column| column.tiles[0].window.0))
            .collect();
        assert!(all.contains(&11));
    }
}
