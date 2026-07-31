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
    input::{Seat, SeatHandler, WeakSeat},
    output::{Output, WeakOutput},
    reexports::wayland_server::{
        Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
        backend::{ClientId, GlobalId},
        protocol::wl_seat::WlSeat,
    },
    utils::{Logical, Rectangle, SealedFile, Size},
    wayland::{Dispatch2, GlobalDispatch2},
};

use crate::protocols::{
    ClientFilter, GlobalData,
    xfce_output::proto::{xfce_output_manager_private_v1::XfceOutputManagerPrivateV1, xfce_output_v1::XfceOutputV1},
};

const PROTO_VERSION: u32 = 1;

pub struct XfceOutputState<D: SeatHandler> {
    _global: GlobalId,
    manager_instances: Vec<XfceOutputManagerPrivateV1>,
    outputs: Vec<XfceOutput>,
    pointer_outputs: Vec<(WeakSeat<D>, WeakOutput)>,
}

pub trait XfceOutputHandler: SeatHandler + Sized {
    fn xfce_output_state(&mut self) -> &mut XfceOutputState<Self>;
}

struct XfceOutput {
    output: WeakOutput,
    instances: Vec<XfceOutputV1>,
    edid: Bytes,
    is_primary: bool,
    workarea: Rectangle<i32, Logical>,
}

#[derive(Default)]
pub struct XfceOutputChangedInput {
    pub is_primary: Option<bool>,
    pub workarea: Option<Rectangle<i32, Logical>>,
}

impl<D: SeatHandler> XfceOutputState<D> {
    pub fn new<H, F>(dh: &DisplayHandle, filter: F) -> Self
    where
        H: XfceOutputHandler + GlobalDispatch<XfceOutputManagerPrivateV1, ClientFilter> + 'static,
        D: SeatHandler,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let _global = dh.create_global::<H, XfceOutputManagerPrivateV1, _>(PROTO_VERSION, Box::new(filter));
        Self {
            _global,
            manager_instances: Vec::new(),
            outputs: Vec::new(),
            pointer_outputs: Vec::new(),
        }
    }

    pub fn output_created(&mut self, output: &Output, output_size: Size<i32, Logical>, edid: Bytes, is_primary: bool) {
        let xfce_output = XfceOutput {
            output: output.downgrade(),
            instances: Vec::new(),
            edid,
            is_primary,
            workarea: Rectangle::new((0, 0).into(), output_size),
        };
        self.outputs.push(xfce_output);
    }

    pub fn output_changed(&mut self, output: &Output, input: XfceOutputChangedInput) {
        if let Some(xfce_output) = self.outputs.iter_mut().find(|o| &o.output == output) {
            let changed_is_primary = input.is_primary.filter(|is_primary| *is_primary != xfce_output.is_primary);
            let changed_workarea = input.workarea.filter(|workarea| *workarea != xfce_output.workarea);

            let send_primary = changed_is_primary.is_some_and(|is_primary| is_primary);
            let something_sent = send_primary || changed_workarea.is_some();

            for instance in &xfce_output.instances {
                if send_primary {
                    instance.primary();
                }

                if let Some(workarea) = &changed_workarea {
                    instance.workarea(workarea.loc.x, workarea.loc.y, workarea.size.w as u32, workarea.size.h as u32);
                }

                if something_sent {
                    instance.done();
                }
            }

            if let Some(changed_is_primary) = changed_is_primary {
                xfce_output.is_primary = changed_is_primary;
            }
            if let Some(changed_workarea) = changed_workarea {
                xfce_output.workarea = changed_workarea;
            }
        }
    }

    pub fn pointer_output_changed_for_seat(&mut self, seat: &Seat<D>, output: Option<&Output>) {
        let cur_output = self
            .pointer_outputs
            .iter()
            .find_map(|(a_seat, cur_output)| (a_seat.upgrade().as_ref() == Some(seat)).then_some(cur_output));

        let changed = match (cur_output, output) {
            (Some(cur), Some(new)) => cur != new,
            (None, None) => false,
            _ => true,
        };

        if changed {
            if let Some(cur_output) = cur_output
                && let Some(xfce_output) = self.outputs.iter().find(|xfce_output| xfce_output.output == *cur_output)
            {
                send_pointer_event(xfce_output, seat, |instance, seat_instance| instance.pointer_leave(seat_instance));
            }

            self.pointer_outputs
                .retain(|(a_seat, _)| a_seat.upgrade().is_some_and(|a_seat| a_seat != *seat));

            if let Some(output) = output {
                if let Some(xfce_output) = self.outputs.iter().find(|xfce_output| xfce_output.output == *output) {
                    send_pointer_event(xfce_output, seat, |instance, seat_instance| instance.pointer_enter(seat_instance));
                }

                self.pointer_outputs.push((seat.downgrade(), output.downgrade()));
            }
        }
    }

    pub fn output_destroyed(&mut self, output: &Output) {
        if let Some(pos) = self.outputs.iter().position(|o| &o.output == output) {
            let xfce_output = self.outputs.remove(pos);

            for seat in self
                .pointer_outputs
                .iter()
                .filter_map(|(seat, o)| (o == output).then(|| seat.upgrade()).flatten())
            {
                send_pointer_event(&xfce_output, &seat, |instance, seat_instance| instance.pointer_leave(seat_instance));
            }
        }

        self.pointer_outputs.retain(|(seat, o)| o != output && seat.is_alive());
    }
}

