// xfwl4 -- Wayland compositor for the Xfce Desktop Environment
//
// Copyright (C) 2026 Brian Tarricone <brian@tarricone.org>
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use smithay::{
    desktop::{WindowSurface, layer_map_for_output, space::SpaceElement},
    output::{Mode, Output, Scale, WeakOutput},
    reexports::wayland_server::backend::GlobalId,
    utils::{Logical, Point, Rectangle, Transform},
};

use crate::{
    backend::Backend,
    core::{
        handlers::ForeignToplevelState,
        shell::{WindowElement, WindowState, place_new_window},
        state::Xfwl4State,
        workspaces::Workspace,
    },
    protocols::wlr_output_management::{
        ConfiguredMode, OutputConfigurationUpdate, WlrOutputConfiguration, WlrOutputManagementHandler, WlrOutputManagementState,
        delegate_wlr_output_management,
    },
};

pub struct OutputsConfig {
    configs: Vec<OutputConfig>,
    wlr_output_management_state: WlrOutputManagementState,
}

impl OutputsConfig {
    pub fn new(wlr_output_management_state: WlrOutputManagementState) -> Self {
        Self {
            configs: Vec::new(),
            wlr_output_management_state,
        }
    }

    fn config_for_output_mut(&mut self, output: &Output) -> Option<&mut OutputConfig> {
        self.configs.iter_mut().find(|config| config.output == *output)
    }

