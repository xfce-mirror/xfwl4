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
    output::Output,
    reexports::{
        wayland_protocols_wlr::output_power_management::v1::server::{
            zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1,
            zwlr_output_power_v1::{Mode as PowerMode, ZwlrOutputPowerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
            backend::{ClientId, GlobalId},
        },
    },
};

pub struct WlrOutputPowerManagementState {
    dh: DisplayHandle,
    _global: GlobalId,
    manager_instances: Vec<ZwlrOutputPowerManagerV1>,
    output_powers: Vec<WlrOutputPower>,
}

pub trait WlrOutputPowerManagementHandler
where
    Self: GlobalDispatch<ZwlrOutputPowerManagerV1, Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>>
        + Dispatch<ZwlrOutputPowerManagerV1, ()>
        + Dispatch<ZwlrOutputPowerV1, ()>
        + Sized
        + 'static,
{
    fn wlr_output_power_management_state(&mut self) -> &mut WlrOutputPowerManagementState;

    fn on_set_mode(&mut self, output: &Output, new_mode: PowerMode) -> Result<(), WlrOutputPowerError>;
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum WlrOutputPowerError {
    #[error("Power management not supported on this output")]
    #[allow(unused)]
    Unsupported,
    #[error("Output not found")]
    OutputNotFound,
    #[error("Permission denied; another client may have exclusive access to power management for this output")]
    #[allow(unused)]
    PermissionDenied,
    #[error("Failed; try again later")]
    TransientFailure,
}

struct WlrOutputPower {
    instances: Vec<ZwlrOutputPowerV1>,
    output: Output,
    cur_mode: PowerMode,
}

impl WlrOutputPowerManagementState {
    pub fn new<H, F>(dh: &DisplayHandle, filter: F) -> Self
    where
        H: WlrOutputPowerManagementHandler,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global = dh.create_global::<H, _, _>(1, Box::new(filter));
        Self {
            dh: dh.clone(),
            _global: global,
            manager_instances: Vec::new(),
            output_powers: Vec::new(),
        }
    }

    pub fn output_created<H: WlrOutputPowerManagementHandler>(&mut self, output: &Output, cur_mode: PowerMode) {
        let mut power = WlrOutputPower {
            instances: Vec::new(),
            output: output.clone(),
            cur_mode,
        };

        for manager in &self.manager_instances {
            if let Some(client) = manager.client()
                && let Err(err) = send_power::<H>(&self.dh, &client, manager, &mut power)
            {
                tracing::info!("Failed to send output-power instance to client: {err}");
            }
        }

        self.output_powers.push(power);
    }

    pub fn output_changed(&mut self, output: &Output, new_mode: PowerMode) {
        if let Some(power) = self.output_powers.iter_mut().find(|power| power.output == *output)
            && power.cur_mode != new_mode
        {
            power.cur_mode = new_mode;
            for instance in &power.instances {
                instance.mode(new_mode);
            }
        }
    }

    pub fn output_destroyed(&mut self, output: &Output) {
        self.output_powers.retain(|power| power.output != *output);
    }
}

impl Drop for WlrOutputPower {
    fn drop(&mut self) {
        for instance in &self.instances {
            instance.failed();
        }
    }
}

impl<H: WlrOutputPowerManagementHandler> GlobalDispatch<ZwlrOutputPowerManagerV1, Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>, H>
    for WlrOutputPowerManagementState
{
    fn bind(
        state: &mut H,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrOutputPowerManagerV1>,
        _global_data: &Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
        data_init: &mut DataInit<'_, H>,
    ) {
        let instance = data_init.init(resource, ());
        state.wlr_output_power_management_state().manager_instances.push(instance);
    }

    fn can_view(client: Client, global_data: &Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>) -> bool {
        global_data(&client)
    }
}

