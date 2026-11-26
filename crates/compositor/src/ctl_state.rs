//! Session control uses explicit actions, never input routing.
use super::Compositor;
use crate::ctl::Command;
use serde_json::{Value, json};
use std::path::Path;

impl Compositor {
    /// # Errors
    /// Validates destinations and reports failures without changing approval state.
    #[expect(clippy::too_many_lines, reason = "exhaustive typed control dispatch")]
    pub fn control(&mut self, command: Command, directory: &Path) -> Result<Value, String> {
        match command {
            Command::Status => {
                return Ok(
                    json!({"pid": std::process::id(), "version": env!("CARGO_PKG_VERSION"), "id": self.placement.focused_destination(), "focused": self.placement.focused_destination().is_some(), "app_id": self.focused_window_label().0, "restart_requested": self.restart_requested}),
                );
            }
            Command::Windows => return Ok(self.control_windows()),
            Command::Workspaces => return Ok(self.control_workspaces()),
            Command::Outputs => return Ok(self.control_outputs()),
            Command::Events => {
                return Ok(json!(self.control_events()));
            }
            Command::Layout { action } => self.apply_layout(action),
            Command::Focus { identity } => {
                self.placement
                    .focus_destination(identity)
                    .map_err(|error| error.to_string())?;
                self.place_layout();
            }
            Command::Close { identity } => {
                let identity = identity
                    .or_else(|| self.placement.focused_destination())
                    .ok_or("no focused window")?;
                let key = self
                    .placement
                    .placed(|_| 0.0)
                    .into_iter()
                    .find(|placed| placed.identity == identity)
                    .map(|placed| placed.key.clone())
                    .ok_or("unknown window id")?;
                self.remove_layout_window(&key, true);
            }
            Command::Notice { text } => {
                let text = tehuti_protocol::Text::new(text)?;
                self.show_notice(text.as_str());
            }
            Command::Launch { app } => {
                let installed = self.launcher.installed();
                let entry = installed
                    .iter()
                    .find(|entry| **entry == app || entry.strip_suffix(".desktop") == Some(app.as_str()))
                    .cloned()
                    .or_else(|| {
                        self.launcher
                            .table()
                            .entries()
                            .iter()
                            .filter(|destination| {
                                destination.source == crate::destinations::Source::Desktop
                                    && destination.name.eq_ignore_ascii_case(&app)
                            })
                            .flat_map(|destination| &destination.routes)
                            .flat_map(|route| &route.steps)
                            .find_map(|step| match &step.target {
                                crate::destinations::Target::Desktop(entry) => Some(entry.clone()),
                                crate::destinations::Target::Url(_)
                                | crate::destinations::Target::Action(_)
                                | crate::destinations::Target::Window(_)
                                | crate::destinations::Target::Application(_)
                                | crate::destinations::Target::Menu(_)
                                | crate::destinations::Target::Workspace(_) => None,
                            })
                    })
                    .ok_or("unknown desktop application; raw commands require --command")?;
                self.launcher.control_launch(&crate::desktop::launch_for(
                    &entry,
                    self.launch_socket.as_deref().ok_or("no compositor socket")?,
                )?)?;
            }
            Command::LaunchCommand { program, arguments } => {
                if program.is_empty()
                    || program.contains('\0')
                    || arguments.iter().any(|arg| arg.contains('\0'))
                {
                    return Err("invalid command".into());
                }
                let mut launch = crate::launch::Launch::new(&program, "ctl command")
                    .on_socket(self.launch_socket.as_deref().ok_or("no compositor socket")?);
                for argument in arguments {
                    launch = launch.arg(argument);
                }
                self.launcher.control_launch(&launch)?;
            }
            Command::DisplayScale { scale } => {
                if !(1..=8).contains(&scale)
                    || self
                        .output
                        .as_ref()
                        .and_then(smithay::output::Output::current_mode)
                        .is_none()
                {
                    return Err("scale requires an output and an integer from 1 to 8".into());
                }
                self.change_display_scale(core::num::NonZeroU32::new(scale).ok_or("invalid scale")?);
            }
            Command::DisplayMode { .. } => {
                return Err("display mode must be handled by the output backend".into());
            }
            Command::Shot { path } => {
                crate::driver_shot::save(self, &path).map_err(|_| "output capture failed".to_owned())?;
                return Ok(json!({"path": path}));
            }
            Command::ShotWindow { path } => {
                let focused = self
                    .placement
                    .focused()
                    .and_then(|key| self.placement.window(key))
                    .ok_or("no focused window")?;
                let location = self
                    .space
                    .element_location(focused)
                    .ok_or("window is not mapped")?;
                let size = focused.layout_size();
                let scale = self
                    .output
                    .as_ref()
                    .ok_or("no active output")?
                    .current_scale()
                    .integer_scale();
                let mut frame =
                    crate::driver_shot::frame(self, "00:00").map_err(|_| "window capture failed")?;
                let left = u32::try_from(location.x.saturating_mul(scale).max(0))
                    .map_err(|_| "invalid window geometry")?;
                let top = u32::try_from(location.y.saturating_mul(scale).max(0))
                    .map_err(|_| "invalid window geometry")?;
                let width = u32::try_from(size.w.saturating_mul(scale))
                    .map_err(|_| "invalid window geometry")?
                    .min(frame.width().saturating_sub(left));
                let height = u32::try_from(size.h.saturating_mul(scale))
                    .map_err(|_| "invalid window geometry")?
                    .min(frame.height().saturating_sub(top));
                if width == 0 || height == 0 {
                    return Err("window is outside the output".into());
                }
                image::imageops::crop(&mut frame, left, top, width, height)
                    .to_image()
                    .save_with_format(&path, image::ImageFormat::Png)
                    .map_err(|_| "cannot save window capture")?;
                return Ok(json!({"path": path}));
            }
            Command::ContextList => return Ok(self.control_contexts()),
            Command::ContextActive => {
                let (app, title) = self.focused_window_label();
                let active = app.as_deref().and_then(|app| {
                    tehuti_core::own_context(self.contexts(), app, title.as_deref().unwrap_or_default())
                });
                return Ok(
                    json!({"app_id": active.map(tehuti_protocol::Context::app_id), "global": self.contexts().iter().any(|context| context.app_id() == "*")}),
                );
            }
            Command::ContextReload => {
                let reload = crate::contexts::reload(directory)
                    .map_err(|_| "context directory unavailable".to_owned())?;
                let skipped = !reload.skipped.is_empty();
                self.apply_reload(Ok(reload));
                if skipped {
                    return Err("invalid context files; last good versions retained".into());
                }
                return Ok(self.control_contexts());
            }
            Command::ContextCheck { path } => {
                let checked = crate::contexts::reload(path.as_deref().unwrap_or(directory))
                    .map_err(|_| "context directory unavailable".to_owned())?;
                if !checked.skipped.is_empty() || !checked.phrase_warnings.is_empty() {
                    return Err("context check failed; repair invalid files or phrases".into());
                }
                return Ok(json!({"valid": true, "count": checked.contexts.len()}));
            }
            Command::PluginList => {
                return Ok(self
                    .plugins
                    .as_ref()
                    .map_or_else(|| json!([]), crate::plugins::Plugins::control_list));
            }
            Command::SessionRestart => self.apply(&tehuti_protocol::Action::Restart),
        }
        Ok(json!({}))
    }

