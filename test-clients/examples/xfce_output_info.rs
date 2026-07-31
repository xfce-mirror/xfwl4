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
    collections::HashMap,
    fs::File,
    io::Read,
    os::fd::{FromRawFd, IntoRawFd, OwnedFd},
};

use smithay_client_toolkit::reexports::{
    client::{
        Connection, Dispatch, Proxy, QueueHandle,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{wl_output, wl_registry, wl_seat},
    },
    protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1},
};

mod proto {
    use smithay_client_toolkit::reexports::client::{
        self as wayland_client,
        protocol::{wl_output, wl_seat},
    };

    pub mod __interfaces {
        use smithay_client_toolkit::reexports::client::{
            backend as wayland_backend,
            protocol::__interfaces::{WL_OUTPUT_INTERFACE, WL_SEAT_INTERFACE, wl_output_interface, wl_seat_interface},
        };

        wayland_scanner::generate_interfaces!("../resources/xfce-wayland-protocols/xfce-output-private-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("../resources/xfce-wayland-protocols/xfce-output-private-v1.xml");
}

use proto::{
    xfce_output_manager_private_v1::XfceOutputManagerPrivateV1,
    xfce_output_v1::{self, XfceOutputV1},
};

const WL_OUTPUT_VERSION: u32 = 4;
const XDG_OUTPUT_MANAGER_VERSION: u32 = 3;
const XFCE_OUTPUT_MANAGER_VERSION: u32 = 1;

// Registry global name, used to tie an output's wl_output, zxdg_output_v1, and xfce_output_v1
// together so events on any of them can be attributed to the same output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct OutputId(u32);

struct Output {
    wl_output: wl_output::WlOutput,
    xdg_output: Option<zxdg_output_v1::ZxdgOutputV1>,
    xfce_output: Option<XfceOutputV1>,
    name: Option<String>,
    xfce_initial_done: bool,
}

struct Seat {
    wl_seat: wl_seat::WlSeat,
    name: Option<String>,
}

struct State {
    xdg_output_manager: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    xfce_output_manager: Option<XfceOutputManagerPrivateV1>,
    outputs: HashMap<OutputId, Output>,
    seats: HashMap<u32, Seat>,
}

impl State {
    fn label(&self, id: OutputId) -> String {
        self.outputs
            .get(&id)
            .and_then(|output| output.name.clone())
            .unwrap_or_else(|| format!("output@{}", id.0))
    }

    fn seat_label(&self, wl_seat: &wl_seat::WlSeat) -> String {
        self.seats
            .values()
            .find(|seat| &seat.wl_seat == wl_seat)
            .and_then(|seat| seat.name.clone())
            .unwrap_or_else(|| format!("seat@{}", wl_seat.id().protocol_id()))
    }

    // The compositor delivers pointer_enter/pointer_leave only to clients that have bound the
    // wl_seat in question, so seats are bound before any xfce_output is requested.
    fn add_output(&mut self, id: OutputId, wl_output: wl_output::WlOutput, qh: &QueueHandle<Self>) {
        let xdg_output = self
            .xdg_output_manager
            .as_ref()
            .map(|manager| manager.get_xdg_output(&wl_output, qh, id));
        let xfce_output = self
            .xfce_output_manager
            .as_ref()
            .map(|manager| manager.get_xfce_output(&wl_output, qh, id));

        if xdg_output.is_none() {
            println!("[{}] no zxdg_output_manager_v1; skipping xdg_output", self.label(id));
        }
        if xfce_output.is_none() {
            println!("[{}] no xfce_output_manager_private_v1; skipping xfce_output", self.label(id));
        }

        self.outputs.insert(
            id,
            Output {
                wl_output,
                xdg_output,
                xfce_output,
                name: None,
                xfce_initial_done: false,
            },
        );
    }

