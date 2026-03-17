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
    protocols::wlr_foreign_toplevel_management::{
        ToplevelId, WlrForeignToplevelHandler, WlrForeignToplevelManagementState, delegate_wlr_foreign_toplevel_management,
    },
};

impl<BackendData: Backend + 'static> WlrForeignToplevelHandler for Xfwl4State<BackendData> {
    fn wlr_foreign_toplevel_management_state(&mut self) -> &mut WlrForeignToplevelManagementState {
        &mut self
            .core
            .protocol_delegates
            .foreign_toplevel_state
            .wlr_foreign_toplevel_management_state
    }

    fn on_toplevel_set_maximized(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self
            .core
            .protocol_delegates
            .foreign_toplevel_state
            .wlr_windows
            .get(toplevel_id)
            .cloned()
        {
            self.set_window_maximized(&window);
        }
    }

    fn on_toplevel_unset_maximized(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self
            .core
            .protocol_delegates
            .foreign_toplevel_state
            .wlr_windows
            .get(toplevel_id)
            .cloned()
        {
            self.set_window_unmaximized(&window, None);
        }
    }

    fn on_toplevel_set_minimized(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self
            .core
            .protocol_delegates
            .foreign_toplevel_state
            .wlr_windows
            .get(toplevel_id)
            .cloned()
        {
            self.set_window_minimized(&window);
        }
    }

    fn on_toplevel_unset_minimized(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self
            .core
            .protocol_delegates
            .foreign_toplevel_state
            .wlr_windows
            .get(toplevel_id)
            .cloned()
        {
            self.set_window_unminimized(&window, SERIAL_COUNTER.next_serial(), true);
        }
    }

    fn on_toplevel_activate(&mut self, toplevel_id: &ToplevelId, wl_seat: &WlSeat) {
        if let Some(window) = self
            .core
            .protocol_delegates
            .foreign_toplevel_state
            .wlr_windows
            .get(toplevel_id)
            .cloned()
        {
            let seat = Seat::from_resource(wl_seat);
            self.activate_window(&window, true, seat);
        }
    }

    fn on_toplevel_close(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self
            .core
            .protocol_delegates
            .foreign_toplevel_state
            .wlr_windows
            .get(toplevel_id)
            .cloned()
        {
            window.close();
        }
    }

    fn on_toplevel_set_rectangle(&mut self, _toplevel_id: &ToplevelId, _wl_surface: &WlSurface, _rect: Rectangle<i32, Logical>) {
        // Currently don't do anything with this
    }

    fn on_toplevel_set_fullscreen(&mut self, toplevel_id: &ToplevelId, wl_output: Option<&WlOutput>) {
        if let Some(window) = self
            .core
            .protocol_delegates
            .foreign_toplevel_state
            .wlr_windows
            .get(toplevel_id)
            .cloned()
        {
            let output = wl_output.and_then(Output::from_resource);
            self.set_window_fullscreen(&window, output);
        }
    }

    fn on_toplevel_unset_fullscreen(&mut self, toplevel_id: &ToplevelId) {
        if let Some(window) = self
            .core
            .protocol_delegates
            .foreign_toplevel_state
            .wlr_windows
            .get(toplevel_id)
            .cloned()
        {
            self.set_window_unfullscreen(&window);
        }
    }
}

delegate_wlr_foreign_toplevel_management!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
