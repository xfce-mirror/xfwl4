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

use std::collections::HashMap;

use smithay_client_toolkit::reexports::client::{
    Connection, Dispatch, Proxy, QueueHandle, event_created_child,
    globals::{GlobalListContents, registry_queue_init},
    protocol::wl_registry,
};

mod proto {
    use smithay_client_toolkit::reexports::client as wayland_client;

    pub mod __interfaces {
        use smithay_client_toolkit::reexports::client::backend as wayland_backend;
        wayland_scanner::generate_interfaces!("../resources/xfce-wayland-protocols/xfce-input-device-list-private-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("../resources/xfce-wayland-protocols/xfce-input-device-list-private-v1.xml");
}

use proto::{
    xfce_input_device_list_private_v1::{self, XfceInputDeviceListPrivateV1},
    xfce_input_device_v1::{self, XfceInputDeviceV1},
};

struct State {
    devices: HashMap<u32, String>,
    initial_done: bool,
}

impl State {
    fn label(&self, device: &XfceInputDeviceV1) -> String {
        let id = device.id().protocol_id();
        self.devices
            .get(&id)
            .map(|name| format!("{name} #{id}"))
            .unwrap_or_else(|| format!("<unknown> #{id}"))
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XfceInputDeviceListPrivateV1, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &XfceInputDeviceListPrivateV1,
        event: xfce_input_device_list_private_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use xfce_input_device_list_private_v1::Event;
        match event {
            Event::Device { id, name, capabilities } => {
                let pid = id.id().protocol_id();
                println!("[+] device #{pid} added: name={name:?} capabilities={capabilities:?}");
                state.devices.insert(pid, name);
            }
            Event::Done => {
                if !state.initial_done {
                    println!("--- initial enumeration complete; watching for changes ---");
                    state.initial_done = true;
                } else {
                    println!("--- hotplug batch complete ---");
                }
            }
        }
    }

