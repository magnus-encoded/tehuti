//! No gap opens past the last column while columns sit off the left edge (#426).

#[cfg(test)]
#[expect(
    clippy::float_arithmetic,
    reason = "layout geometry is fractional logical pixels"
)]
mod column_gap {

    use proptest::prelude::*;
    use tehuti_core::layout::{
        Command, Layout, LayoutConfig, OutputGeometry, OutputId, Rect, SizeChange, WindowId, WindowRules,
    };

    const EPSILON: f64 = 1e-6;

    #[derive(Debug, Clone, Copy)]
    enum Step {
        Open,
        Close,
        Command(Command),
    }

    fn layout_on(width: f64, height: f64) -> Layout {
        let mut layout = Layout::new(LayoutConfig::default());
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        };
        layout
            .add_output(OutputId(0), OutputGeometry { area, usable: area }, None)
            .unwrap();
        layout
    }

    fn run(layout: &mut Layout, next: &mut u64, step: Step) {
        match step {
            Step::Open => {
                layout
                    .add_window(WindowId(*next), WindowRules::default())
                    .unwrap();
                *next = next.saturating_add(1);
            }
            Step::Close => {
                if let Some(focused) = layout.snapshot().focused_window {
                    layout.remove_window(focused).unwrap();
                }
            }
            Step::Command(command) => layout.apply(command),
        }
    }

    /// Checks both laws of #426 and describes the first one broken.
    fn broken_law(layout: &Layout) -> Option<String> {
        let gaps = LayoutConfig::default().gaps;
        let snapshot = layout.snapshot();
        let output = &snapshot.outputs[0];
        let working = output.working_area;
        let workspace = &output.workspaces[output.active_workspace];
        let focused = workspace.focused_column?;
        let tiles: Vec<Rect> = workspace
            .columns
            .iter()
            .map(|column| column.tiles[column.focused_tile].tile)
            .collect();
        let tile = tiles[focused];
        let right = working.x + working.width;
        if tile.width <= 2.0_f64.mul_add(-gaps, working.width) {
            if tile.x < working.x - EPSILON || tile.x + tile.width > right + EPSILON {
                return Some(format!("focused column {tile:?} leaves {working:?}"));
            }
        } else if tile.x < working.x - EPSILON || tile.x > right {
            return Some(format!("left edge of wide column {tile:?} is hidden"));
        }
        let first = tiles.first()?;
        let last = tiles.last()?;
        let off_left = first.x < working.x + gaps - EPSILON;
        let gap = right - (last.x + last.width) > gaps + EPSILON;
        let content: f64 = tiles.iter().map(|column| column.width + gaps).sum::<f64>() + gaps;
        if off_left && gap && content <= working.width + EPSILON {
            return Some(format!(
                "gap past last column {last:?} while {first:?} is off the left"
            ));
        }
        None
    }

    fn step() -> impl Strategy<Value = Step> {
        prop_oneof![
            Just(Step::Open),
            Just(Step::Close),
            Just(Step::Command(Command::FocusColumnLeft)),
            Just(Step::Command(Command::FocusColumnRight)),
            Just(Step::Command(Command::SwitchPresetColumnWidth)),
            Just(Step::Command(Command::SwitchPresetColumnWidthBack)),
            prop::sample::select(vec![1.0 / 3.0, 0.5, 2.0 / 3.0, 1.0])
                .prop_map(|value| Step::Command(Command::SetColumnWidth(SizeChange::SetProportion(value)))),
        ]
    }

    /// The minimal sequence the property found: the second of two half-width columns
    /// was widened to 2/3, scrolling the view, and set back to 1/2. The view stayed
    /// scrolled, so the first column sat off the left and a gap opened on the right.
    #[test]
    fn narrowing_the_second_column_scrolls_the_gap_away() {
        let mut layout = layout_on(1920.0, 1080.0);
        let mut next = 0;
        for step in [
            Step::Open,
            Step::Command(Command::SetColumnWidth(SizeChange::SetProportion(0.5))),
            Step::Open,
            Step::Command(Command::SetColumnWidth(SizeChange::SetProportion(2.0 / 3.0))),
            Step::Command(Command::SetColumnWidth(SizeChange::SetProportion(0.5))),
        ] {
            run(&mut layout, &mut next, step);
        }
        assert_eq!(broken_law(&layout), None);
        let snapshot = layout.snapshot();
        let workspace = &snapshot.outputs[0].workspaces[0];
        let first = workspace.columns[0].tiles[0].tile;
        assert!((first.x - 16.0).abs() < EPSILON, "{first:?}");
    }

    proptest! {
        #[test]
        fn columns_stay_on_the_output_without_a_right_gap(
            output in prop::sample::select(vec![(1920.0, 1080.0), (1280.0, 720.0), (2560.0, 1440.0), (3440.0, 1440.0), (1080.0, 1920.0)]),
            steps in prop::collection::vec(step(), 1..40),
        ) {
            let mut layout = layout_on(output.0, output.1);
            let mut next = 0;
            for (index, step) in steps.iter().enumerate() {
                run(&mut layout, &mut next, *step);
                if let Some(broken) = broken_law(&layout) {
                    prop_assert!(false, "after {:?}: {}", &steps[..=index], broken);
                }
            }
        }
    }
}