    fn remove_output(&mut self, id: OutputId) {
        if let Some(output) = self.outputs.remove(&id) {
            println!("[-] [{}] output removed", output.name.unwrap_or_else(|| format!("output@{}", id.0)));

            if let Some(xfce_output) = output.xfce_output {
                xfce_output.destroy();
            }
            if let Some(xdg_output) = output.xdg_output {
                xdg_output.destroy();
            }
            if output.wl_output.version() >= 3 {
                output.wl_output.release();
            }
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global { name, interface, version } if interface == wl_seat::WlSeat::interface().name => {
                let wl_seat = registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(wl_seat::WlSeat::interface().version), qh, name);
                println!("[+] seat@{name} appeared");
                state.seats.insert(name, Seat { wl_seat, name: None });
            }
            wl_registry::Event::Global { name, interface, version } if interface == wl_output::WlOutput::interface().name => {
                let id = OutputId(name);
                let wl_output = registry.bind::<wl_output::WlOutput, _, _>(name, version.min(WL_OUTPUT_VERSION), qh, id);
                println!("[+] output@{name} appeared");
                state.add_output(id, wl_output, qh);
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(seat) = state.seats.remove(&name) {
                    println!("[-] [{}] seat removed", seat.name.unwrap_or_else(|| format!("seat@{name}")));
                    if seat.wl_seat.version() >= 5 {
                        seat.wl_seat.release();
                    }
                } else {
                    state.remove_output(OutputId(name));
                }
            }
            _ => (),
        }
    }
}

impl Dispatch<wl_seat::WlSeat, u32> for State {
    fn event(state: &mut Self, _proxy: &wl_seat::WlSeat, event: wl_seat::Event, name: &u32, _conn: &Connection, _qh: &QueueHandle<Self>) {
        match event {
            wl_seat::Event::Name { name: seat_name } => {
                println!("[seat@{name}] name: {seat_name}");
                if let Some(seat) = state.seats.get_mut(name) {
                    seat.name = Some(seat_name);
                }
            }
            wl_seat::Event::Capabilities { capabilities } => {
                println!("[seat@{name}] capabilities: {capabilities:?}");
            }
            _ => (),
        }
    }
}

impl Dispatch<wl_output::WlOutput, OutputId> for State {
    fn event(
        state: &mut Self,
        _proxy: &wl_output::WlOutput,
        event: wl_output::Event,
        id: &OutputId,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // wl_output.name arrives before the first wl_output.done, so adopt it before labelling.
        if let wl_output::Event::Name { name } = &event
            && let Some(output) = state.outputs.get_mut(id)
        {
            output.name = Some(name.clone());
        }

        let label = state.label(*id);
        match event {
            wl_output::Event::Geometry {
                x,
                y,
                physical_width,
                physical_height,
                subpixel,
                make,
                model,
                transform,
            } => {
                println!(
                    "[{label}] wl_output.geometry: position=({x},{y}) physical={physical_width}x{physical_height}mm \
                     subpixel={subpixel:?} make={make:?} model={model:?} transform={transform:?}"
                );
            }
            wl_output::Event::Mode {
                flags,
                width,
                height,
                refresh,
            } => {
                println!(
                    "[{label}] wl_output.mode: {width}x{height}@{:.3}Hz flags={flags:?}",
                    refresh as f64 / 1000.0
                );
            }
            wl_output::Event::Scale { factor } => {
                println!("[{label}] wl_output.scale: {factor}");
            }
            wl_output::Event::Name { name } => {
                println!("[{label}] wl_output.name: {name}");
            }
            wl_output::Event::Description { description } => {
                println!("[{label}] wl_output.description: {description}");
            }
            wl_output::Event::Done => {
                println!("[{label}] wl_output.done");
            }
            _ => (),
        }
    }
}

impl Dispatch<zxdg_output_manager_v1::ZxdgOutputManagerV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &zxdg_output_manager_v1::ZxdgOutputManagerV1,
        _event: zxdg_output_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, OutputId> for State {
    fn event(
        state: &mut Self,
        _proxy: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        id: &OutputId,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let label = state.label(*id);
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => {
                println!("[{label}] xdg_output.logical_position: ({x},{y})");
            }
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                println!("[{label}] xdg_output.logical_size: {width}x{height}");
            }
            zxdg_output_v1::Event::Name { name } => {
                println!("[{label}] xdg_output.name: {name}");
            }
            zxdg_output_v1::Event::Description { description } => {
                println!("[{label}] xdg_output.description: {description}");
            }
            zxdg_output_v1::Event::Done => {
                println!("[{label}] xdg_output.done (deprecated since v3)");
            }
            _ => (),
        }
    }
}

