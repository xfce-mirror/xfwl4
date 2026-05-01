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

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::*;
use x11rb::wrapper::ConnectionExt as _;

const XK_ESCAPE: u32 = 0xff1b;

const WIN_W: u16 = 400;
const WIN_H: u16 = 300;

// Premultiplied ARGB: 50% alpha over a dark blue-gray. With premultiplied
// colors, alpha=0x80 means RGB channels can't exceed 0x80.
const BG_PIXEL: u32 = 0x80404060;
// Fully opaque orange used to outline the declared-opaque rectangles, so the
// "wrong" regions are visible.
const OUTLINE_PIXEL: u32 = 0xffff8000;

// Two declared-opaque rectangles as fractions of (width, height), deliberately
// not actually opaque. They scale with the window so resizes still cover the
// "wrong" pixels.
fn compute_rects(w: u16, h: u16) -> [(i16, i16, u16, u16); 2] {
    let fw = f32::from(w);
    let fh = f32::from(h);
    [
        ((fw * 0.10) as i16, (fh * 0.13) as i16, (fw * 0.30) as u16, (fh * 0.30) as u16),
        ((fw * 0.60) as i16, (fh * 0.53) as i16, (fw * 0.32) as u16, (fh * 0.33) as u16),
    ]
}

fn set_opaque_region<C: Connection>(conn: &C, win: u32, atom: u32, w: u16, h: u16) {
    let rects = compute_rects(w, h);
    let region: Vec<u32> = rects
        .iter()
        .flat_map(|(x, y, rw, rh)| [*x as u32, *y as u32, u32::from(*rw), u32::from(*rh)])
        .collect();
    conn.change_property32(PropMode::REPLACE, win, atom, AtomEnum::CARDINAL, &region)
        .unwrap();
    eprintln!("Set _NET_WM_OPAQUE_REGION for {w}x{h}:");
    for (x, y, rw, rh) in &rects {
        eprintln!("  x={x} y={y} w={rw} h={rh}");
    }
}

fn paint<C: Connection>(conn: &C, win: u32, gc: u32, w: u16, h: u16) {
    conn.change_gc(gc, &ChangeGCAux::new().foreground(BG_PIXEL)).unwrap();
    conn.poly_fill_rectangle(
        win,
        gc,
        &[Rectangle {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }],
    )
    .unwrap();

    conn.change_gc(gc, &ChangeGCAux::new().foreground(OUTLINE_PIXEL)).unwrap();
    let outlines: Vec<Rectangle> = compute_rects(w, h)
        .iter()
        .map(|(x, y, rw, rh)| Rectangle {
            x: *x,
            y: *y,
            width: *rw,
            height: *rh,
        })
        .collect();
    conn.poly_rectangle(win, gc, &outlines).unwrap();
}

fn main() {
    let (conn, screen_num) = x11rb::connect(None).unwrap();
    let setup = conn.setup();
    let screen = &setup.roots[screen_num];
    let min_keycode = setup.min_keycode;
    let max_keycode = setup.max_keycode;

    let visual_id = screen
        .allowed_depths
        .iter()
        .find(|d| d.depth == 32)
        .and_then(|d| d.visuals.first())
        .map(|v| v.visual_id)
        .expect("no 32-bit ARGB visual found on this screen");

    let colormap = conn.generate_id().unwrap();
    conn.create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual_id).unwrap();

    let win = conn.generate_id().unwrap();
    conn.create_window(
        32,
        win,
        screen.root,
        100,
        100,
        WIN_W,
        WIN_H,
        0,
        WindowClass::INPUT_OUTPUT,
        visual_id,
        &CreateWindowAux::new()
            .border_pixel(0)
            .colormap(colormap)
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY | EventMask::KEY_PRESS),
    )
    .unwrap();

    conn.change_property8(
        PropMode::REPLACE,
        win,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        b"X11 Opaque Region Test",
    )
    .unwrap();

    let opaque_region_atom = conn.intern_atom(false, b"_NET_WM_OPAQUE_REGION").unwrap().reply().unwrap().atom;

    let gc = conn.generate_id().unwrap();
    conn.create_gc(gc, win, &CreateGCAux::new()).unwrap();

    eprintln!("Window 0x{win:x} fill is translucent ARGB 0x{BG_PIXEL:08x} (premultiplied)");
    set_opaque_region(&conn, win, opaque_region_atom, WIN_W, WIN_H);

    let mapping = conn
        .get_keyboard_mapping(min_keycode, max_keycode - min_keycode + 1)
        .unwrap()
        .reply()
        .unwrap();
    let kpc = mapping.keysyms_per_keycode as usize;
    let escape_keycode = mapping
        .keysyms
        .chunks(kpc)
        .position(|syms| syms.first() == Some(&XK_ESCAPE))
        .map(|i| min_keycode + i as u8);

    conn.map_window(win).unwrap();
    conn.flush().unwrap();

    let mut last_size = (WIN_W, WIN_H);
    loop {
        let event = conn.wait_for_event().unwrap();
        match event {
            Event::Expose(e) if e.count == 0 => {
                let (w, h) = last_size;
                paint(&conn, win, gc, w, h);
                conn.flush().unwrap();
            }
            Event::Expose(_) => {}
            Event::KeyPress(e) if Some(e.detail) == escape_keycode => {
                eprintln!("Escape pressed, exiting");
                break;
            }
            Event::ConfigureNotify(e) => {
                eprintln!("ConfigureNotify: {}x{} at ({}, {})", e.width, e.height, e.x, e.y);
                if (e.width, e.height) != last_size {
                    last_size = (e.width, e.height);
                    set_opaque_region(&conn, win, opaque_region_atom, e.width, e.height);
                    paint(&conn, win, gc, e.width, e.height);
                    conn.flush().unwrap();
                }
            }
            _ => {}
        }
    }
}
