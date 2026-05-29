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
                default,
                current,
            } => {
                println!("[{label}] send_events: supported={supported:?} default={default:?} current={current:?}");
            }
            Event::AccelSpeed { default, current } => {
                println!("[{label}] accel_speed: default={default} current={current}");
            }
            Event::AccelProfile {
                supported,
                default,
                current,
            } => {
                println!("[{label}] accel_profile: supported={supported:?} default={default:?} current={current:?}");
            }
            Event::NaturalScroll { default, current } => {
                println!("[{label}] natural_scroll: default={default:?} current={current:?}");
            }
            Event::ScrollMethod {
                supported,
                default,
                current,
            } => {
                println!("[{label}] scroll_method: supported={supported:?} default={default:?} current={current:?}");
            }
            Event::ScrollButton { default, current } => {
                println!("[{label}] scroll_button: default={default} current={current}");
            }
            Event::ScrollButtonLock { default, current } => {
                println!("[{label}] scroll_button_lock: default={default:?} current={current:?}");
            }
            Event::ClickMethod {
                supported,
                default,
                current,
            } => {
                println!("[{label}] click_method: supported={supported:?} default={default:?} current={current:?}");
            }
            Event::ClickfingerButtonMap { default, current } => {
                println!("[{label}] clickfinger_button_map: default={default:?} current={current:?}");
            }
            Event::LeftHanded { default, current } => {
                println!("[{label}] left_handed: default={default:?} current={current:?}");
            }
            Event::MiddleEmulation { default, current } => {
                println!("[{label}] middle_emulation: default={default:?} current={current:?}");
            }
            Event::Tap { default, current } => {
                println!("[{label}] tap: default={default:?} current={current:?}");
            }
            Event::TapFingerCount { count } => {
                println!("[{label}] tap_finger_count: {count}");
            }
            Event::TapButtonMap { default, current } => {
                println!("[{label}] tap_button_map: default={default:?} current={current:?}");
            }
            Event::TapDrag { default, current } => {
                println!("[{label}] tap_drag: default={default:?} current={current:?}");
            }
            Event::TapDragLock { default, current } => {
                println!("[{label}] tap_drag_lock: default={default:?} current={current:?}");
            }
            Event::ThreeFingerDrag {
                max_fingers,
                default,
                current,
            } => {
                println!("[{label}] three_finger_drag: max_fingers={max_fingers} default={default:?} current={current:?}");
            }
            Event::Dwt { default, current } => {
                println!("[{label}] dwt: default={default:?} current={current:?}");
            }
            Event::DwtTimeout { default, current } => {
                println!("[{label}] dwt_timeout: default={default}ms current={current}ms");
            }
            Event::Dwtp { default, current } => {
                println!("[{label}] dwtp: default={default:?} current={current:?}");
            }
            Event::DwtpTimeout { default, current } => {
                println!("[{label}] dwtp_timeout: default={default}ms current={current}ms");
            }
            Event::Rotation { default, current } => {
                println!("[{label}] rotation: default={default}° current={current}°");
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
                    "[{label}] calibration: default=[{default_a}, {default_b}, {default_c}, {default_d}, {default_e}, {default_f}] \
                     current=[{current_a}, {current_b}, {current_c}, {current_d}, {current_e}, {current_f}]"
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
                    "[{label}] area: default=({default_x1},{default_y1})..({default_x2},{default_y2}) \
                     current=({current_x1},{current_y1})..({current_x2},{current_y2})"
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