    fn remove_config_for_output(&mut self, output: &Output) -> Option<OutputConfig> {
        if let Some(pos) = self.configs.iter().position(|config| config.output == *output) {
            Some(self.configs.remove(pos))
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub struct OutputConfig {
    pub global_id: GlobalId,
    pub output: WeakOutput,
    pub preferred_mode: Option<Mode>,
    pub current_mode: Option<Mode>,
    pub scale: Scale,
    pub transform: Transform,
    pub location: Point<i32, Logical>,
}

impl From<(GlobalId, Output)> for OutputConfig {
    fn from((global_id, output): (GlobalId, Output)) -> Self {
        Self {
            global_id,
            output: output.downgrade(),
            preferred_mode: output.preferred_mode(),
            current_mode: output.current_mode(),
            scale: output.current_scale(),
            transform: output.current_transform(),
            location: output.current_location(),
        }
    }
}

#[derive(Debug)]
pub struct OutputConfigChange {
    pub preferred_mode: Option<Option<Mode>>,
    pub current_mode: Option<Option<Mode>>,
    pub scale: Option<Scale>,
    pub transform: Option<Transform>,
    pub location: Option<Point<i32, Logical>>,
}

impl OutputConfigChange {
    pub fn new_disabled() -> Self {
        Self {
            current_mode: Some(None),
            preferred_mode: None,
            scale: None,
            transform: None,
            location: None,
        }
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub fn output_created(&mut self, global_id: GlobalId, output: &Output) {
        let config = (global_id, output.clone()).into();
        self.core.outputs_config.configs.push(config);

        #[cfg(feature = "debug")]
        if let Some(debug) = self.core.debug.as_ref() {
            output
                .user_data()
                .insert_if_missing(|| std::cell::RefCell::new(crate::core::debug::RenderDebug::new(debug)));
        }

        self.core.workspace_manager.map_output(output, output.current_location());
        self.core.workspace_manager.refresh_spaces();
        self.core.outputs_config.wlr_output_management_state.output_created::<Self>(output);
    }

    pub fn output_changed(&mut self, output: &Output) {
        if let Some(config) = self.core.outputs_config.config_for_output_mut(output) {
            let newly_enabled = config.current_mode.is_none() && output.current_mode().is_some();
            let newly_disabled = config.current_mode.is_some() && output.current_mode().is_none();
            let size_changed = config.current_mode != output.current_mode()
                || config.scale.integer_scale() != output.current_scale().integer_scale()
                || config.scale.fractional_scale() != output.current_scale().fractional_scale()
                || config.transform != output.current_transform();

            config.preferred_mode = output.preferred_mode();
            config.current_mode = output.current_mode();
            config.scale = output.current_scale();
            config.transform = output.current_transform();
            config.location = output.current_location();

            if newly_disabled {
                self.core.workspace_manager.unmap_output(output);
                self.fixup_window_positions(Some(output));
            } else {
                if newly_enabled {
                    self.core.workspace_manager.map_output(output, config.location);
                }

                if newly_enabled || size_changed {
                    layer_map_for_output(output).arrange();
                    self.core.workspace_manager.refresh_spaces();
                }

                if size_changed {
                    self.fixup_window_positions(None);
                    self.backend.reset_buffers(output);
                }
            }

            self.core.outputs_config.wlr_output_management_state.output_changed::<Self>(output);
        } else {
            tracing::warn!("Got output_changed for unknown output {}", output.name());
        }
    }

    pub fn output_destroyed(&mut self, output: &Output) {
        if self.core.outputs_config.remove_config_for_output(output).is_some() {
            self.core.workspace_manager.unmap_output(output);
            self.core.workspace_manager.refresh_spaces();
            self.core.outputs_config.wlr_output_management_state.output_destroyed(output);
            self.fixup_window_positions(Some(output));
        }
    }

    pub fn apply_output_config_change(&mut self, output: &Output, config_change: OutputConfigChange) -> anyhow::Result<()> {
        let res = self.backend.apply_output_config_change(output, config_change);
        if res.is_ok() {
            // The backend can't call Xfwl4State::output_changed(), so we have to do it ourselves.
            self.output_changed(output);
        }
        res
    }

    fn fixup_window_positions(&mut self, output_removed: Option<&Output>) {
        let pointer_location = self.core.pointer.current_location();

        for workspace in self.core.workspace_manager.workspaces_mut() {
            let outputs = workspace
                .outputs()
                .flat_map(|o| {
                    let geo = workspace.output_geometry(o)?;
                    let map = layer_map_for_output(o);
                    let zone = map.non_exclusive_zone();
                    Some(Rectangle::new(geo.loc + zone.loc, zone.size))
                })
                .collect::<Vec<_>>();

            let pointer_output_geometry = workspace
                .output_under(pointer_location)
                .next()
                .or_else(|| workspace.outputs().next())
                .map(|output| layer_map_for_output(output).non_exclusive_zone())
                .unwrap_or_else(|| Rectangle::from_size((800, 800).into()));

            let mut orphaned_windows = Vec::new();
            let mut remaximize_windows = Vec::new();
            for window in workspace.elements() {
                let window_location = match workspace.element_location(window) {
                    Some(loc) => loc,
                    None => continue,
                };
                let geo_loc = window.bbox().loc + window_location;

                let maximized_output_geom = window
                    .maximized_output()
                    .map(|output| layer_map_for_output(&output).non_exclusive_zone());

                if !outputs.iter().any(|o_geo| o_geo.contains(geo_loc)) {
                    if window.maximized() {
                        remaximize_windows.push((window.clone(), pointer_output_geometry));
                    } else {
                        orphaned_windows.push(window.clone());
                    }
                } else if let Some(maximized_output_geom) = maximized_output_geom {
                    remaximize_windows.push((window.clone(), maximized_output_geom));
                }

                if let Some(output_removed) = output_removed {
                    self.core.foreign_toplevel_state.toplevel_changed(
                        window,
                        None,
                        None,
                        WindowState::empty(),
                        WindowState::empty(),
                        Vec::new(),
                        vec![output_removed.clone()],
                        None,
                    );
                }
            }

            for window in orphaned_windows.into_iter() {
                place_new_window(workspace, pointer_location, &window, false);
            }

            for (window, into_rect) in remaximize_windows.into_iter() {
                remaximize_window::<BackendData>(workspace, &mut self.core.foreign_toplevel_state, &window, into_rect);
            }
        }
    }
}

fn remaximize_window<BackendData: Backend + 'static>(
    workspace: &mut Workspace,
    foreign_toplevel_state: &mut ForeignToplevelState<BackendData>,
    window: &WindowElement,
    mut geometry: Rectangle<i32, Logical>,
) {
    if let Some(window_decorations) = window.decoration_state().window_decorations_mut() {
        window_decorations.update();
        geometry.size.w -= window_decorations.left_decoration_width() + window_decorations.right_decoration_width();
        geometry.size.h -= window_decorations.top_decoration_height() + window_decorations.bottom_decoration_height();
    }

    if !window.minimized() {
        workspace.map_element(window.clone(), geometry.loc, false);
    }

    match window.0.underlying_surface() {
        WindowSurface::Wayland(surface) => {
            surface.with_pending_state(|state| {
                state.bounds = Some(geometry.size);
                state.size = Some(geometry.size);
            });

            if surface.is_initial_configure_sent() {
                surface.send_pending_configure();
            }
        }

        #[cfg(feature = "xwayland")]
        WindowSurface::X11(surface) => {
            let _ = surface.configure(geometry);
        }
    }

    let new_outputs = workspace.output_under(geometry.loc.to_f64()).cloned().collect::<Vec<_>>();
    if !new_outputs.is_empty() {
        foreign_toplevel_state.toplevel_changed(
            window,
            None,
            None,
            WindowState::empty(),
            WindowState::empty(),
            new_outputs,
            Vec::new(),
            None,
        );
    }
}

pub fn scale_from_fractional(scale: f64) -> Scale {
    Scale::Custom {
        advertised_integer: scale.ceil() as i32,
        // We only allow fractional scale in increments of 0.25.
        fractional: ((scale * 4.).ceil() / 4.).max(1.),
    }
}

impl<BackendData: Backend + 'static> WlrOutputManagementHandler for Xfwl4State<BackendData> {
    fn wlr_output_management_state(&mut self) -> &mut WlrOutputManagementState {
        &mut self.core.outputs_config.wlr_output_management_state
    }

    fn on_test_configuration(&mut self, configuration: WlrOutputConfiguration) {
        tracing::debug!("test configuration {configuration:?}");

        if configuration
            .updates()
            .iter()
            .any(|update| matches!(update, OutputConfigurationUpdate::Enable(head) if head.adaptive_sync().is_some()))
        {
            configuration.send_failed();
        } else {
            configuration.send_succeeded();
        }
    }

    fn on_apply_configuration(&mut self, configuration: WlrOutputConfiguration) {
        tracing::debug!("apply configuration {configuration:?}");

        let mut failed = false;
        for update in configuration.updates() {
            if let Some((output, config_change)) = match update {
                OutputConfigurationUpdate::Enable(head) => head.output().map(|output| {
                    (
                        output,
                        OutputConfigChange {
                            current_mode: head.mode().map(|mode| {
                                Some(match mode {
                                    ConfiguredMode::Advertised(mode) => mode,
                                    ConfiguredMode::Custom { width, height, refresh } => smithay::output::Mode {
                                        size: (width, height).into(),
                                        refresh,
                                    },
                                })
                            }),
                            scale: head.scale().map(scale_from_fractional),
                            transform: head.transform(),
                            location: head.position(),
                            preferred_mode: None,
                        },
                    )
                }),
                OutputConfigurationUpdate::Disable(output) => output.upgrade().map(|output| (output, OutputConfigChange::new_disabled())),
            } && let Err(err) = self.apply_output_config_change(&output, config_change)
            {
                // TODO: roll back any prior successful updates
                tracing::warn!("Failed to apply output config change to output {}: {err}", output.name());
                failed = true;
                break;
            }
        }

        if !failed {
            configuration.send_succeeded();
        } else {
            configuration.send_failed();
        }
    }
}

delegate_wlr_output_management!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
