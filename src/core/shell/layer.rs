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

use std::cell::Cell;

use smithay::{
    desktop::{LayerSurface, PopupKind, WindowSurfaceType, layer_map_for_output},
    output::{Output, WeakOutput},
    reexports::wayland_server::protocol::{wl_output, wl_surface::WlSurface},
    utils::{Logical, Rectangle},
    wayland::shell::{
        wlr_layer::{KeyboardInteractivity, Layer, LayerSurface as WlrLayerSurface, WlrLayerShellHandler, WlrLayerShellState},
        xdg::PopupSurface,
    },
};

use crate::{backend::Backend, core::state::Xfwl4State};

#[derive(Debug, Default)]
struct LastLayerBbox(Cell<Option<Rectangle<i32, Logical>>>);
#[derive(Debug, Default)]
struct RequestedOutput(Option<WeakOutput>);

impl<BackendData: Backend> WlrLayerShellHandler for Xfwl4State<BackendData> {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.core.shell_state.layer_shell_state
    }

    fn new_layer_surface(&mut self, surface: WlrLayerSurface, wl_output: Option<wl_output::WlOutput>, _layer: Layer, namespace: String) {
        let surface = LayerSurface::new(surface, namespace);

        let requested_output = wl_output.as_ref().and_then(Output::from_resource);
        surface
            .user_data()
            .insert_if_missing(|| RequestedOutput(requested_output.as_ref().map(Output::downgrade)));
        let output = requested_output
            .as_ref()
            .filter(|requested_output| self.core.outputs_config.output_is_enabled(requested_output))
            .cloned()
            .or_else(|| self.core.workspace_manager.outputs().next().cloned());

        if let Some(output) = &output {
            self.handle_new_layer_surface(&surface, output);
        } else {
            // Some clients (like gtk3-layer-shell) will bail if they don't get a configure soon
            // enough, so send a dummy configure.
            let size = requested_output
                .and_then(|output| self.core.outputs_config.last_known_output_size(&output))
                .unwrap_or_else(|| {
                    // We don't really have anything real to tell the client, so just give it
                    // something plausible.
                    (1920, 32).into()
                });
            surface.layer_surface().with_pending_state(|state| {
                state.size = Some(size);
            });
            surface.layer_surface().send_configure();
            self.add_pending_layer_surface(surface);
        }
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        if self.core.shell_state.pending_layer_surfaces.remove(surface.wl_surface()).is_none() {
            let output = self
                .core
                .workspace_manager
                .outputs()
                .find(|o| layer_map_for_output(o).layers().any(|layer| layer.layer_surface() == &surface))
                .cloned();

            if let Some(output) = output {
                let mut map = layer_map_for_output(&output);
                let layer = map.layers().find(|&layer| layer.layer_surface() == &surface).cloned();
                if let Some(layer) = layer {
                    map.unmap_layer(&layer);
                }
                drop(map);

                self.core.set_pointer_focus_dirty();
                self.output_workarea_changed(&output);
                self.reapply_anchored_layouts_on_output(&output);
                self.schedule_render_output(&output);
            }
        }
    }

    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        self.unconstrain_popup(&popup);

        if let Err(err) = popup.send_configure() {
            tracing::warn!("Failed to send configure event to popup with layer-shell parent: {err}");
        } else if let Err(err) = self.core.shell_state.popup_manager.track_popup(PopupKind::from(popup)) {
            tracing::warn!("Failed to track popup with layer-shell parent: {err}");
        }
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    fn handle_new_layer_surface(&mut self, surface: &LayerSurface, output: &Output) {
        let mut map = layer_map_for_output(output);
        let _ = map.map_layer(surface);
        drop(map);

        self.schedule_render_output(output);
    }

    pub(in crate::core) fn add_pending_layer_surface(&mut self, surface: LayerSurface) {
        self.core
            .shell_state
            .pending_layer_surfaces
            .insert(surface.wl_surface().clone(), surface);
    }

    pub(in crate::core) fn layer_surfaces_handle_output_enabled(&mut self, enabled_output: &Output) {
        for surface in std::mem::take(&mut self.core.shell_state.pending_layer_surfaces).into_values() {
            let requested_output = requested_output_for_layer_surface(&surface);
            let surface_output = requested_output
                .as_ref()
                .filter(|requested_output| self.core.outputs_config.output_is_enabled(requested_output))
                .unwrap_or(enabled_output);
            self.handle_new_layer_surface(&surface, surface_output);
            self.ensure_layer_initial_configure(surface.wl_surface());
        }

        // Take this opportunity to see if any layer surfaces on *any* output are on the "wrong"
        // output, and the "right" output is enabled and availble.  We collect the moves before
        // actually doing them, because re-mapping holds the layer maps of the old and new outputs,
        // but `ensure_layer_initial_configure` needs to take them again, which would otherwise
        // deadlock.
        let moves = self
            .core
            .outputs_config
            .outputs()
            .into_iter()
            .filter_map(|(_, output)| (output != *enabled_output).then_some(output))
            .flat_map(|output| {
                let map = layer_map_for_output(&output);
                map.layers()
                    .filter_map(|surface| {
                        requested_output_for_layer_surface(surface)
                            .filter(|requested_output| {
                                *requested_output != output && self.core.outputs_config.output_is_enabled(requested_output)
                            })
                            .map(|requested_output| (surface.clone(), output.clone(), requested_output))
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        for (surface, from_output, to_output) in moves {
            layer_map_for_output(&from_output).unmap_layer(&surface);
            let _ = layer_map_for_output(&to_output).map_layer(&surface);

            self.ensure_layer_initial_configure(surface.wl_surface());
            self.core.set_pointer_focus_dirty();
            self.output_workarea_changed(&from_output);
            self.reapply_anchored_layouts_on_output(&from_output);
        }
    }

    pub(in crate::core) fn layer_surface_with_exclusive_focus(&self) -> Option<LayerSurface> {
        self.core.shell_state.layer_surfaces().rev().find_map(|layer| {
            let exclusive = layer.with_cached_state(|data| {
                data.keyboard_interactivity == KeyboardInteractivity::Exclusive
                    && (data.layer == Layer::Top || data.layer == Layer::Overlay)
            });
            if exclusive {
                self.core.workspace_manager.outputs().find_map(|o| {
                    let map = layer_map_for_output(o);
                    map.layers().find(|l| l.layer_surface() == &layer).cloned()
                })
            } else {
                None
            }
        })
    }

    pub(in crate::core) fn ensure_layer_initial_configure(&mut self, surface: &WlSurface) {
        let output = self
            .core
            .workspace_manager
            .outputs()
            .find(|o| {
                let map = layer_map_for_output(o);
                map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).is_some()
            })
            .cloned();

        if let Some(output) = output {
            let mut map = layer_map_for_output(&output);

            // arrange the layers before sending the initial configure
            // to respect any size the client may have sent
            let layout_changed = map.arrange();

            // Sends the initial configure, and also catches up a surface that was configured from
            // a guessed size while it had no output to be arranged on.
            if let Some(layer) = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL) {
                layer.layer_surface().send_pending_configure();
            }

            // A layer is arranged from the size it asked for, before it has a buffer to be found
            // under the pointer at all, so the commit that finally gives it one usually leaves the
            // arrangement alone while still changing what the pointer is over.
            let bbox_changed = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).is_some_and(|layer| {
                let bbox = layer.bbox();
                layer.user_data().get_or_insert(LastLayerBbox::default).0.replace(Some(bbox)) != Some(bbox)
            });
            drop(map);

            if layout_changed || bbox_changed {
                self.core.set_pointer_focus_dirty();
            }
            if layout_changed {
                self.output_workarea_changed(&output);
                self.reapply_anchored_layouts_on_output(&output);
            }

            self.schedule_render_output(&output);
        };
    }
}

fn requested_output_for_layer_surface(surface: &LayerSurface) -> Option<Output> {
    surface
        .user_data()
        .get::<RequestedOutput>()
        .and_then(|requested_output| requested_output.0.as_ref())
        .and_then(WeakOutput::upgrade)
}
