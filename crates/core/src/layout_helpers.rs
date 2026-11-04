//! Workspace membership for windows outside the column strip.
use super::{Command, Layout, LayoutError, WindowId, WindowRules};

impl Layout {
    pub(super) fn contains_helper(&self, identity: WindowId) -> bool {
        self.outputs
            .iter()
            .flat_map(|output| &output.workspaces)
            .chain(&self.detached)
            .any(|workspace| workspace.helpers.iter().any(|(window, _)| *window == identity))
    }

    pub(super) fn find_helper(&self, identity: WindowId) -> Option<(usize, usize)> {
        self.outputs.iter().enumerate().find_map(|(output, target)| {
            target
                .workspaces
                .iter()
                .position(|workspace| workspace.helpers.iter().any(|(window, _)| *window == identity))
                .map(|workspace| (output, workspace))
        })
    }

    pub(super) fn clear_helper_focus(&mut self) {
        for workspace in self
            .outputs
            .iter_mut()
            .flat_map(|output| &mut output.workspaces)
            .chain(&mut self.detached)
        {
            workspace.focused_helper = None;
        }
    }

    pub(super) fn remove_helper(&mut self, identity: WindowId) -> bool {
        let mut removed = false;
        for workspace in self
            .outputs
            .iter_mut()
            .flat_map(|output| &mut output.workspaces)
            .chain(&mut self.detached)
        {
            let before = workspace.helpers.len();
            workspace.helpers.retain(|(window, _)| *window != identity);
            removed |= before != workspace.helpers.len();
            if workspace.focused_helper == Some(identity) {
                workspace.focused_helper = None;
            }
        }
        removed
    }

    pub(super) fn helper_blocks(&self, command: Command) -> bool {
        let focused = self
            .focused_index()
            .and_then(|index| self.outputs.get(index))
            .and_then(|output| output.workspaces.get(output.active));
        focused.is_some_and(|workspace| workspace.focused_helper.is_some())
            && !matches!(
                command,
                Command::FocusColumnLeft
                    | Command::FocusColumnRight
                    | Command::FocusColumnFirst
                    | Command::FocusColumnLast
                    | Command::FocusWindowUp
                    | Command::FocusWindowDown
                    | Command::FocusWindowTop
                    | Command::FocusWindowBottom
                    | Command::FocusWindowDownOrTop
                    | Command::FocusWindowUpOrBottom
                    | Command::FocusWindowUpOrColumnRight
                    | Command::FocusWindowInColumn(_)
                    | Command::FocusWorkspaceDown
                    | Command::FocusWorkspaceUp
                    | Command::FocusWorkspaceAt(_)
                    | Command::FocusMonitor(_)
                    | Command::MoveWorkspaceDown
                    | Command::MoveWorkspaceUp
                    | Command::MoveWorkspaceToMonitor(_)
            )
    }

    /// Changes helper membership within its current workspace.
    ///
    /// # Errors
    /// The window is absent from the connected outputs.
    pub fn set_helper(&mut self, identity: WindowId, helper: bool) -> Result<bool, LayoutError> {
        if let Some((output, workspace)) = self.find_helper(identity) {
            if helper {
                return Ok(false);
            }
            self.promote_helper(identity, output, workspace)?;
        } else {
            let (output, workspace, column, tile) = self
                .find_window(identity)
                .ok_or(LayoutError::UnknownWindow(identity))?;
            if !helper {
                return Ok(false);
            }
            if let Some(target) = self
                .outputs
                .get_mut(output)
                .and_then(|output| output.workspaces.get_mut(workspace))
            {
                let _removed = target.remove_tile(column, tile, identity);
                target.helpers.push((identity, WindowRules::default()));
            }
        }
        self.normalize();
        Ok(true)
    }

    fn promote_helper(
        &mut self,
        identity: WindowId,
        output: usize,
        workspace: usize,
    ) -> Result<(), LayoutError> {
        let rules = self
            .outputs
            .get_mut(output)
            .and_then(|output| output.workspaces.get_mut(workspace))
            .and_then(|workspace| {
                let position = workspace
                    .helpers
                    .iter()
                    .position(|(window, _)| *window == identity)?;
                Some(workspace.helpers.remove(position).1)
            })
            .ok_or(LayoutError::UnknownWindow(identity))?;
        let position = self.insert_window(identity, &rules, output, workspace);
        if let Some(target) = self
            .outputs
            .get_mut(output)
            .and_then(|output| output.workspaces.get_mut(workspace))
        {
            let column = target.columns.remove(position);
            let last = target.columns.len();
            target.columns.push(column);
            if target.focused.is_none() || target.focused_helper == Some(identity) {
                target.focused = Some(last);
            }
            target.focused_helper = None;
        }
        Ok(())
    }
}
