//! Timed workspace navigation feedback, independent of focus and input routing.

use core::time::Duration;
use tehuti_core::layout::{OutputId, Snapshot, WorkspaceId};

#[derive(Debug, Default)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "conflicts with the workspace unreachable_pub lint"
)]
pub(crate) struct Feedback {
    previous: Option<Previous>,
    current: Option<Current>,
}

#[derive(Debug, Clone, Copy)]
struct Previous {
    output: OutputId,
    workspace: WorkspaceId,
    index: usize,
}

#[derive(Debug)]
struct Current {
    workspace: WorkspaceId,
    fallback: String,
    direction: Direction,
    started: Duration,
}

#[derive(Debug)]
enum Direction {
    Upward,
    Downward,
}

impl Direction {
    const fn label(&self) -> &'static str {
        match self {
            Self::Upward => "UP",
            Self::Downward => "DOWN",
        }
    }
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
        if let Some(previous) = self.previous {
            if previous.output != output.output {
                self.current = None;
            } else if previous.workspace != workspace.workspace {
                let previous_index = output
                    .workspaces
                    .iter()
                    .position(|candidate| candidate.workspace == previous.workspace)
                    .unwrap_or(previous.index);
                let direction = if index < previous_index {
                    Direction::Upward
                } else {
                    Direction::Downward
                };
                let name = workspace
                    .name
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| index.saturating_add(1).to_string());
                self.current = Some(Current {
                    workspace: workspace.workspace,
                    fallback: name,
                    direction,
                    started: now,
                });
            }
        }
        self.previous = Some(Previous {
            output: output.output,
            workspace: workspace.workspace,
            index,
        });
    }

    pub(crate) fn text(&self, now: Duration, destinations: &crate::destinations::Table) -> Option<String> {
        let current = self.current.as_ref()?;
        if now.saturating_sub(current.started) >= Duration::from_secs(1) {
            return None;
        }
        let name = destinations
            .name_for(&crate::destinations::Target::Workspace(current.workspace.0))
            .unwrap_or(&current.fallback);
        Some(format!("{} · {name}", current.direction.label()))
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
                Some("DOWN · 2")
            );
            layout.apply(Command::FocusWorkspaceAt(0));
            assert_eq!(layout.snapshot().focused_window, Some(WindowId(1)));
            feedback.update(&layout.snapshot(), Duration::ZERO);
            assert_eq!(
                feedback.text(Duration::ZERO, &destinations).as_deref(),
                Some("UP · 1")
            );
        }
        layout.apply(Command::FocusWorkspaceAt(1));
        feedback.update(&layout.snapshot(), Duration::ZERO);
        assert_eq!(
            feedback.text(Duration::ZERO, &destinations).as_deref(),
            Some("DOWN · 2")
        );
        layout.apply(Command::FocusWorkspaceAt(0));
        feedback.update(&layout.snapshot(), Duration::from_millis(100));
        assert_eq!(
            feedback.text(Duration::from_secs(1), &destinations).as_deref(),
            Some("UP · 1")
        );
        feedback.update(&layout.snapshot(), Duration::from_secs(1));
        assert_eq!(feedback.text(Duration::from_millis(1100), &destinations), None);
    }
    #[test]
    fn named_workspace_feedback_uses_the_layout_name() {
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
    }
    #[test]
    fn destination_name_overrides_the_captured_layout_name() {
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
        let mut snapshot = layout.snapshot();
        for output in &mut snapshot.outputs {
            for view in &mut output.workspaces {
                if view.workspace == workspace {
                    view.name = Some("Library".into());
                }
            }
        }
        let mut destinations = crate::destinations::Table::default();
        destinations.update_workspaces(&snapshot);
        assert_eq!(
            feedback.text(Duration::ZERO, &destinations).as_deref(),
            Some("UP · Library")
        );
    }
}
