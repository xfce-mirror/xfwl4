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
    reexports::wayland_server::{
        Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
        backend::{ClientId, GlobalId},
    },
    utils::{SealedFile, Serial},
    wayland::{Dispatch2, GlobalDispatch2},
};

use crate::protocols::output_management::xfce_output_management::proto::{
    xfce_output_head_private_v1::XfceOutputHeadPrivateV1, xfce_output_manager_private_v1::XfceOutputManagerPrivateV1,
};
use crate::protocols::{ClientFilter, GlobalData};

pub struct XfceOutputManagementGlobalData {
    filter: ClientFilter,
}

pub struct XfceOutputManagementState {
    dh: DisplayHandle,
    _global: GlobalId,
    manager_instances: Vec<XfceOutputManagerPrivateV1>,
    heads: Vec<XfceHead>,
    last_config_serial: Option<Serial>,
}

pub trait XfceOutputManagementHandler: 'static {
    fn xfce_output_management_state(&mut self) -> &mut XfceOutputManagementState;
}

struct XfceHead {
    instances: Vec<XfceOutputHeadPrivateV1>,
    output: Output,
    edid: Bytes,
}

impl XfceOutputManagementState {
    pub fn new<H, F>(dh: &DisplayHandle, filter: F) -> Self
    where
        H: XfceOutputManagementHandler + GlobalDispatch<XfceOutputManagerPrivateV1, XfceOutputManagementGlobalData>,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global = dh.create_global::<H, XfceOutputManagerPrivateV1, _>(1, XfceOutputManagementGlobalData { filter: Box::new(filter) });
        Self {
            dh: dh.clone(),
            _global: global,
            manager_instances: Vec::new(),
            heads: Vec::new(),
            last_config_serial: None,
        }
    }

    pub fn output_created<H: XfceOutputManagementHandler + Dispatch<XfceOutputHeadPrivateV1, GlobalData>>(
        &mut self,
        output: &Output,
        edid: Bytes,
        new_config_serial: Serial,
    ) {
        let mut head = XfceHead::new(output, edid);

        for instance in &self.manager_instances {
            if let Some(client) = instance.client()
                && let Err(err) = create_and_send_head::<H>(&self.dh, &client, instance, &mut head)
            {
                tracing::info!("Failed to send new head to client {:?}: {err}", client.id());
            }
        }

        self.heads.push(head);
        self.send_done(new_config_serial);
    }

    pub fn output_changed<H: XfceOutputManagementHandler>(&mut self, _output: &Output, _is_enabled: bool, new_config_serial: Serial) {
        if self.last_config_serial != Some(new_config_serial) {
            self.send_done(new_config_serial);
        }
    }

    pub fn output_destroyed(&mut self, output: &Output, new_config_serial: Serial) {
        let old_len = self.heads.len();
        self.heads.retain(|head| &head.output != output);

        if old_len != self.heads.len() || self.last_config_serial != Some(new_config_serial) {
            self.send_done(new_config_serial);
        }
    }

    fn send_done(&mut self, new_config_serial: Serial) {
        for instance in &self.manager_instances {
            instance.done(new_config_serial.into());
        }
        self.last_config_serial = Some(new_config_serial);
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

impl<D: XfceOutputManagementHandler> GlobalDispatch2<XfceOutputManagerPrivateV1, D> for XfceOutputManagementGlobalData
where
    D: Dispatch<XfceOutputManagerPrivateV1, GlobalData> + Dispatch<XfceOutputHeadPrivateV1, GlobalData>,
{
    fn bind(
        &self,
        state: &mut D,
        handle: &DisplayHandle,
        client: &Client,
        resource: New<XfceOutputManagerPrivateV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        let instance = data_init.init(resource, GlobalData);

        let xfce_state = state.xfce_output_management_state();
        for head in xfce_state.heads.iter_mut() {
            if let Err(err) = create_and_send_head::<D>(handle, client, &instance, head) {
                tracing::info!("Failed to send head to client on new bind: {err}");
            }
        }
        xfce_state.manager_instances.push(instance.clone());

        if let Some(last_config_serial) = xfce_state.last_config_serial {
            instance.done(last_config_serial.into());
        }
    }

    fn can_view(&self, client: &Client) -> bool {
        (self.filter)(client)
    }
}

impl<D: XfceOutputManagementHandler> Dispatch2<XfceOutputManagerPrivateV1, D> for GlobalData {
    fn request(
        &self,
        state: &mut D,
        client: &Client,
        resource: &XfceOutputManagerPrivateV1,
        request: <XfceOutputManagerPrivateV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        use crate::protocols::output_management::xfce_output_management::proto::xfce_output_manager_private_v1::Request;

        match request {
            Request::Stop => self.destroyed(state, client.id(), resource),
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &XfceOutputManagerPrivateV1) {
        state
            .xfce_output_management_state()
            .manager_instances
            .retain(|manager| manager != resource);
    }
}

impl<D: XfceOutputManagementHandler> Dispatch2<XfceOutputHeadPrivateV1, D> for GlobalData {
    fn request(
        &self,
        state: &mut D,
        client: &Client,
        resource: &XfceOutputHeadPrivateV1,
        request: <XfceOutputHeadPrivateV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        use crate::protocols::output_management::xfce_output_management::proto::xfce_output_head_private_v1::Request;

        match request {
            Request::Release => self.destroyed(state, client.id(), resource),
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &XfceOutputHeadPrivateV1) {
        for head in state.xfce_output_management_state().heads.iter_mut() {
            head.instances.retain(|instance| instance != resource);
        }
    }
}

fn create_and_send_head<H: XfceOutputManagementHandler + Dispatch<XfceOutputHeadPrivateV1, GlobalData>>(
    dh: &DisplayHandle,
    client: &Client,
    manager_instance: &XfceOutputManagerPrivateV1,
    head: &mut XfceHead,
) -> anyhow::Result<()> {
    let instance = client.create_resource::<XfceOutputHeadPrivateV1, _, H>(dh, manager_instance.version(), GlobalData)?;
    manager_instance.head(&instance, head.output.name());
    head.send_initial(&instance);
    head.instances.push(instance);

    Ok(())
}

pub mod proto {
    use smithay::reexports::wayland_server;

    pub mod __interfaces {
        use smithay::reexports::wayland_server::backend as wayland_backend;

        wayland_scanner::generate_interfaces!("./resources/xfce-wayland-protocols/xfce-output-management-private-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("./resources/xfce-wayland-protocols/xfce-output-management-private-v1.xml");
}