impl Dispatch<XfceOutputManagerPrivateV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &XfceOutputManagerPrivateV1,
        _event: proto::xfce_output_manager_private_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XfceOutputV1, OutputId> for State {
    fn event(
        state: &mut Self,
        _proxy: &XfceOutputV1,
        event: xfce_output_v1::Event,
        id: &OutputId,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let label = state.label(*id);
        match event {
            xfce_output_v1::Event::Serial { serial } => {
                println!("[{label}] xfce_output.serial: {serial:?}");
            }
            xfce_output_v1::Event::Edid { fd, size } => {
                println!("[{label}] xfce_output.edid: {size} bytes; {}", describe_edid(fd, size));
            }
            xfce_output_v1::Event::Primary => {
                println!("[{label}] xfce_output.primary");
            }
            xfce_output_v1::Event::Workarea { x, y, width, height } => {
                println!("[{label}] xfce_output.workarea: ({x},{y}) {width}x{height}");
            }
            // Terminates a batch of serial/edid/primary/workarea; pointer_enter and pointer_leave
            // are explicitly outside the grouping and can arrive between batches.
            xfce_output_v1::Event::Done => {
                let initial = state
                    .outputs
                    .get_mut(id)
                    .is_some_and(|output| !std::mem::replace(&mut output.xfce_initial_done, true));
                if initial {
                    println!("[{label}] xfce_output.done (initial properties complete)");
                } else {
                    println!("[{label}] xfce_output.done (change batch complete)");
                }
            }
            xfce_output_v1::Event::PointerEnter { seat } => {
                println!("[{label}] xfce_output.pointer_enter: {}", state.seat_label(&seat));
            }
            xfce_output_v1::Event::PointerLeave { seat } => {
                println!("[{label}] xfce_output.pointer_leave: {}", state.seat_label(&seat));
            }
        }
    }
}

const EDID_MAGIC: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];

fn describe_edid(fd: OwnedFd, size: u32) -> String {
    let mut edid = Vec::with_capacity(size as usize);
    let mut file = unsafe { File::from_raw_fd(fd.into_raw_fd()) };

    if let Err(err) = file.read_to_end(&mut edid) {
        format!("<failed to read: {err}>")
    } else if edid.len() < 18 {
        format!("<short read: {} bytes>", edid.len())
    } else if edid[..8] != EDID_MAGIC {
        "<bad header magic>".to_owned()
    } else {
        // Manufacturer ID is three 5-bit letters packed big-endian into bytes 8..10.
        let packed = u16::from_be_bytes([edid[8], edid[9]]);
        let letter = |shift: u32| match ((packed >> shift) & 0x1f) as u8 {
            0 => '?',
            value => (b'A' + value - 1) as char,
        };
        let manufacturer: String = [letter(10), letter(5), letter(0)].into_iter().collect();
        let product = u16::from_le_bytes([edid[10], edid[11]]);
        let serial = u32::from_le_bytes([edid[12], edid[13], edid[14], edid[15]]);

        format!("manufacturer={manufacturer} product=0x{product:04x} serial=0x{serial:08x}")
    }
}

fn main() {
    let conn = Connection::connect_to_env().expect("Failed to connect to Wayland");
    let (globals, mut queue) = registry_queue_init::<State>(&conn).expect("Failed to init registry");
    let qh = queue.handle();

    let mut state = State {
        xdg_output_manager: None,
        xfce_output_manager: None,
        outputs: HashMap::new(),
        seats: HashMap::new(),
    };

    // Bind seats first: the compositor sends pointer_enter only to clients that already have the
    // seat bound, so binding after get_xfce_output would drop the initial enter for that output.
    for global in globals
        .contents()
        .clone_list()
        .iter()
        .filter(|global| global.interface == wl_seat::WlSeat::interface().name)
    {
        let version = global.version.min(wl_seat::WlSeat::interface().version);
        let wl_seat = globals
            .registry()
            .bind::<wl_seat::WlSeat, _, _>(global.name, version, &qh, global.name);
        state.seats.insert(global.name, Seat { wl_seat, name: None });
    }
    println!("Bound {} seat(s)", state.seats.len());

    state.xdg_output_manager = globals.bind(&qh, 1..=XDG_OUTPUT_MANAGER_VERSION, ()).ok();
    if state.xdg_output_manager.is_none() {
        println!("Compositor does not support zxdg_output_manager_v1");
    }

    state.xfce_output_manager = globals.bind(&qh, 1..=XFCE_OUTPUT_MANAGER_VERSION, ()).ok();
    if state.xfce_output_manager.is_none() {
        println!("Compositor does not support xfce_output_manager_private_v1");
    }

    for global in globals
        .contents()
        .clone_list()
        .iter()
        .filter(|global| global.interface == wl_output::WlOutput::interface().name)
    {
        let id = OutputId(global.name);
        let version = global.version.min(WL_OUTPUT_VERSION);
        let wl_output = globals.registry().bind::<wl_output::WlOutput, _, _>(global.name, version, &qh, id);
        state.add_output(id, wl_output, &qh);
    }
    println!("Bound {} output(s); watching for changes...", state.outputs.len());

    loop {
        queue.blocking_dispatch(&mut state).expect("Dispatch failed");
    }
}
