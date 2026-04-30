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

use std::{thread, time::Duration};

use x11rb::{
    COPY_DEPTH_FROM_PARENT,
    connection::Connection,
    protocol::{
        Event,
        xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask, PropMode, WindowClass},
    },
    wrapper::ConnectionExt as _,
};

const _NET_WM_STATE_ADD: u32 = 1;

fn main() {
    let (conn, screen_num) = x11rb::connect(None).unwrap();
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;

    let wm_state = conn.intern_atom(false, b"_NET_WM_STATE").unwrap().reply().unwrap().atom;
    let demands_attention = conn
        .intern_atom(false, b"_NET_WM_STATE_DEMANDS_ATTENTION")
        .unwrap()
        .reply()
        .unwrap()
        .atom;

    let win = conn.generate_id().unwrap();
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        root,
        100,
        100,
        300,
        200,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new()
            .background_pixel(0x006688aa)
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY | EventMask::KEY_PRESS),
    )
    .unwrap();

    conn.change_property8(
        PropMode::REPLACE,
        win,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        b"X11 Demands Attention Test",
    )
    .unwrap();

    conn.map_window(win).unwrap();
    conn.flush().unwrap();

    let delay = std::env::args().nth(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(3);

    eprintln!("Window mapped. Will set _NET_WM_STATE_DEMANDS_ATTENTION in {delay}s...");
    eprintln!("(Pass a number as argument to change the delay)");

    thread::spawn(move || {
        thread::sleep(Duration::from_secs(delay));

        let (conn2, screen_num2) = x11rb::connect(None).unwrap();
        let root2 = conn2.setup().roots[screen_num2].root;

        let wm_state2 = conn2.intern_atom(false, b"_NET_WM_STATE").unwrap().reply().unwrap().atom;
        let demands2 = conn2
            .intern_atom(false, b"_NET_WM_STATE_DEMANDS_ATTENTION")
            .unwrap()
            .reply()
            .unwrap()
            .atom;

        let data = [_NET_WM_STATE_ADD, demands2, 0, 1, 0];
        let event = ClientMessageEvent::new(32, win, wm_state2, data);

        conn2
            .send_event(
                false,
                root2,
                EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
                event,
            )
            .unwrap();
        conn2.flush().unwrap();

        eprintln!("Sent _NET_WM_STATE_DEMANDS_ATTENTION for window 0x{win:x}");
    });

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
            Event::PropertyNotify(e) if e.atom == wm_state => {
                let reply = conn
                    .get_property(false, win, wm_state, AtomEnum::ATOM, 0, 1024)
                    .unwrap()
                    .reply()
                    .unwrap();
                let atoms: &[u32] = reply
                    .value32()
                    .map(|v| {
                        // Safety: we're just reading the values from the iterator
                        // and collecting isn't possible without allocation, so
                        // we print inline instead.
                        let atoms: Vec<u32> = v.collect();
                        eprintln!("_NET_WM_STATE changed:");
                        for atom in &atoms {
                            let name = conn.get_atom_name(*atom).unwrap().reply().unwrap();
                            eprintln!("  {}", String::from_utf8_lossy(&name.name));
                        }
                        &[] as &[u32]
                    })
                    .unwrap_or(&[]);
                let _ = atoms;
                let has_attention = reply
                    .value32()
                    .map(|v| v.into_iter().any(|a| a == demands_attention))
                    .unwrap_or(false);
                if has_attention {
                    eprintln!("  -> DEMANDS_ATTENTION is set!");
                }
            }
            _ => {}
        }
    }
}
