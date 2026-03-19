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
    output::Output,
    reexports::{
        wayland_protocols_wlr::output_management::v1::server::zwlr_output_head_v1::ZwlrOutputHeadV1,
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::{ClientId, GlobalId},
        },
    },
    utils::{SealedFile, Serial},
};

use crate::protocols::output_management::{
    wlr_output_management::{WlrHead, WlrOutputManagementHandler, WlrOutputManagementState},
    xfce_output_management::proto::{
        xfce_output_head_private_v1::XfceOutputHeadPrivateV1, xfce_output_manager_private_v1::XfceOutputManagerPrivateV1,
    },
};

const WLR_HEAD_INTERFACE_VERSION: u32 = 4;

pub struct XfceOutputManagementState {
    dh: DisplayHandle,
    _global: GlobalId,
    pub(super) wlr_output_management_state: WlrOutputManagementState,
    manager_instances: Vec<XfceOutputManagerPrivateV1>,
    heads: Vec<(WlrHead, XfceHead)>,
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

struct XfceHead {
    instances: Vec<(ZwlrOutputHeadV1, XfceOutputHeadPrivateV1)>,
    output: Output,
    edid: Bytes,
}

impl XfceOutputManagementState {
    pub fn new<H, F>(dh: &DisplayHandle, filter: F) -> Self
    where
        H: XfceOutputManagementHandler + WlrOutputManagementHandler,
        F: for<'c> Fn(&'c Client) -> bool + Clone + Send + Sync + 'static,
    {
        let global = dh.create_global::<H, XfceOutputManagerPrivateV1, _>(1, Box::new(filter.clone()));
        Self {
            dh: dh.clone(),
            _global: global,
            wlr_output_management_state: WlrOutputManagementState::new::<H, _>(dh, filter),
            manager_instances: Vec::new(),
            heads: Vec::new(),
        }
    }

    pub fn output_created<H: XfceOutputManagementHandler + WlrOutputManagementHandler>(&mut self, output: &Output, edid: Bytes) {
        let cur_config_serial = self.wlr_output_management_state.output_created_return_serial::<H>(output);

        let mut wlr_head = WlrHead::new(output);
        let mut xfce_head = XfceHead::new(output, edid);

        for instance in &self.manager_instances {
            if let Some(client) = instance.client()
                && let Err(err) = create_and_send_heads::<H>(&self.dh, &client, instance, &mut wlr_head, &mut xfce_head) {
                    tracing::info!("Failed to send new head to client {:?}: {err}", client.id());
                }

            instance.done(cur_config_serial.into());
        }

        self.heads.push((wlr_head, xfce_head));
    }

    pub fn output_changed<H: WlrOutputManagementHandler>(&mut self, output: &Output, is_enabled: bool) {
        if let Some(new_config_serial) = self
            .wlr_output_management_state
            .output_changed_return_serial::<H>(output, is_enabled)
        {
            if let Some((wlr_head, _)) = self.heads.iter_mut().find(|(_, xfce_head)| xfce_head.output == *output) {
                wlr_head.changed::<H>(&self.dh, output, is_enabled);
            }

            self.post_configuration_change(new_config_serial);
        }
    }

    pub fn output_destroyed(&mut self, output: &Output) {
        if let Some(new_config_serial) = self.wlr_output_management_state.output_destroyed_return_serial(output) {
            self.heads.retain(|head| head.1.output != *output);
            self.post_configuration_change(new_config_serial);
        }
    }

    fn post_configuration_change(&mut self, new_config_serial: Serial) {
        for instance in &self.manager_instances {
            instance.done(new_config_serial.into());
        }
    }
}

impl XfceHead {
    fn new(output: &Output, edid: Bytes) -> Self {
        Self {
            instances: Vec::new(),
            output: output.clone(),
            edid,
        }
    }

    fn send_initial(&self, instance: &XfceOutputHeadPrivateV1) {
        match SealedFile::with_data(c"edid", &self.edid)
            .map_err(anyhow::Error::from)
            .and_then(|fd| u32::try_from(fd.size()).map_err(anyhow::Error::from).map(|size| (fd, size)))
        {
            Ok((fd, size)) => instance.edid(fd.as_fd(), size),
            Err(err) => tracing::warn!("Failed to create memfd/shm FD for EDID transfer: {err}"),
        }
    }
}

impl<H: XfceOutputManagementHandler + WlrOutputManagementHandler>
    GlobalDispatch<XfceOutputManagerPrivateV1, Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>, H> for XfceOutputManagementState
{
    fn bind(
        state: &mut H,
        handle: &DisplayHandle,
        client: &Client,
        resource: New<XfceOutputManagerPrivateV1>,
        _global_data: &Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
        data_init: &mut DataInit<'_, H>,
    ) {
        let instance = data_init.init(resource, ());

        let xfce_state = state.xfce_output_management_state();
        for (wlr_head, xfce_head) in xfce_state.heads.iter_mut() {
            if let Err(err) = create_and_send_heads::<H>(handle, client, &instance, wlr_head, xfce_head) {
                tracing::info!("Failed to send head to client on new bind: {err}");
            }
        }
        xfce_state.manager_instances.push(instance.clone());

        let wlr_state = state.wlr_output_management_state();
        instance.done(wlr_state.cur_config_serial().into());
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
        _data_init: &mut DataInit<'_, H>,
    ) {
        use crate::protocols::output_management::xfce_output_management::proto::xfce_output_manager_private_v1::Request;

        match request {
            Request::Stop => <Self as Dispatch<XfceOutputManagerPrivateV1, (), H>>::destroyed(state, client.id(), resource, data),
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
        for (_, xfce_head) in state.xfce_output_management_state().heads.iter_mut() {
            xfce_head.instances.retain(|(_, instance)| instance != resource);
        }
    }
}

fn create_and_send_heads<H: XfceOutputManagementHandler + WlrOutputManagementHandler>(
    dh: &DisplayHandle,
    client: &Client,
    manager_instance: &XfceOutputManagerPrivateV1,
    wlr_head: &mut WlrHead,
    xfce_head: &mut XfceHead,
) -> anyhow::Result<()> {
    let wlr_instance = client.create_resource::<ZwlrOutputHeadV1, _, H>(dh, WLR_HEAD_INTERFACE_VERSION, wlr_head.data())?;
    manager_instance.wlr_head(&wlr_instance);
    let xfce_instance = client.create_resource::<XfceOutputHeadPrivateV1, _, H>(dh, manager_instance.version(), ())?;
    manager_instance.xfce_head(&xfce_instance, &wlr_instance);

    wlr_head.send_initial::<H>(dh, client, &wlr_instance)?;
    xfce_head.send_initial(&xfce_instance);

    xfce_head.instances.push((wlr_instance, xfce_instance));

    Ok(())
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