impl<D> GlobalDispatch2<XfceOutputManagerPrivateV1, D> for ClientFilter
where
    D: XfceOutputHandler + Dispatch<XfceOutputManagerPrivateV1, GlobalData> + Dispatch<XfceOutputV1, GlobalData>,
{
    fn bind(
        &self,
        state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<XfceOutputManagerPrivateV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        let instance = data_init.init(resource, GlobalData);
        state.xfce_output_state().manager_instances.push(instance);
    }

    fn can_view(&self, client: &Client) -> bool {
        self(client)
    }
}

impl<D> Dispatch2<XfceOutputManagerPrivateV1, D> for GlobalData
where
    D: XfceOutputHandler + Dispatch<XfceOutputV1, GlobalData>,
{
    fn request(
        &self,
        state: &mut D,
        client: &Client,
        resource: &XfceOutputManagerPrivateV1,
        request: <XfceOutputManagerPrivateV1 as smithay::reexports::wayland_server::Resource>::Request,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use proto::xfce_output_manager_private_v1::Request;

        match request {
            Request::GetXfceOutput { id, output } => {
                let instance = data_init.init(id, GlobalData);

                let state = state.xfce_output_state();
                if let Some((xfce_output, output)) = state.outputs.iter_mut().find_map(|xfce_output| {
                    xfce_output
                        .output
                        .upgrade()
                        .and_then(|o| o.owns(&output).then_some((xfce_output, o)))
                }) {
                    let serial = output.physical_properties().serial_number;
                    if !serial.is_empty() {
                        instance.serial(serial);
                    }

                    if !xfce_output.edid.is_empty() {
                        match SealedFile::with_data(c"edid", &xfce_output.edid) {
                            Err(err) => tracing::warn!("Failed to make FD for EDID: {err}"),
                            Ok(fd) => instance.edid(fd.as_fd(), xfce_output.edid.len() as u32),
                        }
                    }

                    if xfce_output.is_primary {
                        instance.primary();
                    }

                    instance.workarea(
                        xfce_output.workarea.loc.x,
                        xfce_output.workarea.loc.y,
                        xfce_output.workarea.size.w as u32,
                        xfce_output.workarea.size.h as u32,
                    );

                    instance.done();

                    for seat in state
                        .pointer_outputs
                        .iter()
                        .filter_map(|(seat, output)| (*output == xfce_output.output).then_some(seat))
                    {
                        if let Some(seat) = seat.upgrade() {
                            for seat_instance in seat.client_seats(client) {
                                instance.pointer_enter(&seat_instance);
                            }
                        }
                    }

                    xfce_output.instances.push(instance);
                }
            }
            Request::Release => self.destroyed(state, client.id(), resource),
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &XfceOutputManagerPrivateV1) {
        state.xfce_output_state().manager_instances.retain(|instance| instance != resource);
    }
}

impl<D> Dispatch2<XfceOutputV1, D> for GlobalData
where
    D: XfceOutputHandler,
{
    fn request(
        &self,
        state: &mut D,
        client: &Client,
        resource: &XfceOutputV1,
        request: <XfceOutputV1 as smithay::reexports::wayland_server::Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        use proto::xfce_output_v1::Request;

        match request {
            Request::Destroy => self.destroyed(state, client.id(), resource),
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &XfceOutputV1) {
        for xfce_output in &mut state.xfce_output_state().outputs {
            xfce_output.instances.retain(|instance| instance != resource);
        }
    }
}

fn send_pointer_event<D: SeatHandler>(xfce_output: &XfceOutput, seat: &Seat<D>, send: impl Fn(&XfceOutputV1, &WlSeat)) {
    for instance in &xfce_output.instances {
        if let Some(client) = instance.client() {
            for seat_instance in seat.client_seats(&client) {
                send(instance, &seat_instance);
            }
        }
    }
}

pub mod proto {
    use smithay::reexports::wayland_server::{
        self,
        protocol::{wl_output, wl_seat},
    };

    pub mod __interfaces {
        use smithay::reexports::wayland_server::{
            backend as wayland_backend,
            protocol::__interfaces::{WL_OUTPUT_INTERFACE, WL_SEAT_INTERFACE, wl_output_interface, wl_seat_interface},
        };

        wayland_scanner::generate_interfaces!("./resources/xfce-wayland-protocols/xfce-output-private-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("./resources/xfce-wayland-protocols/xfce-output-private-v1.xml");
}
