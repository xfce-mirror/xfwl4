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

use std::{env, process};

use smithay_client_toolkit::reexports::{
    client::{
        Connection, Dispatch, QueueHandle, delegate_noop,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{wl_output, wl_registry},
    },
    protocols_wlr::output_power_management::v1::client::{
        zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1,
        zwlr_output_power_v1::{self, ZwlrOutputPowerV1},
    },
};

struct State {
    target_name: String,
    target_mode: zwlr_output_power_v1::Mode,
    outputs: Vec<OutputInfo>,
    done: bool,
}

struct OutputInfo {
    output: wl_output::WlOutput,
    name: Option<String>,
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &wl_output::WlOutput,
        event: wl_output::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            if let Some(info) = state.outputs.iter_mut().find(|o| o.output == *proxy) {
                info.name = Some(name);
            }
        }
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

impl Dispatch<ZwlrOutputPowerV1, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrOutputPowerV1,
        event: zwlr_output_power_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_power_v1::Event::Mode { mode } => {
                let mode = match mode.into_result() {
                    Ok(mode) => mode,
                    Err(val) => {
                        eprintln!("Unknown mode value: {val}");
                        process::exit(1);
                    }
                };
                let mode_str = match mode {
                    zwlr_output_power_v1::Mode::On => "on",
                    zwlr_output_power_v1::Mode::Off => "off",
                    _ => "unknown",
                };
                eprintln!("Output power mode is now: {mode_str}");
                state.done = true;
            }
            zwlr_output_power_v1::Event::Failed => {
                eprintln!("Power management failed for this output");
                process::exit(1);
            }
            _ => {}
        }
    }
}

delegate_noop!(State: ignore ZwlrOutputPowerManagerV1);

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <output-name> <on|off>", args[0]);
        process::exit(1);
    }

    let target_name = &args[1];
    let target_mode = match args[2].as_str() {
        "on" => zwlr_output_power_v1::Mode::On,
        "off" => zwlr_output_power_v1::Mode::Off,
        other => {
            eprintln!("Invalid mode '{other}', expected 'on' or 'off'");
            process::exit(1);
        }
    };

    let conn = Connection::connect_to_env().expect("Failed to connect to Wayland");
    let (globals, mut queue) = registry_queue_init::<State>(&conn).expect("Failed to init registry");

    let mut state = State {
        target_name: target_name.clone(),
        target_mode,
        outputs: Vec::new(),
        done: false,
    };

    let qh = queue.handle();

    for global in globals.contents().clone_list() {
        if global.interface == "wl_output" {
            let output: wl_output::WlOutput = globals.registry().bind(global.name, global.version.min(4), &qh, ());
            state.outputs.push(OutputInfo { output, name: None });
        }
    }

    queue.roundtrip(&mut state).expect("Roundtrip failed");

    let target_output = state.outputs.iter().find(|o| o.name.as_deref() == Some(&state.target_name));

    let target_output = match target_output {
        Some(o) => &o.output,
        None => {
            eprintln!("Output '{}' not found. Available outputs:", state.target_name);
            for o in &state.outputs {
                if let Some(name) = &o.name {
                    eprintln!("  {name}");
                }
            }
            process::exit(1);
        }
    };

    let power_manager: ZwlrOutputPowerManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .expect("Compositor does not support wlr-output-power-management-unstable-v1");

    let power = power_manager.get_output_power(target_output, &qh, ());
    power.set_mode(state.target_mode.into());

    while !state.done {
        queue.blocking_dispatch(&mut state).expect("Dispatch failed");
    }

    power.destroy();
    power_manager.destroy();
}
