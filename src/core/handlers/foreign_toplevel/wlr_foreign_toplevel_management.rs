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
    reexports::wayland_server::protocol::{wl_output::WlOutput, wl_seat::WlSeat, wl_surface::WlSurface},
    utils::{Logical, Rectangle, SERIAL_COUNTER},
};

use crate::{
    backend::Backend,
    core::state::Xfwl4State,
    protocols::foreign_toplevel_management::{
        ToplevelId,
        wlr_foreign_toplevel_management::{WlrForeignToplevelHandler, WlrForeignToplevelManagementState},
    },
};

impl<BackendData: Backend + 'static> WlrForeignToplevelHandler for Xfwl4State<BackendData> {
    fn wlr_foreign_toplevel_management_state(&mut self) -> &mut WlrForeignToplevelManagementState {
        self.core
            .protocol_delegates
            .foreign_toplevel_state
            .foreign_toplevel_management_state
            .wlr_state()
    }

    fn on_toplevel_set_maximized(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id) {
            self.set_window_maximized(&window, None);
        }
    }

    fn on_toplevel_unset_maximized(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id) {
            self.set_window_unmaximized(&window, None);
        }
    }

    fn on_toplevel_set_minimized(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id) {
            self.set_window_minimized(&window);
        }
    }

    fn on_toplevel_unset_minimized(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id) {
            self.set_window_unminimized(&window, SERIAL_COUNTER.next_serial(), true);
        }
    }

    fn on_toplevel_activate(&mut self, toplevel_id: &ToplevelId, wl_seat: &WlSeat) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id) {
            let seat = Seat::from_resource(wl_seat);
            self.activate_window(&window, true, self.core.config.activate_action(), seat);
        }
    }

    fn on_toplevel_close(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id) {
            self.close_window(&window);
        }
    }

    fn on_toplevel_set_rectangle(&mut self, _toplevel_id: &ToplevelId, _wl_surface: &WlSurface, _rect: Rectangle<i32, Logical>) {
        // Currently don't do anything with this
    }

    fn on_toplevel_set_fullscreen(&mut self, toplevel_id: &ToplevelId, wl_output: Option<&WlOutput>) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id) {
            let output = wl_output.and_then(Output::from_resource);
            self.set_window_fullscreen(&window, output);
        }
    }

    fn on_toplevel_unset_fullscreen(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self.window_for_toplevel_id(toplevel_id) {
            self.set_window_unfullscreen(&window);
        }
    }
}
