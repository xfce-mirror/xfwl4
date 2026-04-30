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
    wayland::{Dispatch2, GlobalDispatch2},
};

use crate::protocols::{ClientFilter, GlobalData};

pub struct WlrOutputPowerManagementGlobalData {
    filter: ClientFilter,
}

pub struct WlrOutputPowerManagementState {
    dh: DisplayHandle,
    _global: GlobalId,
    manager_instances: Vec<ZwlrOutputPowerManagerV1>,
    output_powers: Vec<WlrOutputPower>,
}

pub trait WlrOutputPowerManagementHandler: 'static {
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
        H: WlrOutputPowerManagementHandler + GlobalDispatch<ZwlrOutputPowerManagerV1, WlrOutputPowerManagementGlobalData>,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global = dh.create_global::<H, ZwlrOutputPowerManagerV1, _>(1, WlrOutputPowerManagementGlobalData { filter: Box::new(filter) });
        Self {
            dh: dh.clone(),
            _global: global,
            manager_instances: Vec::new(),
            output_powers: Vec::new(),
        }
    }

    pub fn output_created<H: WlrOutputPowerManagementHandler + Dispatch<ZwlrOutputPowerV1, GlobalData>>(
        &mut self,
        output: &Output,
        cur_mode: PowerMode,
    ) {
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

impl<D: WlrOutputPowerManagementHandler> GlobalDispatch2<ZwlrOutputPowerManagerV1, D> for WlrOutputPowerManagementGlobalData
where
    D: Dispatch<ZwlrOutputPowerManagerV1, GlobalData>,
{
    fn bind(
        &self,
        state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrOutputPowerManagerV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        let instance = data_init.init(resource, GlobalData);
        state.wlr_output_power_management_state().manager_instances.push(instance);
    }

    fn can_view(&self, client: &Client) -> bool {
        (self.filter)(client)
    }
}

impl<D: WlrOutputPowerManagementHandler> Dispatch2<ZwlrOutputPowerManagerV1, D> for GlobalData
where
    D: Dispatch<ZwlrOutputPowerV1, GlobalData>,
{
    fn request(
        &self,
        state: &mut D,
        client: &Client,
        resource: &ZwlrOutputPowerManagerV1,
        request: <ZwlrOutputPowerManagerV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use smithay::reexports::wayland_protocols_wlr::output_power_management::v1::server::zwlr_output_power_manager_v1::Request;

        match request {
            Request::GetOutputPower { id, output } => {
                let instance = data_init.init(id, GlobalData);

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

            Request::Destroy => {
                self.destroyed(state, client.id(), resource);
            }

            _ => (),
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &ZwlrOutputPowerManagerV1) {
        state
            .wlr_output_power_management_state()
            .manager_instances
            .retain(|instance| instance != resource);
    }
}

impl<D: WlrOutputPowerManagementHandler> Dispatch2<ZwlrOutputPowerV1, D> for GlobalData {
    fn request(
        &self,
        state: &mut D,
        client: &Client,
        resource: &ZwlrOutputPowerV1,
        request: <ZwlrOutputPowerV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
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
                            Err(_) => self.destroyed(state, client.id(), resource),
                        },

                        WEnum::Unknown(v) => {
                            resource.post_error(Error::InvalidMode, format!("unknown power management mode {v}"));
                        }
                    }
                }
            }

            Request::Destroy => {
                self.destroyed(state, client.id(), resource);
            }

            _ => (),
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &ZwlrOutputPowerV1) {
        for power in &mut state.wlr_output_power_management_state().output_powers {
            power.instances.retain(|instance| instance != resource);
        }
    }
}

fn send_power<H: WlrOutputPowerManagementHandler + Dispatch<ZwlrOutputPowerV1, GlobalData>>(
    dh: &DisplayHandle,
    client: &Client,
    manager: &ZwlrOutputPowerManagerV1,
    power: &mut WlrOutputPower,
) -> anyhow::Result<()> {
    let instance = client.create_resource::<ZwlrOutputPowerV1, _, H>(dh, manager.version(), GlobalData)?;
    instance.mode(power.cur_mode);
    power.instances.push(instance);

    Ok(())
}
