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
//
// Portions of this file are based on "anvil", an example compositor
// based on the smithay crate, and are licensed under the MIT license
// with the following terms:
//
// Copyright (C) Victor Berger <victor.berger@m4x.org>
// Copyright (C) Drakulix (Victoria Brekenfeld)
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use smithay::{
    delegate_seat,
    input::{Seat, SeatHandler, SeatState, keyboard::LedState, pointer::CursorImageStatus},
    reexports::wayland_server::Resource,
    wayland::{
        seat::WaylandFocus,
        selection::{data_device::set_data_device_focus, primary_selection::set_primary_focus},
    },
};

use crate::{
    backend::Backend,
    core::{
        focus::{KeyboardFocusTarget, PointerFocusTarget},
        state::Xfwl4State,
    },
};

impl<BackendData: Backend> SeatHandler for Xfwl4State<BackendData> {
    type KeyboardFocus = KeyboardFocusTarget;
    type PointerFocus = PointerFocusTarget;
    type TouchFocus = PointerFocusTarget;

    fn seat_state(&mut self) -> &mut SeatState<Xfwl4State<BackendData>> {
        &mut self.core.protocol_delegates.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, target: Option<&KeyboardFocusTarget>) {
        let dh = &self.core.display_handle;

        let wl_surface = target.and_then(WaylandFocus::wl_surface);

        let focus = wl_surface.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, focus.clone());
        set_primary_focus(dh, seat, focus);
    }
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        if let CursorImageStatus::Surface(ref cursor_surface) = image
            && let Some(anchor) = self.core.window_menu_anchor.as_ref()
            && self.core.workspace_manager.active_workspace().window_location(anchor).is_some()
            && let Some(anchor_surface) = anchor.wl_surface()
            && let Ok(cursor_client) = self.core.display_handle.get_client(cursor_surface.id())
            && let Ok(anchor_client) = self.core.display_handle.get_client(anchor_surface.id())
            && cursor_client == anchor_client
        {
            // Ignore when GTK/GDK tries to set the cursor when we pop up the window menu, because
            // the cursor GTK sets has a different hotspot than our default cursor that makes it
            // look like the pointer warps a little, which is really jarring and looks bad.
            self.core.pointer_element.set_status(CursorImageStatus::default_named());
        } else {
            self.core.pointer_element.set_status(image);
        }
    }

    fn led_state_changed(&mut self, _seat: &Seat<Self>, led_state: LedState) {
        if let Some(numlock_on) = led_state.num {
            self.core.keyboard_config.store_numlock_state(numlock_on);
        }
        self.backend.update_led_state(led_state)
    }
}

delegate_seat!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
