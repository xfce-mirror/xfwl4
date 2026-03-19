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

use std::os::fd::AsFd;

use bytes::Bytes;
use smithay::{
    output::{Output, WeakOutput},
    reexports::wayland_server::{
        Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
        backend::{ClientId, GlobalId},
    },
    utils::SealedFile,
};

use crate::protocols::output_management::xfce_output_management::proto::{
    xfce_output_head_private_v1::XfceOutputHeadPrivateV1, xfce_output_manager_private_v1::XfceOutputManagerPrivateV1,
};

pub struct XfceOutputManagementState {
    _global: GlobalId,
    manager_instances: Vec<XfceOutputManagerPrivateV1>,
    heads: Vec<XfceOutputHead>,
}

pub trait XfceOutputManagementHandler
where
    Self: GlobalDispatch<XfceOutputManagerPrivateV1, Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>>
        + Dispatch<XfceOutputManagerPrivateV1, ()>
        + Dispatch<XfceOutputHeadPrivateV1, ()>
        + Sized
        + 'static,
{
    fn xfce_output_management_state(&mut self) -> &mut XfceOutputManagementState;
}

struct XfceOutputHead {
    instances: Vec<XfceOutputHeadPrivateV1>,
    output: Output,
    edid: Bytes,
}

impl XfceOutputManagementState {
    pub fn new<H, F>(dh: &DisplayHandle, filter: F) -> Self
    where
        H: XfceOutputManagementHandler,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global = dh.create_global::<H, XfceOutputManagerPrivateV1, _>(1, Box::new(filter));
        Self {
            _global: global,
            manager_instances: Vec::new(),
            heads: Vec::new(),
        }
    }

    pub fn output_created<H: XfceOutputManagementHandler>(&mut self, output: &Output, edid: Bytes) {
        self.heads.push(XfceOutputHead {
            instances: Vec::new(),
            output: output.clone(),
            edid,
        });
    }

    pub fn output_destroyed(&mut self, output: &Output) {
        if let Some(pos) = self.heads.iter().position(|head| head.output == *output) {
            let head = self.heads.remove(pos);
            for instance in head.instances {
                instance.finished();
            }
        }
    }
}

impl<H: XfceOutputManagementHandler> GlobalDispatch<XfceOutputManagerPrivateV1, Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>, H>
    for XfceOutputManagementState
{
    fn bind(
        state: &mut H,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<XfceOutputManagerPrivateV1>,
        _global_data: &Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
        data_init: &mut DataInit<'_, H>,
    ) {
        let manager = data_init.init::<XfceOutputManagerPrivateV1, _>(resource, ());
        state.xfce_output_management_state().manager_instances.push(manager);
    }

    fn can_view(client: Client, global_data: &Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>) -> bool {
        global_data(&client)
    }
}

impl<H: XfceOutputManagementHandler> Dispatch<XfceOutputManagerPrivateV1, (), H> for XfceOutputManagementState {
    fn request(
        state: &mut H,
        client: &Client,
        resource: &XfceOutputManagerPrivateV1,
        request: <XfceOutputManagerPrivateV1 as Resource>::Request,
        data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, H>,
    ) {
        use crate::protocols::output_management::xfce_output_management::proto::xfce_output_manager_private_v1::Request;

        match request {
            Request::GetXfceOutputHead { id, head } => {
                let head_instance = data_init.init::<XfceOutputHeadPrivateV1, _>(id, ());
                if let Some(output) = head.data::<WeakOutput>().and_then(|weak| weak.upgrade())
                    && let Some(head) = state
                        .xfce_output_management_state()
                        .heads
                        .iter_mut()
                        .find(|head| head.output == output)
                {
                    match SealedFile::with_data(c"edid", &head.edid)
                        .map_err(anyhow::Error::from)
                        .and_then(|fd| u32::try_from(fd.size()).map_err(anyhow::Error::from).map(|size| (fd, size)))
                    {
                        Ok((fd, size)) => head_instance.edid(fd.as_fd(), size),
                        Err(err) => tracing::warn!("Failed to create memfd/shm FD for EDID transfer: {err}"),
                    }
                    head_instance.done();
                    head.instances.push(head_instance);
                } else {
                    head_instance.finished();
                }
            }

            Request::Destroy => <Self as Dispatch<XfceOutputManagerPrivateV1, (), H>>::destroyed(state, client.id(), resource, data),
        }
    }

    fn destroyed(state: &mut H, _client: ClientId, resource: &XfceOutputManagerPrivateV1, _data: &()) {
        state
            .xfce_output_management_state()
            .manager_instances
            .retain(|manager| manager != resource);
    }
}

impl<H: XfceOutputManagementHandler> Dispatch<XfceOutputHeadPrivateV1, (), H> for XfceOutputManagementState {
    fn request(
        state: &mut H,
        client: &Client,
        resource: &XfceOutputHeadPrivateV1,
        request: <XfceOutputHeadPrivateV1 as Resource>::Request,
        data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, H>,
    ) {
        use crate::protocols::output_management::xfce_output_management::proto::xfce_output_head_private_v1::Request;

        match request {
            Request::Release => <Self as Dispatch<XfceOutputHeadPrivateV1, (), H>>::destroyed(state, client.id(), resource, data),
        }
    }

    fn destroyed(state: &mut H, _client: ClientId, resource: &XfceOutputHeadPrivateV1, _data: &()) {
        for head in state.xfce_output_management_state().heads.iter_mut() {
            head.instances.retain(|instance| instance != resource);
        }
    }
}

macro_rules! delegate_xfce_output_management {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            crate::protocols::output_management::xfce_output_management::proto::xfce_output_manager_private_v1::XfceOutputManagerPrivateV1: Box<dyn for<'c> Fn(&'c smithay::reexports::wayland_server::Client) -> bool + Send + Sync>
        ] => $crate::protocols::output_management::xfce_output_management::XfceOutputManagementState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            crate::protocols::output_management::xfce_output_management::proto::xfce_output_manager_private_v1::XfceOutputManagerPrivateV1: ()
        ] => $crate::protocols::output_management::xfce_output_management::XfceOutputManagementState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            crate::protocols::output_management::xfce_output_management::proto::xfce_output_head_private_v1::XfceOutputHeadPrivateV1: ()
        ] => $crate::protocols::output_management::xfce_output_management::XfceOutputManagementState);
    };
}

pub(crate) use delegate_xfce_output_management;

pub mod proto {
    use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::*;
    use smithay::reexports::wayland_server;

    pub mod __interfaces {
        use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::__interfaces::*;
        use smithay::reexports::wayland_server::backend as wayland_backend;

        wayland_scanner::generate_interfaces!("./resources/xfce-wayland-protocols/xfce-output-management-private-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("./resources/xfce-wayland-protocols/xfce-output-management-private-v1.xml");
}