    event_created_child!(State, XfceInputDeviceListPrivateV1, [
        xfce_input_device_list_private_v1::EVT_DEVICE_OPCODE => (XfceInputDeviceV1, ()),
    ]);
}

impl Dispatch<XfceInputDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        device: &XfceInputDeviceV1,
        event: xfce_input_device_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use xfce_input_device_v1::Event;
        let label = state.label(device);
        match event {
            Event::Removed => {
                println!("[-] [{label}] removed");
                state.devices.remove(&device.id().protocol_id());
                device.release();
            }
            Event::SendEvents {
                supported,
                default_value,
                current_value,
            } => {
                println!("[{label}] send_events: supported={supported:?} default_value={default_value:?} current_value={current_value:?}");
            }
            Event::AccelSpeed {
                default_value,
                current_value,
            } => {
                println!("[{label}] accel_speed: default_value={default_value} current_value={current_value}");
            }
            Event::AccelProfile {
                supported,
                default_value,
                current_value,
            } => {
                println!(
                    "[{label}] accel_profile: supported={supported:?} default_value={default_value:?} current_value={current_value:?}"
                );
            }
            Event::NaturalScroll {
                default_value,
                current_value,
            } => {
                println!("[{label}] natural_scroll: default_value={default_value:?} current_value={current_value:?}");
            }
            Event::ScrollMethod {
                supported,
                default_value,
                current_value,
            } => {
                println!(
                    "[{label}] scroll_method: supported={supported:?} default_value={default_value:?} current_value={current_value:?}"
                );
            }
            Event::ScrollButton {
                default_value,
                current_value,
            } => {
                println!("[{label}] scroll_button: default_value={default_value} current_value={current_value}");
            }
            Event::ScrollButtonLock {
                default_value,
                current_value,
            } => {
                println!("[{label}] scroll_button_lock: default_value={default_value:?} current_value={current_value:?}");
            }
            Event::ClickMethod {
                supported,
                default_value,
                current_value,
            } => {
                println!("[{label}] click_method: supported={supported:?} default_value={default_value:?} current_value={current_value:?}");
            }
            Event::ClickfingerButtonMap {
                default_value,
                current_value,
            } => {
                println!("[{label}] clickfinger_button_map: default_value={default_value:?} current_value={current_value:?}");
            }
            Event::LeftHanded {
                default_value,
                current_value,
            } => {
                println!("[{label}] left_handed: default_value={default_value:?} current_value={current_value:?}");
            }
            Event::MiddleEmulation {
                default_value,
                current_value,
            } => {
                println!("[{label}] middle_emulation: default_value={default_value:?} current_value={current_value:?}");
            }
            Event::Tap {
                default_value,
                current_value,
            } => {
                println!("[{label}] tap: default_value={default_value:?} current_value={current_value:?}");
            }
            Event::TapFingerCount { count } => {
                println!("[{label}] tap_finger_count: {count}");
            }
            Event::TapButtonMap {
                default_value,
                current_value,
            } => {
                println!("[{label}] tap_button_map: default_value={default_value:?} current_value={current_value:?}");
            }
            Event::TapDrag {
                default_value,
                current_value,
            } => {
                println!("[{label}] tap_drag: default_value={default_value:?} current_value={current_value:?}");
            }
            Event::TapDragLock {
                default_value,
                current_value,
            } => {
                println!("[{label}] tap_drag_lock: default_value={default_value:?} current_value={current_value:?}");
            }
            Event::ThreeFingerDrag {
                max_fingers,
                default_value,
                current_value,
            } => {
                println!(
                    "[{label}] three_finger_drag: max_fingers={max_fingers} default_value={default_value:?} current_value={current_value:?}"
                );
            }
            Event::Dwt {
                default_value,
                current_value,
            } => {
                println!("[{label}] dwt: default_value={default_value:?} current_value={current_value:?}");
            }
            Event::DwtTimeout {
                default_value,
                current_value,
            } => {
                println!("[{label}] dwt_timeout: default_value={default_value}ms current_value={current_value}ms");
            }
            Event::Dwtp {
                default_value,
                current_value,
            } => {
                println!("[{label}] dwtp: default_value={default_value:?} current_value={current_value:?}");
            }
            Event::DwtpTimeout {
                default_value,
                current_value,
            } => {
                println!("[{label}] dwtp_timeout: default_value={default_value}ms current_value={current_value}ms");
            }
            Event::Rotation {
                default_value,
                current_value,
            } => {
                println!("[{label}] rotation: default_value={default_value}° current_value={current_value}°");
            }
            Event::Calibration {
                default_a,
                default_b,
                default_c,
                default_d,
                default_e,
                default_f,
                current_a,
                current_b,
                current_c,
                current_d,
                current_e,
                current_f,
            } => {
                println!(
                    "[{label}] calibration: default_value=[{default_a}, {default_b}, {default_c}, {default_d}, {default_e}, {default_f}] \
                     current_value=[{current_a}, {current_b}, {current_c}, {current_d}, {current_e}, {current_f}]"
                );
            }
            Event::Area {
                default_x1,
                default_y1,
                default_x2,
                default_y2,
                current_x1,
                current_y1,
                current_x2,
                current_y2,
            } => {
                println!(
                    "[{label}] area: default_value=({default_x1},{default_y1})..({default_x2},{default_y2}) \
                     current_value=({current_x1},{current_y1})..({current_x2},{current_y2})"
                );
            }
        }
    }
}

fn main() {
    let conn = Connection::connect_to_env().expect("Failed to connect to Wayland");
    let (globals, mut queue) = registry_queue_init::<State>(&conn).expect("Failed to init registry");
    let qh = queue.handle();

    let mut state = State {
        devices: HashMap::new(),
        initial_done: false,
    };

    let _list: XfceInputDeviceListPrivateV1 = globals
        .bind(&qh, 1..=1, ())
        .expect("Compositor does not support xfce_input_device_list_private_v1");

    println!("Bound xfce_input_device_list_private_v1; enumerating devices...");

    loop {
        queue.blocking_dispatch(&mut state).expect("Dispatch failed");
    }
}