impl<H: WlrOutputPowerManagementHandler> Dispatch<ZwlrOutputPowerManagerV1, (), H> for WlrOutputPowerManagementState {
    fn request(
        state: &mut H,
        client: &Client,
        resource: &ZwlrOutputPowerManagerV1,
        request: <ZwlrOutputPowerManagerV1 as Resource>::Request,
        data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, H>,
    ) {
        use smithay::reexports::wayland_protocols_wlr::output_power_management::v1::server::zwlr_output_power_manager_v1::Request;

        match request {
            Request::GetOutputPower { id, output } => {
                let instance = data_init.init(id, ());

                if let Some(output) = Output::from_resource(&output)
                    && let Some(power) = state
                        .wlr_output_power_management_state()
                        .output_powers
                        .iter_mut()
                        .find(|power| power.output == output)
                {
                    instance.mode(power.cur_mode);
                    power.instances.push(instance);
                } else {
                    instance.failed();
                }
            }

            Request::Destroy => <Self as Dispatch<ZwlrOutputPowerManagerV1, (), H>>::destroyed(state, client.id(), resource, data),

            _ => (),
        }
    }

    fn destroyed(state: &mut H, _client: ClientId, resource: &ZwlrOutputPowerManagerV1, _data: &()) {
        state
            .wlr_output_power_management_state()
            .manager_instances
            .retain(|instance| instance != resource);
    }
}

impl<H: WlrOutputPowerManagementHandler> Dispatch<ZwlrOutputPowerV1, (), H> for WlrOutputPowerManagementState {
    fn request(
        state: &mut H,
        client: &Client,
        resource: &ZwlrOutputPowerV1,
        request: <ZwlrOutputPowerV1 as Resource>::Request,
        data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, H>,
    ) {
        use smithay::reexports::wayland_protocols_wlr::output_power_management::v1::server::zwlr_output_power_v1::{Error, Request};

        match request {
            Request::SetMode { mode } => {
                if let Some(output) = state.wlr_output_power_management_state().output_powers.iter().find_map(|power| {
                    power
                        .instances
                        .iter()
                        .find_map(|instance| (instance == resource).then(|| power.output.clone()))
                }) {
                    match mode {
                        WEnum::Value(mode) => match state.on_set_mode(&output, mode) {
                            Ok(_) => (),
                            Err(WlrOutputPowerError::TransientFailure) => (),
                            Err(_) => <Self as Dispatch<ZwlrOutputPowerV1, (), H>>::destroyed(state, client.id(), resource, data),
                        },

                        WEnum::Unknown(v) => {
                            resource.post_error(Error::InvalidMode, format!("unknown power management mode {v}"));
                        }
                    }
                }
            }

            Request::Destroy => <Self as Dispatch<ZwlrOutputPowerV1, (), H>>::destroyed(state, client.id(), resource, data),

            _ => (),
        }
    }

    fn destroyed(state: &mut H, _client: ClientId, resource: &ZwlrOutputPowerV1, _data: &()) {
        for power in &mut state.wlr_output_power_management_state().output_powers {
            power.instances.retain(|instance| instance != resource);
        }
    }
}

fn send_power<H: WlrOutputPowerManagementHandler>(
    dh: &DisplayHandle,
    client: &Client,
    manager: &ZwlrOutputPowerManagerV1,
    power: &mut WlrOutputPower,
) -> anyhow::Result<()> {
    let instance = client.create_resource::<ZwlrOutputPowerV1, _, H>(dh, manager.version(), ())?;
    instance.mode(power.cur_mode);
    power.instances.push(instance);

    Ok(())
}

macro_rules! delegate_wlr_output_power_management {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::output_power_management::v1::server::zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1: Box<dyn for<'c> Fn(&'c smithay::reexports::wayland_server::Client) -> bool + Send + Sync>
        ] => $crate::protocols::wlr_output_power_management::WlrOutputPowerManagementState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::output_power_management::v1::server::zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1: ()
        ] => $crate::protocols::wlr_output_power_management::WlrOutputPowerManagementState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::output_power_management::v1::server::zwlr_output_power_v1::ZwlrOutputPowerV1: ()
        ] => $crate::protocols::wlr_output_power_management::WlrOutputPowerManagementState);
    };
}

pub(crate) use delegate_wlr_output_power_management;
