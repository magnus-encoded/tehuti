//! Timed workspace navigation feedback, independent of focus and input routing.

use core::time::Duration;
use tehuti_core::layout::{OutputId, Snapshot, WorkspaceId};

#[derive(Debug, Default)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "conflicts with the workspace unreachable_pub lint"
)]
pub(crate) struct Feedback {
    previous: Option<(OutputId, WorkspaceId, usize)>,
    current: Option<(WorkspaceId, String, &'static str, Duration)>,
}

impl Feedback {
    pub(crate) fn update(&mut self, snapshot: &Snapshot, now: Duration) {
        let active = snapshot.outputs.iter().find_map(|output| {
            if Some(output.output) != snapshot.focused_output {
                return None;
            }
            let workspace = output.workspaces.get(output.active_workspace)?;
            Some((output, workspace, output.active_workspace))
        });
        let Some((output, workspace, index)) = active else {
            self.previous = None;
            self.current = None;
            return;
        };
        if let Some((previous_output, previous_workspace, previous_index)) = self.previous {
            if previous_output != output.output {
                self.current = None;
            } else if previous_workspace != workspace.workspace {
                let previous_index = output
                    .workspaces
                    .iter()
                    .position(|candidate| candidate.workspace == previous_workspace)
                    .unwrap_or(previous_index);
                let direction = if index < previous_index { "UP" } else { "DOWN" };
                let name = workspace
                    .name
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| format!("Workspace {}", index.saturating_add(1)));
                self.current = Some((workspace.workspace, name, direction, now));
            }
        }
        self.previous = Some((output.output, workspace.workspace, index));
    }

    pub(crate) fn text(&self, now: Duration, destinations: &crate::destinations::Table) -> Option<String> {
        let (workspace, fallback, direction, started) = self.current.as_ref()?;
        if now.saturating_sub(*started) >= Duration::from_secs(1) {
            return None;
        }
        let name = destinations
            .name_for(&crate::destinations::Target::Workspace(workspace.0))
            .unwrap_or(fallback);
        Some(format!("{direction} · {name}"))
    }
}

#[cfg(test)]
mod tests {
    use super::Feedback;
    use core::time::Duration;
    use tehuti_core::layout::{
        Command, Layout, LayoutConfig, OutputGeometry, OutputId, Rect, WindowId, WindowRules,
    };

    #[test]
    fn rapid_switches_replace_feedback_and_expire_after_the_last_switch() {
        let mut layout = Layout::new(LayoutConfig::default());
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width: 1280.0,
            height: 720.0,
        };
        layout
            .add_output(OutputId(1), OutputGeometry { area, usable: area }, None)
            .unwrap();
        let mut feedback = Feedback::default();
        layout.add_window(WindowId(1), WindowRules::default()).unwrap();
        let destinations = crate::destinations::Table::default();
        feedback.update(&layout.snapshot(), Duration::ZERO);
        assert_eq!(feedback.text(Duration::ZERO, &destinations), None);
        for _ in 0..20 {
            layout.apply(Command::FocusWorkspaceAt(1));
            assert_eq!(layout.snapshot().outputs[0].active_workspace, 1);
            feedback.update(&layout.snapshot(), Duration::ZERO);
            assert_eq!(
                feedback.text(Duration::ZERO, &destinations).as_deref(),
                Some("DOWN · Workspace 2")
            );
            layout.apply(Command::FocusWorkspaceAt(0));
            assert_eq!(layout.snapshot().focused_window, Some(WindowId(1)));
            feedback.update(&layout.snapshot(), Duration::ZERO);
            assert_eq!(
                feedback.text(Duration::ZERO, &destinations).as_deref(),
                Some("UP · Workspace 1")
            );
        }
        layout.apply(Command::FocusWorkspaceAt(1));
        feedback.update(&layout.snapshot(), Duration::ZERO);
        assert_eq!(
            feedback.text(Duration::ZERO, &destinations).as_deref(),
            Some("DOWN · Workspace 2")
        );
        layout.apply(Command::FocusWorkspaceAt(0));
        feedback.update(&layout.snapshot(), Duration::from_millis(100));
        assert_eq!(
            feedback.text(Duration::from_secs(1), &destinations).as_deref(),
            Some("UP · Workspace 1")
        );
        feedback.update(&layout.snapshot(), Duration::from_secs(1));
        assert_eq!(feedback.text(Duration::from_millis(1100), &destinations), None);
    }
    #[test]
    fn named_workspace_feedback_preserves_other_bar_feedback() {
        let mut layout = Layout::new(LayoutConfig::default());
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width: 1280.0,
            height: 720.0,
        };
        layout
            .add_output(OutputId(1), OutputGeometry { area, usable: area }, None)
            .unwrap();
        let mut feedback = Feedback::default();
        feedback.update(&layout.snapshot(), Duration::ZERO);
        let workspace = layout.add_named_workspace("Writing".into(), None).unwrap();
        layout.focus_workspace_id(workspace).unwrap();
        feedback.update(&layout.snapshot(), Duration::ZERO);
        let text = feedback
            .text(Duration::ZERO, &crate::destinations::Table::default())
            .unwrap();
        assert_eq!(text, "UP · Writing");
        let bar = tehuti_core::bar::Bar::new()
            .with_feedback(tehuti_core::bar::Segment {
                text: "Listening…".into(),
                tone: tehuti_core::bar::Tone::Live,
            })
            .with_navigation(text);
        let texts: Vec<_> = bar.segments().into_iter().map(|segment| segment.text).collect();
        assert_eq!(texts, ["Desktop", "Listening…", "UP · Writing"]);
    }
}
