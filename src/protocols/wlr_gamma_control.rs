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

use std::{
    io::{self, Read},
    os::unix::io::OwnedFd,
};

use smithay::{
    output::{Output, WeakOutput},
    reexports::{
        wayland_protocols_wlr::gamma_control::v1::server::{
            zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1, zwlr_gamma_control_v1::ZwlrGammaControlV1,
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::{ClientId, GlobalId},
        },
    },
};

pub struct WlrGammaControlState {
    _global: GlobalId,
    manager_instances: Vec<ZwlrGammaControlManagerV1>,
    outputs: Vec<OutputInfo>,
}

// The protocol doesn't specify anything about this, but I've chosen to limit to a single gamma
// control object per output.  It seems somewhat nonsensical to have more than one client dueling
// to change the output gamma, and it makes restoring the original gamma on object destruction
// impossible to get right (if there even *is* a "right" there).
struct OutputInfo {
    gamma_control: Option<ZwlrGammaControlV1>,
    output: WeakOutput,
    orig_gamma: Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
    gamma_length: u32,
}

impl WlrGammaControlState {
    pub fn new<H: WlrGammaControlHandler, F>(dh: &DisplayHandle, filter: F) -> Self
    where
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global = dh.create_global::<H, ZwlrGammaControlManagerV1, _>(1, Box::new(filter));

        Self {
            _global: global,
            outputs: Default::default(),
            manager_instances: Default::default(),
        }
    }

    pub fn output_created(&mut self, output: &Output, orig_gamma: Option<(Vec<u16>, Vec<u16>, Vec<u16>)>, gamma_length: u32) {
        self.outputs.push(OutputInfo {
            gamma_control: None,
            output: output.downgrade(),
            orig_gamma,
            gamma_length,
        });
    }

    pub fn output_destroyed(&mut self, output: &Output) {
        self.outputs.retain(|info| &info.output != output);
    }
}

pub trait WlrGammaControlHandler
where
    Self: GlobalDispatch<ZwlrGammaControlManagerV1, Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>>
        + Dispatch<ZwlrGammaControlManagerV1, ()>
        + Dispatch<ZwlrGammaControlV1, ()>
        + Sized
        + 'static,
{
    fn wlr_gamma_control_state(&mut self) -> &mut WlrGammaControlState;

    fn set_output_gamma(&mut self, output: &Output, red: &[u16], green: &[u16], blue: &[u16]) -> bool;
}

impl<H: WlrGammaControlHandler> GlobalDispatch<ZwlrGammaControlManagerV1, Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>, H>
    for WlrGammaControlState
{
    fn bind(
        state: &mut H,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrGammaControlManagerV1>,
        _global_data: &Box<dyn Fn(&Client) -> bool + Send + Sync>,
        data_init: &mut DataInit<'_, H>,
    ) {
        let manager = data_init.init(resource, ());
        state.wlr_gamma_control_state().manager_instances.push(manager);
    }

    fn can_view(client: Client, global_data: &Box<dyn Fn(&Client) -> bool + Send + Sync>) -> bool {
        global_data(&client)
    }
}

impl<H: WlrGammaControlHandler> Dispatch<ZwlrGammaControlManagerV1, (), H> for WlrGammaControlState {
    fn request(
        state: &mut H,
        client: &Client,
        resource: &ZwlrGammaControlManagerV1,
        request: <ZwlrGammaControlManagerV1 as Resource>::Request,
        data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, H>,
    ) {
        use smithay::reexports::wayland_protocols_wlr::gamma_control::v1::server::zwlr_gamma_control_manager_v1::Request;
        match request {
            Request::GetGammaControl { id, output } => {
                let wlr_state = state.wlr_gamma_control_state();
                let gamma_control = data_init.init(id, ());

                if let Some(output_info) = wlr_state
                    .outputs
                    .iter_mut()
                    .find(|info| info.output.upgrade().is_some_and(|o| o.owns(&output)))
                {
                    if output_info.gamma_control.is_some() {
                        gamma_control.post_error(0u32, "there is already a zwlr_gamma_control_v1 object for this output");
                    } else {
                        gamma_control.gamma_size(output_info.gamma_length);
                        output_info.gamma_control = Some(gamma_control);
                    }
                } else {
                    gamma_control.post_error(0u32, "invalid output");
                }
            }

            Request::Destroy => {
                Self::destroyed(state, client.id(), resource, data);
            }

            _ => (),
        }
    }

