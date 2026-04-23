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

#[cfg(feature = "xwayland")]
use std::os::fd::OwnedFd;

#[cfg(feature = "xwayland")]
use smithay::wayland::selection::{SelectionSource, SelectionTarget};
use smithay::{
    delegate_data_control, delegate_data_device, delegate_primary_selection,
    input::{
        Seat,
        dnd::{DnDGrab, DndGrabHandler, DndTarget, GrabType, Source},
        pointer::Focus,
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Serial},
    wayland::selection::{
        SelectionHandler,
        data_device::{DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler},
        primary_selection::{PrimarySelectionHandler, PrimarySelectionState},
        wlr_data_control::{DataControlHandler, DataControlState},
    },
};
use tracing::warn;

use crate::{backend::Backend, core::state::Xfwl4State};

#[derive(Debug)]
pub struct DndIcon {
    pub surface: WlSurface,
    pub offset: Point<i32, Logical>,
}

impl<BackendData: Backend> DataDeviceHandler for Xfwl4State<BackendData> {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.core.protocol_delegates.data_device_state
    }
}

impl<BackendData: Backend> WaylandDndGrabHandler for Xfwl4State<BackendData> {
    fn dnd_requested<S: Source>(&mut self, source: S, icon: Option<WlSurface>, seat: Seat<Self>, serial: Serial, type_: GrabType) {
        self.core.dnd_icon = icon.map(|surface| DndIcon {
            surface,
            offset: (0, 0).into(),
        });

        match type_ {
            GrabType::Pointer => {
                let pointer = seat.get_pointer().unwrap();
                let start_data = pointer.grab_start_data().unwrap();
                pointer.set_grab(
                    self,
                    DnDGrab::new_pointer(&self.core.display_handle, start_data, source, seat),
                    serial,
                    Focus::Keep,
                );
            }
            GrabType::Touch => {
                let touch = seat.get_touch().unwrap();
                let start_data = touch.grab_start_data().unwrap();
                touch.set_grab(
                    self,
                    DnDGrab::new_touch(&self.core.display_handle, start_data, source, seat),
                    serial,
                );
            }
        }
    }
}

impl<BackendData: Backend> DndGrabHandler for Xfwl4State<BackendData> {
    fn dropped(&mut self, _target: Option<DndTarget<'_, Self>>, _validated: bool, _seat: Seat<Self>, _location: Point<f64, Logical>) {
        self.core.dnd_icon = None;
    }
}
delegate_data_device!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);

impl<BackendData: Backend> SelectionHandler for Xfwl4State<BackendData> {
    type SelectionUserData = ();

    #[cfg(feature = "xwayland")]
    fn new_selection(&mut self, ty: SelectionTarget, source: Option<SelectionSource>, _seat: Seat<Self>) {
        if let Some(xw) = self.core.xwayland.as_mut()
            && let Err(err) = xw.xwm().new_selection(ty, source.map(|source| source.mime_types()))
        {
            warn!(?err, ?ty, "Failed to set Xwayland selection");
        }
    }

    #[cfg(feature = "xwayland")]
    fn send_selection(&mut self, ty: SelectionTarget, mime_type: String, fd: OwnedFd, _seat: Seat<Self>, _user_data: &()) {
        if let Some(xw) = self.core.xwayland.as_mut()
            && let Err(err) = xw.xwm().send_selection(ty, mime_type, fd)
        {
            warn!(?err, "Failed to send primary (X11 -> Wayland)");
        }
    }
}

impl<BackendData: Backend> PrimarySelectionHandler for Xfwl4State<BackendData> {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.core.protocol_delegates.primary_selection_state
    }
}
delegate_primary_selection!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);

impl<BackendData: Backend> DataControlHandler for Xfwl4State<BackendData> {
    fn data_control_state(&mut self) -> &mut DataControlState {
        &mut self.core.protocol_delegates.data_control_state
    }
}

delegate_data_control!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
