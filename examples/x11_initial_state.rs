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

use x11rb::COPY_DEPTH_FROM_PARENT;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::*;
use x11rb::wrapper::ConnectionExt as _;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let modes: Vec<String> = if args.is_empty() { vec!["fullscreen".to_string()] } else { args };
    let state_atom_names: Vec<&[u8]> = modes
        .iter()
        .flat_map(|mode| {
            let atoms: &[&[u8]] = match mode.as_str() {
                "hidden" => &[b"_NET_WM_STATE_HIDDEN"],
                "fullscreen" => &[b"_NET_WM_STATE_FULLSCREEN"],
                "above" => &[b"_NET_WM_STATE_ABOVE"],
                "below" => &[b"_NET_WM_STATE_BELOW"],
                "shaded" => &[b"_NET_WM_STATE_SHADED"],
                "sticky" => &[b"_NET_WM_STATE_STICKY"],
                "maximized" => &[b"_NET_WM_STATE_MAXIMIZED_VERT", b"_NET_WM_STATE_MAXIMIZED_HORZ"],
                _ => {
                    eprintln!("Usage: x11_initial_state [hidden|fullscreen|above|below|shaded|sticky|maximized]...");
                    eprintln!();
                    eprintln!("Sets _NET_WM_STATE on the window *before* mapping it, to test");
                    eprintln!("that the compositor honors the initial state. Multiple states may");
                    eprintln!("be specified, and their atoms are combined into one property.");
                    std::process::exit(2);
                }
            };
            atoms.iter().copied()
        })
        .collect();

    let (conn, screen_num) = x11rb::connect(None).unwrap();
    let screen = &conn.setup().roots[screen_num];

    let wm_state = conn.intern_atom(false, b"_NET_WM_STATE").unwrap().reply().unwrap().atom;
    let state_atoms: Vec<u32> = state_atom_names
        .iter()
        .map(|name| conn.intern_atom(false, name).unwrap().reply().unwrap().atom)
        .collect();

    let win = conn.generate_id().unwrap();
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        screen.root,
        100,
        100,
        400,
        300,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new()
            .background_pixel(0x0066aa44)
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY | EventMask::KEY_PRESS | EventMask::PROPERTY_CHANGE),
    )
    .unwrap();

    let title = format!("X11 Initial State: {}", modes.join(", "));
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())
        .unwrap();

    conn.change_property32(PropMode::REPLACE, win, wm_state, AtomEnum::ATOM, &state_atoms)
        .unwrap();

    eprintln!("Set _NET_WM_STATE on window 0x{win:x} before mapping:");
    for name in state_atom_names {
        eprintln!("  {}", String::from_utf8_lossy(name));
    }

    conn.map_window(win).unwrap();
    conn.flush().unwrap();

    eprintln!("Window mapped. Press a key in the window to exit.");

    loop {
        let event = conn.wait_for_event().unwrap();
        match event {
            Event::Expose(_) => {
                eprintln!("Expose");
            }
            Event::KeyPress(_) => {
                eprintln!("KeyPress, exiting");
                break;
            }
            Event::ConfigureNotify(e) => {
                eprintln!("ConfigureNotify: {}x{} at ({}, {})", e.width, e.height, e.x, e.y);
            }
            Event::PropertyNotify(e) if e.atom == wm_state => {
                let reply = conn
                    .get_property(false, win, wm_state, AtomEnum::ATOM, 0, 1024)
                    .unwrap()
                    .reply()
                    .unwrap();
                eprintln!("_NET_WM_STATE changed:");
                if let Some(v) = reply.value32() {
                    for atom in v {
                        let name = conn.get_atom_name(atom).unwrap().reply().unwrap();
                        eprintln!("  {}", String::from_utf8_lossy(&name.name));
                    }
                }
            }
            _ => {}
        }
    }
}