    fn destroyed(state: &mut H, _client: ClientId, resource: &ZwlrGammaControlManagerV1, _data: &()) {
        state
            .wlr_gamma_control_state()
            .manager_instances
            .retain(|manager| manager != resource);
    }
}

impl<H: WlrGammaControlHandler> Dispatch<ZwlrGammaControlV1, (), H> for WlrGammaControlState {
    fn request(
        state: &mut H,
        client: &Client,
        resource: &ZwlrGammaControlV1,
        request: <ZwlrGammaControlV1 as Resource>::Request,
        data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, H>,
    ) {
        use smithay::reexports::wayland_protocols_wlr::gamma_control::v1::server::zwlr_gamma_control_v1::Request;
        match request {
            Request::SetGamma { fd } => {
                let success = if let Some((output, info)) = output_info_for_instance(state, resource) {
                    match read_gamma_ramps(fd, info.gamma_length as usize) {
                        Ok(ramps) => state.set_output_gamma(&output, &ramps.0, &ramps.1, &ramps.2),
                        Err(err) => {
                            tracing::info!("Failed to read gamma ramps from client: {err}");
                            false
                        }
                    }
                } else {
                    false
                };

                if !success {
                    resource.failed();
                }
            }

            Request::Destroy => {
                Self::destroyed(state, client.id(), resource, data);
            }

            _ => (),
        }
    }

    fn destroyed(state: &mut H, _client: ClientId, resource: &ZwlrGammaControlV1, _data: &()) {
        if let Some((output, info)) = output_info_for_instance(state, resource) {
            info.gamma_control = None;
            if let Some(orig_gamma) = &info.orig_gamma {
                let orig_gamma = orig_gamma.clone();
                state.set_output_gamma(&output, &orig_gamma.0, &orig_gamma.1, &orig_gamma.2);
            }
        }
    }
}

fn output_info_for_instance<'a, H: WlrGammaControlHandler>(
    handler: &'a mut H,
    instance: &ZwlrGammaControlV1,
) -> Option<(Output, &'a mut OutputInfo)> {
    handler.wlr_gamma_control_state().outputs.iter_mut().find_map(|info| {
        info.gamma_control.clone().and_then(|gamma_control| {
            if gamma_control == *instance
                && let Some(output) = info.output.upgrade()
            {
                Some((output, info))
            } else {
                None
            }
        })
    })
}

fn read_gamma_ramps(fd: OwnedFd, gamma_length: usize) -> io::Result<(Vec<u16>, Vec<u16>, Vec<u16>)> {
    let expected_bytes = gamma_length * 3 * std::mem::size_of::<u16>();

    let mut buffer = vec![0u8; expected_bytes];
    std::fs::File::from(fd).read_exact(&mut buffer)?;

    let u16_data: Vec<u16> = buffer
        .chunks_exact(2)
        .map(|chunk| u16::from_ne_bytes([chunk[0], chunk[1]]))
        .collect();

    let red = u16_data[0..gamma_length].to_vec();
    let green = u16_data[gamma_length..gamma_length * 2].to_vec();
    let blue = u16_data[gamma_length * 2..gamma_length * 3].to_vec();

    Ok((red, green, blue))
}

macro_rules! delegate_wlr_gamma_control {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::gamma_control::v1::server::zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1: Box<dyn for<'c> Fn(&'c smithay::reexports::wayland_server::Client) -> bool + Send + Sync>
        ] => $crate::protocols::wlr_gamma_control::WlrGammaControlState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::gamma_control::v1::server::zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1: ()
        ] => $crate::protocols::wlr_gamma_control::WlrGammaControlState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::gamma_control::v1::server::zwlr_gamma_control_v1::ZwlrGammaControlV1: ()
        ] => $crate::protocols::wlr_gamma_control::WlrGammaControlState);
    };
}

pub(crate) use delegate_wlr_gamma_control;
