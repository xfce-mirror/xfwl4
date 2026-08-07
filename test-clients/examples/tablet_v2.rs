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

use std::{collections::HashMap, fmt::Debug, time::Duration};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_output, delegate_registry, delegate_seat, delegate_shm, delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    reexports::{
        client::{
            Connection, Dispatch, Proxy, QueueHandle, WEnum, event_created_child,
            protocol::{wl_output::WlOutput, wl_seat::WlSeat, wl_surface::WlSurface},
        },
        protocols::wp::tablet::zv2::client::{
            zwp_tablet_manager_v2::ZwpTabletManagerV2,
            zwp_tablet_pad_dial_v2::{self, ZwpTabletPadDialV2},
            zwp_tablet_pad_group_v2::{self, ZwpTabletPadGroupV2},
            zwp_tablet_pad_ring_v2::{self, ZwpTabletPadRingV2},
            zwp_tablet_pad_strip_v2::{self, ZwpTabletPadStripV2},
            zwp_tablet_pad_v2::{self, ZwpTabletPadV2},
            zwp_tablet_seat_v2::{self, ZwpTabletSeatV2},
            zwp_tablet_tool_v2::{self, ZwpTabletToolV2},
            zwp_tablet_v2::{self, ZwpTabletV2},
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{Capability, SeatHandler, SeatState},
    shell::{
        WaylandSurface,
        xdg::{
            XdgShell,
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use test_clients::wayland::{apply_window_configure, init_event_loop, paint_solid};

const DEFAULT_WIDTH: u32 = 640;
const DEFAULT_HEIGHT: u32 = 400;
const BACKGROUND: [u8; 4] = [0x40, 0x28, 0x18, 0xff];

struct State {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    shm: Shm,
    pool: SlotPool,
    tablet_manager: ZwpTabletManagerV2,
    tablet_seats: Vec<ZwpTabletSeatV2>,
    window: Window,
    buffer: Option<Buffer>,
    width: u32,
    height: u32,
    first_configure: bool,
    labels: HashMap<u32, String>,
}

impl State {
    fn draw(&mut self, qh: &QueueHandle<Self>) {
        paint_solid(
            &mut self.pool,
            &mut self.buffer,
            self.window.wl_surface(),
            qh,
            self.width,
            self.height,
            BACKGROUND,
        );
        self.window.commit();
    }

    fn set_label<P: Proxy>(&mut self, proxy: &P, label: String) {
        self.labels.insert(proxy.id().protocol_id(), label);
    }

    fn label<P: Proxy>(&self, proxy: &P) -> String {
        let id = proxy.id().protocol_id();
        self.labels.get(&id).cloned().unwrap_or_else(|| format!("<unknown> #{id}"))
    }

    fn forget<P: Proxy>(&mut self, proxy: &P) {
        self.labels.remove(&proxy.id().protocol_id());
    }
}

fn enum_name<T: Debug>(value: WEnum<T>) -> String {
    match value {
        WEnum::Value(value) => format!("{value:?}"),
        WEnum::Unknown(raw) => format!("unknown({raw:#x})"),
    }
}

fn surface_id(surface: &WlSurface) -> String {
    format!("surface #{}", surface.id().protocol_id())
}

fn main() {
    let (_conn, globals, qh, mut event_loop) = init_event_loop::<State>();

    let compositor = CompositorState::bind(&globals, &qh).unwrap();
    let shm = Shm::bind(&globals, &qh).unwrap();
    let xdg_shell = XdgShell::bind(&globals, &qh).unwrap();
    let tablet_manager = globals
        .bind::<ZwpTabletManagerV2, State, _>(&qh, 1..=2, ())
        .expect("compositor does not support zwp_tablet_manager_v2");

    let surface = compositor.create_surface(&qh);
    let window = xdg_shell.create_window(surface, WindowDecorations::RequestServer, &qh);
    window.set_title("wp-tablet-v2 monitor");
    window.set_app_id("org.xfce.xfwl4.tablet-v2-test");
    window.set_min_size(Some((200, 150)));
    window.commit();

    let pool = SlotPool::new((DEFAULT_WIDTH * DEFAULT_HEIGHT * 4) as usize, &shm).unwrap();
    let seat_state = SeatState::new(&globals, &qh);

    println!("Bound zwp_tablet_manager_v2 version {}", tablet_manager.version());

    let tablet_seats = seat_state
        .seats()
        .map(|seat| {
            println!("Requesting tablet seat for wl_seat #{}", seat.id().protocol_id());
            tablet_manager.get_tablet_seat(&seat, &qh, ())
        })
        .collect();

    println!("Move a tool over the window to see proximity, motion and axis events.");

    let mut state = State {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state,
        shm,
        pool,
        tablet_manager,
        tablet_seats,
        window,
        buffer: None,
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        first_configure: true,
        labels: HashMap::new(),
    };

    event_loop.run(Duration::from_millis(16), &mut state, |_state| {}).unwrap();
}

impl Dispatch<ZwpTabletManagerV2, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpTabletManagerV2,
        _event: <ZwpTabletManagerV2 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpTabletSeatV2, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &ZwpTabletSeatV2,
        event: zwp_tablet_seat_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use zwp_tablet_seat_v2::Event;
        match event {
            Event::TabletAdded { id } => {
                let label = format!("tablet #{}", id.id().protocol_id());
                println!("[+] {label} added");
                state.set_label(&id, label);
            }
            Event::ToolAdded { id } => {
                let label = format!("tool #{}", id.id().protocol_id());
                println!("[+] {label} added");
                state.set_label(&id, label);
            }
            Event::PadAdded { id } => {
                let label = format!("pad #{}", id.id().protocol_id());
                println!("[+] {label} added");
                state.set_label(&id, label);
            }
            _ => println!("[tablet seat] unknown event: {event:?}"),
        }
    }

    event_created_child!(State, ZwpTabletSeatV2, [
        zwp_tablet_seat_v2::EVT_TABLET_ADDED_OPCODE => (ZwpTabletV2, ()),
        zwp_tablet_seat_v2::EVT_TOOL_ADDED_OPCODE => (ZwpTabletToolV2, ()),
        zwp_tablet_seat_v2::EVT_PAD_ADDED_OPCODE => (ZwpTabletPadV2, ()),
    ]);
}

impl Dispatch<ZwpTabletV2, ()> for State {
    fn event(state: &mut Self, tablet: &ZwpTabletV2, event: zwp_tablet_v2::Event, _data: &(), _conn: &Connection, _qh: &QueueHandle<Self>) {
        use zwp_tablet_v2::Event;
        let label = state.label(tablet);
        match event {
            Event::Name { name } => {
                println!("[{label}] name: {name:?}");
                state.set_label(tablet, format!("{label} {name:?}"));
            }
            Event::Id { vid, pid } => println!("[{label}] id: vid={vid:#06x} pid={pid:#06x}"),
            Event::Path { path } => println!("[{label}] path: {path}"),
            Event::Bustype { bustype } => println!("[{label}] bustype: {}", enum_name(bustype)),
            Event::Done => println!("[{label}] done"),
            Event::Removed => {
                println!("[-] {label} removed");
                state.forget(tablet);
                tablet.destroy();
            }
            _ => println!("[{label}] unknown event: {event:?}"),
        }
    }
}

impl Dispatch<ZwpTabletToolV2, ()> for State {
    fn event(
        state: &mut Self,
        tool: &ZwpTabletToolV2,
        event: zwp_tablet_tool_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use zwp_tablet_tool_v2::Event;
        let label = state.label(tool);
        match event {
            Event::Type { tool_type } => {
                let type_name = enum_name(tool_type);
                println!("[{label}] type: {type_name}");
                state.set_label(tool, format!("{label} ({type_name})"));
            }
            Event::HardwareSerial {
                hardware_serial_hi,
                hardware_serial_lo,
            } => {
                let serial = (u64::from(hardware_serial_hi) << 32) | u64::from(hardware_serial_lo);
                println!("[{label}] hardware_serial: {serial:#018x}");
            }
            Event::HardwareIdWacom {
                hardware_id_hi,
                hardware_id_lo,
            } => {
                let id = (u64::from(hardware_id_hi) << 32) | u64::from(hardware_id_lo);
                println!("[{label}] hardware_id_wacom: {id:#018x}");
            }
            Event::Capability { capability } => println!("[{label}] capability: {}", enum_name(capability)),
            Event::Done => println!("[{label}] done"),
            Event::Removed => {
                println!("[-] {label} removed");
                state.forget(tool);
                tool.destroy();
            }
            Event::ProximityIn { serial, tablet, surface } => println!(
                "[{label}] proximity_in: serial={serial} {} {}",
                state.label(&tablet),
                surface_id(&surface)
            ),
            Event::ProximityOut => println!("[{label}] proximity_out"),
            Event::Down { serial } => println!("[{label}] down: serial={serial}"),
            Event::Up => println!("[{label}] up"),
            Event::Motion { x, y } => println!("[{label}] motion: x={x:.2} y={y:.2}"),
            Event::Pressure { pressure } => println!("[{label}] pressure: {pressure} ({:.1}%)", pressure as f64 * 100.0 / 65535.0),
            Event::Distance { distance } => println!("[{label}] distance: {distance} ({:.1}%)", distance as f64 * 100.0 / 65535.0),
            Event::Tilt { tilt_x, tilt_y } => println!("[{label}] tilt: x={tilt_x:.2}° y={tilt_y:.2}°"),
            Event::Rotation { degrees } => println!("[{label}] rotation: {degrees:.2}°"),
            Event::Slider { position } => println!("[{label}] slider: {position} ({:.1}%)", position as f64 * 100.0 / 65535.0),
            Event::Wheel { degrees, clicks } => println!("[{label}] wheel: {degrees:.2}° clicks={clicks}"),
            Event::Button {
                serial,
                button,
                state: pressed,
            } => println!("[{label}] button: serial={serial} button={button:#x} state={}", enum_name(pressed)),
            Event::Frame { time } => println!("[{label}] frame: time={time}"),
            _ => println!("[{label}] unknown event: {event:?}"),
        }
    }
}

impl Dispatch<ZwpTabletPadV2, ()> for State {
    fn event(
        state: &mut Self,
        pad: &ZwpTabletPadV2,
        event: zwp_tablet_pad_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use zwp_tablet_pad_v2::Event;
        let label = state.label(pad);
        match event {
            Event::Group { pad_group } => {
                let group_label = format!("{label} group #{}", pad_group.id().protocol_id());
                println!("[{label}] group: {group_label}");
                state.set_label(&pad_group, group_label);
            }
            Event::Path { path } => println!("[{label}] path: {path}"),
            Event::Buttons { buttons } => println!("[{label}] buttons: {buttons}"),
            Event::Done => println!("[{label}] done"),
            Event::Button {
                time,
                button,
                state: pressed,
            } => println!("[{label}] button: time={time} button={button} state={}", enum_name(pressed)),
            Event::Enter { serial, tablet, surface } => {
                println!("[{label}] enter: serial={serial} {} {}", state.label(&tablet), surface_id(&surface))
            }
            Event::Leave { serial, surface } => println!("[{label}] leave: serial={serial} {}", surface_id(&surface)),
            Event::Removed => {
                println!("[-] {label} removed");
                state.forget(pad);
                pad.destroy();
            }
            _ => println!("[{label}] unknown event: {event:?}"),
        }
    }

    event_created_child!(State, ZwpTabletPadV2, [
        zwp_tablet_pad_v2::EVT_GROUP_OPCODE => (ZwpTabletPadGroupV2, ()),
    ]);
}

impl Dispatch<ZwpTabletPadGroupV2, ()> for State {
    fn event(
        state: &mut Self,
        group: &ZwpTabletPadGroupV2,
        event: zwp_tablet_pad_group_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use zwp_tablet_pad_group_v2::Event;
        let label = state.label(group);
        match event {
            Event::Buttons { buttons } => {
                let indices = buttons
                    .chunks_exact(4)
                    .map(|bytes| u32::from_ne_bytes(bytes.try_into().unwrap()))
                    .collect::<Vec<_>>();
                println!("[{label}] buttons: {indices:?}");
            }
            Event::Ring { ring } => {
                let ring_label = format!("{label} ring #{}", ring.id().protocol_id());
                println!("[{label}] ring: {ring_label}");
                state.set_label(&ring, ring_label);
            }
            Event::Strip { strip } => {
                let strip_label = format!("{label} strip #{}", strip.id().protocol_id());
                println!("[{label}] strip: {strip_label}");
                state.set_label(&strip, strip_label);
            }
            Event::Dial { dial } => {
                let dial_label = format!("{label} dial #{}", dial.id().protocol_id());
                println!("[{label}] dial: {dial_label}");
                state.set_label(&dial, dial_label);
            }
            Event::Modes { modes } => println!("[{label}] modes: {modes}"),
            Event::Done => println!("[{label}] done"),
            Event::ModeSwitch { time, serial, mode } => println!("[{label}] mode_switch: time={time} serial={serial} mode={mode}"),
            _ => println!("[{label}] unknown event: {event:?}"),
        }
    }

    event_created_child!(State, ZwpTabletPadGroupV2, [
        zwp_tablet_pad_group_v2::EVT_RING_OPCODE => (ZwpTabletPadRingV2, ()),
        zwp_tablet_pad_group_v2::EVT_STRIP_OPCODE => (ZwpTabletPadStripV2, ()),
        zwp_tablet_pad_group_v2::EVT_DIAL_OPCODE => (ZwpTabletPadDialV2, ()),
    ]);
}

impl Dispatch<ZwpTabletPadRingV2, ()> for State {
    fn event(
        state: &mut Self,
        ring: &ZwpTabletPadRingV2,
        event: zwp_tablet_pad_ring_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use zwp_tablet_pad_ring_v2::Event;
        let label = state.label(ring);
        match event {
            Event::Source { source } => println!("[{label}] source: {}", enum_name(source)),
            Event::Angle { degrees } => println!("[{label}] angle: {degrees:.2}°"),
            Event::Stop => println!("[{label}] stop"),
            Event::Frame { time } => println!("[{label}] frame: time={time}"),
            _ => println!("[{label}] unknown event: {event:?}"),
        }
    }
}

impl Dispatch<ZwpTabletPadStripV2, ()> for State {
    fn event(
        state: &mut Self,
        strip: &ZwpTabletPadStripV2,
        event: zwp_tablet_pad_strip_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use zwp_tablet_pad_strip_v2::Event;
        let label = state.label(strip);
        match event {
            Event::Source { source } => println!("[{label}] source: {}", enum_name(source)),
            Event::Position { position } => println!("[{label}] position: {position} ({:.1}%)", position as f64 * 100.0 / 65535.0),
            Event::Stop => println!("[{label}] stop"),
            Event::Frame { time } => println!("[{label}] frame: time={time}"),
            _ => println!("[{label}] unknown event: {event:?}"),
        }
    }
}

impl Dispatch<ZwpTabletPadDialV2, ()> for State {
    fn event(
        state: &mut Self,
        dial: &ZwpTabletPadDialV2,
        event: zwp_tablet_pad_dial_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use zwp_tablet_pad_dial_v2::Event;
        let label = state.label(dial);
        match event {
            Event::Delta { value120 } => println!("[{label}] delta: {value120} ({:.2} turns)", value120 as f64 / 120.0),
            Event::Frame { time } => println!("[{label}] frame: time={time}"),
            _ => println!("[{label}] unknown event: {event:?}"),
        }
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

impl CompositorHandler for State {
    fn frame(&mut self, _conn: &Connection, qh: &QueueHandle<Self>, _surface: &WlSurface, _time: u32) {
        self.draw(qh);
    }

    fn surface_enter(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _surface: &WlSurface, _output: &WlOutput) {}

    fn surface_leave(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _surface: &WlSurface, _output: &WlOutput) {}

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _new_transform: smithay_client_toolkit::reexports::client::protocol::wl_output::Transform,
    ) {
    }

    fn scale_factor_changed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _surface: &WlSurface, _new_factor: i32) {}
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}

    fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}

    fn output_destroyed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, qh: &QueueHandle<Self>, seat: WlSeat) {
        println!("Requesting tablet seat for wl_seat #{}", seat.id().protocol_id());
        let tablet_seat = self.tablet_manager.get_tablet_seat(&seat, qh, ());
        self.tablet_seats.push(tablet_seat);
    }

    fn new_capability(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat, _capability: Capability) {}

    fn remove_capability(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat, _capability: Capability) {}

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {}
}

impl WindowHandler for State {
    fn configure(&mut self, _conn: &Connection, qh: &QueueHandle<Self>, _window: &Window, configure: WindowConfigure, _serial: u32) {
        let (new_w, new_h, redraw) = apply_window_configure(
            &configure,
            self.first_configure,
            (self.width, self.height),
            (DEFAULT_WIDTH, DEFAULT_HEIGHT),
        );
        if redraw {
            self.first_configure = false;
            self.buffer = None;
            self.width = new_w;
            self.height = new_h;
            self.draw(qh);
        } else {
            self.window.commit();
        }
    }

    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _window: &Window) {
        std::process::exit(0);
    }
}

delegate_registry!(State);
delegate_compositor!(State);
delegate_output!(State);
delegate_shm!(State);
delegate_seat!(State);
delegate_xdg_shell!(State);
delegate_xdg_window!(State);
