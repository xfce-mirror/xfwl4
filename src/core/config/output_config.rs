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

use std::collections::HashMap;

use smithay::{
    desktop::{WindowSurface, layer_map_for_output, space::SpaceElement},
    output::{Mode, Output, PhysicalProperties, Scale, WeakOutput},
    reexports::wayland_server::backend::GlobalId,
    utils::{Logical, Point, Rectangle, Size, Transform},
};

use crate::{
    backend::Backend,
    core::{
        drawing::zoom::ZoomState,
        shell::{WindowElement, WindowProps, WindowState},
        state::Xfwl4State,
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

    pub(in crate::core) fn outputs(&self) -> Vec<(GlobalId, Output)> {
        self.configs
            .iter()
            .flat_map(|config| config.output.upgrade().map(|output| (config.global_id.clone(), output)))
            .collect()
    }

    pub(in crate::core) fn zoom_state_for_output_mut<'a>(&'a mut self, output: &Output) -> Option<&'a mut ZoomState> {
        self.config_for_output_mut(output).map(|config| &mut config.zoom_state)
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
    pub zoom_state: ZoomState,
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
            zoom_state: ZoomState::default(),
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
    pub(crate) fn output_created(&mut self, output: &Output) {
        let global_id = output.create_global::<Xfwl4State<BackendData>>(&self.core.display_handle);
        let config = (global_id, output.clone()).into();
        self.core.outputs_config.configs.push(config);

        #[cfg(feature = "debug")]
        if let Some(debug) = self.core.debug.as_ref() {
            output
                .user_data()
                .insert_if_missing(|| std::cell::RefCell::new(crate::core::debug::RenderDebug::new(debug)));
        }

        let x = self.core.workspace_manager.outputs().fold(0, |acc, o| {
            acc + self.core.workspace_manager.output_geometry(o).map(|geom| geom.size.w).unwrap_or(0)
        });
        let position = (x, 0).into();

        let PhysicalProperties {
            size: Size { w: phys_w, h: phys_h, .. },
            ..
        } = output.physical_properties();
        let scale = if phys_w > 0
            && phys_h > 0
            && let Some(Mode {
                size: Size { w: px_w, h: px_h, .. },
                ..
            }) = output.current_mode()
        {
            let phys_w = phys_w as f64;
            let phys_h = phys_h as f64;

            let dpi_w = (px_w as f64 / phys_w) * 25.4;
            let dpi_h = (px_h as f64 / phys_h) * 25.4;
            let dpi = ((dpi_w + dpi_h) / 2.).round();

            let iscale = (dpi / 132.).ceil() as i32;
            // Fractional scale is rounded up to the nearest 0.25.
            let fscale = (((dpi / 132.) * 4.).ceil() / 4.).max(1.);

            Scale::Custom {
                advertised_integer: iscale,
                fractional: fscale,
            }
        } else {
            Scale::Integer(1)
        };

        tracing::debug!("Guessing output scale as {scale:?} for output {}", output.name());

        output.change_current_state(None, None, Some(scale), Some(position));

        self.core.workspace_manager.map_output(output, output.current_location());
        self.core.workspace_manager.refresh_spaces();
        self.core.outputs_config.wlr_output_management_state.output_created::<Self>(output);
    }

    pub(crate) fn output_changed(&mut self, output: &Output) {
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

    pub(crate) fn output_destroyed(&mut self, output: &Output) {
        if let Some(config) = self.core.outputs_config.remove_config_for_output(output) {
            output.leave_all();
            self.core.workspace_manager.unmap_output(output);
            self.core.workspace_manager.refresh_spaces();
            self.core.outputs_config.wlr_output_management_state.output_destroyed(output);
            self.fixup_window_positions(Some(output));
            self.core.display_handle.remove_global::<Xfwl4State<BackendData>>(config.global_id);
        }
    }

    fn apply_output_config_change(&mut self, output: &Output, config_change: OutputConfigChange) -> anyhow::Result<()> {
        let res = self.backend.apply_output_config_change(output, config_change);
        if res.is_ok() {
            // The backend can't call Xfwl4State::output_changed(), so we have to do it ourselves.
            self.output_changed(output);
        }
        res
    }

    fn fixup_window_positions(&mut self, output_removed: Option<&Output>) {
        let pointer_location = self.core.pointer.current_location();

        let mut orphaned_windows = Vec::new();
        let mut remaximize_windows = Vec::new();
        let mut removed_outputs = Vec::new();
        let mut added_outputs = Vec::new();

        let outputs = self
            .core
            .workspace_manager
            .outputs()
            .flat_map(|o| {
                let geo = self.core.workspace_manager.output_geometry(o)?;
                let map = layer_map_for_output(o);
                let zone = map.non_exclusive_zone();
                Some(Rectangle::new(geo.loc + zone.loc, zone.size))
            })
            .collect::<Vec<_>>();

        let pointer_output_and_geometry = self
            .core
            .workspace_manager
            .output_under(pointer_location)
            .next()
            .or_else(|| self.core.workspace_manager.outputs().next())
            .and_then(|output| {
                self.core
                    .workspace_manager
                    .output_geometry(output)
                    .map(|geom| (output.clone(), geom))
            })
            .map(|(output, geom)| {
                let zone = layer_map_for_output(&output).non_exclusive_zone();
                (output, Rectangle::new(geom.loc + zone.loc, zone.size))
            });

        if let Some((pointer_output, pointer_output_geometry)) = pointer_output_and_geometry {
            #[allow(clippy::mutable_key_type)]
            let all_output_geometries = self
                .core
                .workspace_manager
                .outputs()
                .flat_map(|output| {
                    self.core
                        .workspace_manager
                        .output_geometry(output)
                        .map(|geom| (output.clone(), geom))
                })
                .collect::<HashMap<_, _>>();

            for (workspace_num, workspace) in self.core.workspace_manager.workspaces_mut().iter_mut().enumerate() {
                for window in workspace.visible_windows() {
                    if (!window.sticky() || workspace_num == 0)
                        && let Some(window_location) = workspace.window_location(window)
                    {
                        let geo_loc = window.bbox().loc + window_location;

                        if window.maximized() {
                            let maximize_geometry = window
                                .0
                                .user_data()
                                .get::<WindowProps>()
                                .and_then(|props| props.0.lock().unwrap().maximized_output.as_ref().and_then(WeakOutput::upgrade))
                                .and_then(|output| all_output_geometries.get(&output).cloned().map(|geom| (output, geom)))
                                .unwrap_or((pointer_output.clone(), pointer_output_geometry));
                            remaximize_windows.push((window.clone(), maximize_geometry));
                        } else if !outputs.iter().any(|o_geo| o_geo.contains(geo_loc)) {
                            orphaned_windows.push(window.clone());
                        }

                        if let Some(output_removed) = output_removed {
                            removed_outputs.push((window.clone(), output_removed.clone()));
                        }
                    }
                }
            }

            for (window, (output, into_rect)) in remaximize_windows.into_iter() {
                let new_outputs = self.remaximize_window(&window, output, into_rect);
                if !new_outputs.is_empty() {
                    added_outputs.push((window.clone(), new_outputs));
                }
            }

            for window in orphaned_windows.into_iter() {
                self.place_window(&window, false);
            }

            for (window, output_removed) in removed_outputs {
                self.core.toplevel_changed(
                    &window,
                    None,
                    None,
                    WindowState::empty(),
                    WindowState::empty(),
                    Vec::new(),
                    vec![output_removed.clone()],
                    None,
                );
            }

            for (window, outputs_added) in added_outputs {
                self.core.toplevel_changed(
                    &window,
                    None,
                    None,
                    WindowState::empty(),
                    WindowState::empty(),
                    outputs_added,
                    Vec::new(),
                    None,
                );
            }
        }
    }

    fn remaximize_window(&mut self, window: &WindowElement, output: Output, mut geometry: Rectangle<i32, Logical>) -> Vec<Output> {
        if let Some(window_decorations) = window.decoration_state().window_decorations_mut() {
            window_decorations.refresh_layout();
            geometry.size.w -= window_decorations.left_decoration_width() + window_decorations.right_decoration_width();
            geometry.size.h -= window_decorations.top_decoration_height() + window_decorations.bottom_decoration_height();
        }

        if !window.minimized() {
            self.core.workspace_manager.relocate_window(window, geometry.loc, false);
        }

        window
            .0
            .user_data()
            .get_or_insert(WindowProps::default)
            .0
            .lock()
            .unwrap()
            .maximized_output = Some(output.downgrade());

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

        self.core
            .workspace_manager
            .output_under(geometry.loc.to_f64())
            .cloned()
            .collect::<Vec<_>>()
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
