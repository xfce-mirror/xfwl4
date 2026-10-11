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

use std::collections::HashSet;

use smithay::{
    input::{Seat, SeatHandler},
    reexports::wayland_server::{
        Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
        backend::{ClientId, GlobalId},
    },
    wayland::{Dispatch2, GlobalDispatch2},
};

use crate::protocols::{GlobalData, xfce_accessibility::proto::xfce_accessibility_manager_private_v1::XfceAccessibilityManagerPrivateV1};

const PROTO_VERSION: u32 = 1;

pub struct XfceAccessibilityState<H: XfceAccessibilityHandler> {
    _global: GlobalId,
    manager_instances: Vec<XfceAccessibilityManagerPrivateV1>,
    find_pointer_in_progress: HashSet<Seat<H>>,
}

pub trait XfceAccessibilityHandler: SeatHandler + Sized + 'static {
    fn xfce_accessibility_state(&mut self) -> &mut XfceAccessibilityState<Self>;
    fn find_pointer(&mut self, seat: Seat<Self>);
}

impl<H: XfceAccessibilityHandler> XfceAccessibilityState<H> {
    pub fn new(dh: &DisplayHandle) -> Self
    where
        H: GlobalDispatch<XfceAccessibilityManagerPrivateV1, GlobalData, H>,
    {
        let _global = dh.create_global::<H, XfceAccessibilityManagerPrivateV1, _>(PROTO_VERSION, GlobalData);
        Self {
            _global,
            manager_instances: Vec::new(),
            find_pointer_in_progress: HashSet::new(),
        }
    }

    pub fn find_pointer_completed(&mut self, seat: &Seat<H>) {
        self.find_pointer_in_progress.remove(seat);
    }
}

impl<H> GlobalDispatch2<XfceAccessibilityManagerPrivateV1, H> for GlobalData
where
    H: XfceAccessibilityHandler + Dispatch<XfceAccessibilityManagerPrivateV1, GlobalData>,
{
    fn bind(
        &self,
        state: &mut H,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<XfceAccessibilityManagerPrivateV1>,
        data_init: &mut DataInit<'_, H>,
    ) {
        let instance = data_init.init(resource, GlobalData);
        state.xfce_accessibility_state().manager_instances.push(instance);
    }
}

impl<H> Dispatch2<XfceAccessibilityManagerPrivateV1, H> for GlobalData
where
    H: XfceAccessibilityHandler,
{
    fn request(
        &self,
        state: &mut H,
        _client: &Client,
        _resource: &XfceAccessibilityManagerPrivateV1,
        request: <XfceAccessibilityManagerPrivateV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, H>,
    ) {
        use proto::xfce_accessibility_manager_private_v1::Request;

        match request {
            Request::FindPointer { seat } => {
                if let Some(seat) = Seat::<H>::from_resource(&seat)
                    && !state.xfce_accessibility_state().find_pointer_in_progress.contains(&seat)
                {
                    state.xfce_accessibility_state().find_pointer_in_progress.insert(seat.clone());
                    state.find_pointer(seat);
                }
            }
        }
    }

    fn destroyed(&self, state: &mut H, _client: ClientId, resource: &XfceAccessibilityManagerPrivateV1) {
        state
            .xfce_accessibility_state()
            .manager_instances
            .retain(|instance| instance != resource);
    }
}

pub mod proto {
    use smithay::reexports::wayland_server::{self, protocol::wl_seat};

    pub mod __interfaces {
        use smithay::reexports::wayland_server::{
            backend as wayland_backend,
            protocol::__interfaces::{WL_SEAT_INTERFACE, wl_seat_interface},
        };

        wayland_scanner::generate_interfaces!("./resources/xfce-wayland-protocols/xfce-accessibility-private-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("./resources/xfce-wayland-protocols/xfce-accessibility-private-v1.xml");
}
