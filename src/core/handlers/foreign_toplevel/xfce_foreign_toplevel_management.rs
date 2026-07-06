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
    input::Seat,
    output::Output,
    reexports::wayland_server::Client,
    utils::{Rectangle, SERIAL_COUNTER},
};

use crate::{
    backend::Backend,
    core::{
        drawing::wireframe::Wireframe,
        shell::{GrabTrigger, ResizeEdge},
        state::Xfwl4State,
        util::SeatFocusExt,
        workspaces::WindowStackingLayer,
    },
    protocols::foreign_toplevel_management::{
        ToplevelId,
        xfce_foreign_toplevel_management::{IconPixels, XfceForeignToplevelHandler, XfceForeignToplevelManagementState},
    },
};

impl<BackendData: Backend + 'static> XfceForeignToplevelHandler for Xfwl4State<BackendData> {
    fn xfce_foreign_toplevel_management_state(&mut self) -> &mut XfceForeignToplevelManagementState {
        self.core
            .protocol_delegates
            .foreign_toplevel_state
            .foreign_toplevel_management_state
            .xfce_state()
    }

    fn icon_pixels_for_toplevel(&mut self, toplevel_id: &ToplevelId, icon_size: u32, icon_scale: u32) -> Option<IconPixels> {
        self.window_for_toplevel_id(toplevel_id).and_then(|window| {
            let props = window.props();
            props
                .window_icon
                .window_icon_rasters()
                .iter()
                .find(|raster| raster.size.w.max(raster.size.h) == icon_size && raster.scale == icon_scale)
                .and_then(|raster| {
                    IconPixels::new(&raster.bytes, raster.size.w, raster.size.h, raster.size.w * 4)
                        .inspect_err(|err| tracing::warn!("Failed to create mapped file descriptor for icon pixels: {err}"))
                        .ok()
                })
        })
    }

    fn on_toplevel_set_shaded(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id) {
            self.set_window_shaded(&window, true);
        }
    }

    fn on_toplevel_unset_shaded(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id) {
            self.set_window_shaded(&window, false);
        }
    }

    fn on_toplevel_set_sticky(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id) {
            self.set_window_sticky(&window, true);
        }
    }

    fn on_toplevel_unset_sticky(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id) {
            self.set_window_sticky(&window, false);
        }
    }

    fn on_toplevel_set_keep_above(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id) {
            self.set_window_always_on_top(&window);
        }
    }

    fn on_toplevel_unset_keep_above(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id)
            && window.stacking_layer() == WindowStackingLayer::AlwaysOnTop
        {
            self.set_window_normal_stacking(&window);
        }
    }

    fn on_toplevel_set_keep_below(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id) {
            self.set_window_always_on_bottom(&window);
        }
    }

    fn on_toplevel_unset_keep_below(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id)
            && window.stacking_layer() == WindowStackingLayer::AlwaysOnBottom
        {
            self.set_window_normal_stacking(&window);
        }
    }

    fn on_toplevel_highlight(&mut self, toplevel_id: &ToplevelId, requesting_client: Client, seat: Seat<Self>) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id)
            // Wireframe is not being shown at all, or it is and is already owned by the requesting client
            && self.core.wireframe.as_ref().is_none_or(|wireframe| wireframe.is_owned_by(requesting_client.id()))
            // The client has pointer or keyboard focus
            && (seat.pointer_client().is_some_and(|client| client == requesting_client) || seat.keyboard_client().is_some_and(|client| client == requesting_client))
            && let Some(geometry) = self.core.workspace_manager.window_geometry(&window)
        {
            let mut wireframe = self
                .core
                .wireframe
                .take()
                .unwrap_or_else(|| Wireframe::new(Some(requesting_client), Rectangle::zero(), &self.core.config));
            wireframe.update_location(geometry.loc);
            wireframe.update_size(geometry.size);
            self.core.wireframe = Some(wireframe);
        }
    }

    fn on_toplevel_unhighlight(&mut self, _toplevel_id: &ToplevelId, requesting_client: Client) {
        if self
            .core
            .wireframe
            .as_ref()
            .is_some_and(|wireframe| wireframe.is_owned_by(requesting_client.id()))
        {
            self.core.wireframe = None;
        }
    }

    fn on_toplevel_move(&mut self, toplevel_id: &ToplevelId, _requesting_client: Client, seat: Seat<Self>) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id) {
            self.start_window_move(window.clone(), seat, SERIAL_COUNTER.next_serial(), GrabTrigger::Shell);
        }
    }

    fn on_toplevel_resize(&mut self, toplevel_id: &ToplevelId, _requesting_client: Client, seat: Seat<Self>) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id) {
            self.start_window_resize(
                window.clone(),
                seat,
                SERIAL_COUNTER.next_serial(),
                ResizeEdge::BOTTOM_RIGHT,
                GrabTrigger::Shell,
            );
        }
    }

    fn on_toplevel_move_to_output(&mut self, toplevel_id: &ToplevelId, output: Output) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id) {
            self.move_window_to_output(&window, output);
        }
    }

    fn on_toplevel_move_to_workspace(&mut self, toplevel_id: &ToplevelId, workspace_id: String) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id)
            && let Some(index) = self.core.workspace_manager.workspace_index_for_id(&workspace_id)
        {
            self.move_window_to_workspace_index(&window, index);
        }
    }
}