    fn control_events(&self) -> Vec<crate::ctl::Event> {
        use crate::ctl::Event;
        let mut events = vec![Event::Focus {
            id: self.placement.focused_destination(),
            focused: self.placement.focused_destination().is_some(),
        }];
        events.extend(self.placement.placed(|_| 0.0).iter().map(|window| Event::Window {
            id: window.identity,
            focused: window.focused,
        }));
        events.extend(self.contexts().iter().map(|context| Event::Context {
            app_id: context.app_id().to_owned(),
            title: context.title().map(str::to_owned),
        }));
        for output in &self.placement.snapshot().outputs {
            events.extend(
                output
                    .workspaces
                    .iter()
                    .enumerate()
                    .map(|(index, workspace)| Event::Workspace {
                        id: workspace.workspace.0,
                        focused: index == output.active_workspace,
                    }),
            );
        }
        events.extend(self.space.outputs().filter_map(|output| {
            output.current_mode().map(|mode| Event::Mode {
                name: output.name(),
                width: mode.size.w,
                height: mode.size.h,
                refresh_mhz: mode.refresh,
                scale: output.current_scale().fractional_scale(),
            })
        }));
        if let Some(text) = self.visible_notice_text() {
            events.push(Event::Notice { text });
        }
        events
    }

    fn control_windows(&self) -> Value {
        let windows: Vec<_> = self
            .placement
            .placed(|_| 0.0)
            .into_iter()
            .map(|placed| {
                let app_id = placed.window.toplevel().and_then(|surface| {
                    smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
                        states
                            .data_map
                            .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                            .and_then(|data| {
                                data.lock().ok().and_then(|attributes| attributes.app_id.clone())
                            })
                    })
                });
                let location = self.space.element_location(placed.window).unwrap_or_default();
                let size = placed.window.layout_size();
                json!({"id": placed.identity, "app_id": app_id, "focused": placed.focused,
                "geometry": {"x": location.x, "y": location.y, "width": size.w, "height": size.h}})
            })
            .collect();
        json!(windows)
    }

    fn control_workspaces(&self) -> Value {
        let workspaces: Vec<_> = self.placement.snapshot().outputs.iter().flat_map(|output| output.workspaces.iter().enumerate().map(|(index, workspace)| {
            json!({"id": workspace.workspace.0, "name": workspace.name, "output": output.output.0, "focused": index == output.active_workspace,
                "windows": workspace.columns.iter().flat_map(|column| column.tiles.iter().map(|tile| tile.window.0)).chain(workspace.helpers.iter().map(|window| window.0)).collect::<Vec<_>>()})
        })).collect();
        json!(workspaces)
    }

    fn control_outputs(&self) -> Value {
        let outputs: Vec<_> = self.space.outputs().map(|output| {
            let current = output.current_mode().map(|mode| json!({"width": mode.size.w, "height": mode.size.h, "refresh_mhz": mode.refresh}));
            let modes: Vec<_> = output.modes().iter().map(|mode| json!({"width": mode.size.w, "height": mode.size.h, "refresh_mhz": mode.refresh})).collect();
            json!({"name": output.name(), "mode": current, "modes": modes, "scale": output.current_scale().fractional_scale()})
        }).collect();
        json!(outputs)
    }

    fn control_contexts(&self) -> Value {
        json!(
            self.contexts()
                .iter()
                .map(|context| json!({"app_id": context.app_id(), "title": context.title()}))
                .collect::<Vec<_>>()
        )
    }
}
