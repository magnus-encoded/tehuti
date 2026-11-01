//! Each layout change yields one debug line that describes the layout before and after it.

#[cfg(test)]
mod layout_trace {
    use proptest::prelude::*;
    use tehuti_core::{
        layout::{Command, Layout, LayoutConfig, OutputGeometry, OutputId, Rect, WindowId, WindowRules},
        layout_trace::{LayoutTrace, describe},
    };

    const COMMANDS: [Command; 6] = [
        Command::FocusColumnLeft,
        Command::FocusColumnRight,
        Command::MoveColumnLeft,
        Command::MoveColumnRight,
        Command::SwitchPresetColumnWidth,
        Command::SwitchPresetColumnWidthBack,
    ];

    /// Window 3 declares no `app_id`.
    fn app_id(window: WindowId) -> Option<String> {
        (window.0 != 3).then(|| format!("app.example.{}", window.0))
    }

    fn layout(windows: u64) -> Layout {
        let mut layout = Layout::try_new(LayoutConfig::default()).unwrap();
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        layout
            .add_output(OutputId(0), OutputGeometry { area, usable: area }, None)
            .unwrap();
        for window in 0..windows {
            layout
                .add_window(WindowId(window), WindowRules::default())
                .unwrap();
        }
        layout
    }

    #[test]
    fn widen_emits_one_line_with_the_new_widths() {
        let mut layout = layout(2);
        let mut trace = LayoutTrace::default();
        let _first = trace.observe(&layout.snapshot(), app_id);
        layout.apply(Command::SwitchPresetColumnWidth);
        let snapshot = layout.snapshot();
        let change = trace
            .observe(&snapshot, app_id)
            .expect("a widen changes the layout");
        assert!(trace.observe(&snapshot, app_id).is_none());
        let tile = snapshot.outputs[0].workspaces[0].columns[1].tiles[0].tile;
        assert!(
            change
                .after
                .contains(&format!("[app.example.1] w={} x={}", tile.width, tile.x)),
            "{}",
            change.after
        );
        assert!(change.after.contains("output_width=1920"), "{}", change.after);
        assert!(change.after.contains("focused=1"), "{}", change.after);
        assert!(change.after.contains("view_offset="), "{}", change.after);
        assert_ne!(change.before.as_deref(), Some(change.after.as_str()));
    }

    proptest! {
        #[test]
        fn one_line_per_change_and_none_otherwise(
            windows in 0_u64..5,
            script in prop::collection::vec(0..COMMANDS.len(), 0..24),
        ) {
            let mut layout = layout(windows);
            let mut trace = LayoutTrace::default();
            let mut previous = describe(&layout.snapshot(), app_id);
            prop_assert_eq!(
                trace.observe(&layout.snapshot(), app_id).map(|change| change.after),
                Some(previous.clone())
            );
            for index in script {
                layout.apply(COMMANDS[index]);
                let current = describe(&layout.snapshot(), app_id);
                let change = trace.observe(&layout.snapshot(), app_id);
                if current == previous {
                    prop_assert!(change.is_none());
                } else {
                    let change = change.unwrap();
                    prop_assert_eq!(change.before.as_deref(), Some(previous.as_str()));
                    prop_assert_eq!(&change.after, &current);
                }
                prop_assert!(trace.observe(&layout.snapshot(), app_id).is_none());
                previous = current;
            }
        }
    }
}
