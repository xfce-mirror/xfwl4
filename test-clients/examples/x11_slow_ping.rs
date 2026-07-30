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

use std::{fs, thread, time::Duration};

use x11rb::{
    COPY_DEPTH_FROM_PARENT,
    connection::Connection,
    protocol::{
        Event,
        xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask, PropMode, WindowClass},
    },
    wrapper::ConnectionExt as _,
};

fn main() {
    let delay = std::env::args().nth(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(10);

    let (conn, screen_num) = x11rb::connect(None).unwrap();
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;

    let wm_protocols = conn.intern_atom(false, b"WM_PROTOCOLS").unwrap().reply().unwrap().atom;
    let wm_delete_window = conn.intern_atom(false, b"WM_DELETE_WINDOW").unwrap().reply().unwrap().atom;
    let net_wm_ping = conn.intern_atom(false, b"_NET_WM_PING").unwrap().reply().unwrap().atom;
    let net_wm_pid = conn.intern_atom(false, b"_NET_WM_PID").unwrap().reply().unwrap().atom;

    let win = conn.generate_id().unwrap();
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        root,
        100,
        100,
        400,
        300,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new()
            .background_pixel(0x00aa4466)
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY | EventMask::KEY_PRESS),
    )
    .unwrap();

    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, b"X11 Slow Ping Test")
        .unwrap();
    conn.change_property32(
        PropMode::REPLACE,
        win,
        wm_protocols,
        AtomEnum::ATOM,
        &[wm_delete_window, net_wm_ping],
    )
    .unwrap();
    conn.change_property32(PropMode::REPLACE, win, net_wm_pid, AtomEnum::CARDINAL, &[std::process::id()])
        .unwrap();

    // The compositor only kills by pid when WM_CLIENT_MACHINE matches its own
    // hostname; without it, it falls back to KillClient.
    let hostname = fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
    conn.change_property8(
        PropMode::REPLACE,
        win,
        AtomEnum::WM_CLIENT_MACHINE,
        AtomEnum::STRING,
        hostname.trim().as_bytes(),
    )
    .unwrap();

    conn.map_window(win).unwrap();
    conn.flush().unwrap();

    eprintln!("Window 0x{win:x} mapped; will reply to _NET_WM_PING after {delay}s");

    loop {
        let event = conn.wait_for_event().unwrap();
        match event {
            Event::KeyPress(_) => break,
            Event::ClientMessage(e) if e.type_ == wm_protocols => {
                let data = e.data.as_data32();
                if data[0] == net_wm_ping {
                    eprintln!("_NET_WM_PING received; sleeping {delay}s before replying");
                    thread::sleep(Duration::from_secs(delay));

                    let reply = ClientMessageEvent::new(32, root, wm_protocols, data);
                    conn.send_event(
                        false,
                        root,
                        EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
                        reply,
                    )
                    .unwrap();
                    conn.flush().unwrap();

                    eprintln!("_NET_WM_PING replied");
                } else if data[0] == wm_delete_window {
                    eprintln!("WM_DELETE_WINDOW received, ignoring");
                }
            }
            _ => {}
        }
    }
}
