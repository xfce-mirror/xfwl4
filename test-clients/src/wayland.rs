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

use smithay_client_toolkit::{
    reexports::{
        calloop::EventLoop,
        calloop_wayland_source::WaylandSource,
        client::{
            Connection, Dispatch, QueueHandle,
            globals::{GlobalList, GlobalListContents, registry_queue_init},
            protocol::{wl_callback::WlCallback, wl_registry::WlRegistry, wl_shm, wl_surface::WlSurface},
        },
    },
    shell::{wlr_layer::LayerSurfaceConfigure, xdg::window::WindowConfigure},
    shm::slot::{Buffer, SlotPool},
};

pub fn init_event_loop<S>() -> (Connection, GlobalList, QueueHandle<S>, EventLoop<'static, S>)
where
    S: Dispatch<WlRegistry, GlobalListContents> + 'static,
{
    let conn = Connection::connect_to_env().unwrap();
    let (globals, event_queue) = registry_queue_init(&conn).unwrap();
    let qh = event_queue.handle();
    let event_loop = EventLoop::<S>::try_new().unwrap();
    let loop_handle = event_loop.handle();
    WaylandSource::new(conn.clone(), event_queue).insert(loop_handle).unwrap();
    (conn, globals, qh, event_loop)
}

pub fn paint_solid<S>(
    pool: &mut SlotPool,
    buffer: &mut Option<Buffer>,
    surface: &WlSurface,
    qh: &QueueHandle<S>,
    width: u32,
    height: u32,
    color: [u8; 4],
) where
    S: Dispatch<WlCallback, WlSurface> + 'static,
{
    let stride = width as i32 * 4;
    let buf = buffer.get_or_insert_with(|| {
        pool.create_buffer(width as i32, height as i32, stride, wl_shm::Format::Argb8888)
            .unwrap()
            .0
    });

    let canvas = match pool.canvas(buf) {
        Some(canvas) => canvas,
        None => {
            let (backup, canvas) = pool
                .create_buffer(width as i32, height as i32, stride, wl_shm::Format::Argb8888)
                .unwrap();
            *buf = backup;
            canvas
        }
    };

    for pixel in canvas.chunks_exact_mut(4) {
        pixel.copy_from_slice(&color);
    }

    surface.damage_buffer(0, 0, width as i32, height as i32);
    surface.frame(qh, surface.clone());
    buf.attach_to(surface).unwrap();
}

pub fn apply_window_configure(
    configure: &WindowConfigure,
    first_configure: bool,
    current: (u32, u32),
    default: (u32, u32),
) -> (u32, u32, bool) {
    let new_w = configure.new_size.0.map(|w| w.get()).unwrap_or(default.0);
    let new_h = configure.new_size.1.map(|h| h.get()).unwrap_or(default.1);
    let changed = new_w != current.0 || new_h != current.1;
    (new_w, new_h, first_configure || changed)
}

pub fn apply_layer_surface_configure(configure: &LayerSurfaceConfigure, first_configure: bool, current: (u32, u32)) -> (u32, u32, bool) {
    let new_w = if configure.new_size.0 > 0 {
        configure.new_size.0
    } else {
        current.0
    };
    let new_h = if configure.new_size.1 > 0 {
        configure.new_size.1
    } else {
        current.1
    };
    let changed = new_w != current.0 || new_h != current.1;
    (new_w, new_h, first_configure || changed)
}
