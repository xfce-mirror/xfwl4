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

use std::time::Duration;

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_output, delegate_registry, delegate_shm, delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    reexports::{
        client::{Connection, Dispatch, Proxy, QueueHandle, protocol::wl_shm},
        protocols::xdg::toplevel_icon::v1::client::{
            xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1, xdg_toplevel_icon_v1::XdgToplevelIconV1,
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        xdg::{
            XdgShell,
            window::{Window, WindowDecorations, WindowHandler},
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use test_clients::wayland::{apply_window_configure, init_event_loop, paint_solid};

struct XdgToplevelIconExample {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,

    window: Window,

    first_configure: bool,
    buffer: Option<Buffer>,
    width: u32,
    height: u32,
}

impl XdgToplevelIconExample {
    fn draw(&mut self, _conn: &Connection, qh: &QueueHandle<Self>) {
        paint_solid(
            &mut self.pool,
            &mut self.buffer,
            self.window.wl_surface(),
            qh,
            self.width,
            self.height,
            [0xff, 0xff, 0x00, 0x00],
        );
        self.window.commit();
    }
}

fn main() {
    let use_buffer = std::env::args().nth(1).is_some_and(|arg| arg == "--use-buffer");

    let (_conn, globals, qh, mut event_loop) = init_event_loop::<XdgToplevelIconExample>();

    let compositor = CompositorState::bind(&globals, &qh).unwrap();
    let shm = Shm::bind(&globals, &qh).unwrap();
    let mut pool = SlotPool::new(100 * 100 * 4 + 256 * 256 * 4, &shm).unwrap();
    let xdg_shell = XdgShell::bind(&globals, &qh).unwrap();

    let icon_manager = globals
        .bind::<XdgToplevelIconManagerV1, XdgToplevelIconExample, _>(&qh, 1..=1, ())
        .unwrap();

    let icon = icon_manager.create_icon(&qh, ());
    let _icon_buffer = if use_buffer {
        eprintln!("drawing icon into buffer");

        let path = freedesktop_icons::lookup("video-display")
            .with_size(256)
            .with_scale(2)
            .with_theme("elementary-xfce-hidpi")
            .find()
            .unwrap();
        eprintln!("will load icon from {}", path.display());
        let pixbuf = gdk_pixbuf::Pixbuf::from_file_at_scale(path, 256, 256, true).unwrap();
        let pixbuf = if !pixbuf.has_alpha() {
            pixbuf.add_alpha(true, 255, 255, 255).unwrap()
        } else {
            pixbuf
        };

        let (buffer, canvas) = pool
            .create_buffer(pixbuf.width(), pixbuf.height(), pixbuf.width() * 4, wl_shm::Format::Abgr8888)
            .unwrap();
        let pixels = pixbuf.read_pixel_bytes();

        assert!(canvas.len() == pixels.len());
        canvas.copy_from_slice(&pixels);
        buffer.activate().unwrap();

        icon.add_buffer(buffer.wl_buffer(), 2);
        Some(buffer)
    } else {
        icon.set_name("video-display".to_owned());
        None
    };

    let surface = compositor.create_surface(&qh);
    let window = xdg_shell.create_window(surface, WindowDecorations::RequestServer, &qh);
    window.set_title("XDG toplevel icon test");
    window.set_app_id("org.xfce.xfwl4.xdg-toplevel-icon-test");
    window.set_min_size(Some((100, 100)));
    icon_manager.set_icon(window.xdg_toplevel(), Some(&icon));
    window.commit();

    let mut state = XdgToplevelIconExample {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        shm,
        pool,
        window,
        first_configure: true,
        buffer: None,
        width: 100,
        height: 100,
    };

    event_loop.run(Duration::from_millis(16), &mut state, |_state| {}).unwrap();
}

impl Dispatch<XdgToplevelIconManagerV1, ()> for XdgToplevelIconExample {
    fn event(
        _state: &mut Self,
        _proxy: &XdgToplevelIconManagerV1,
        _event: <XdgToplevelIconManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgToplevelIconV1, ()> for XdgToplevelIconExample {
    fn event(
        _state: &mut Self,
        _proxy: &XdgToplevelIconV1,
        _event: <XdgToplevelIconV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl ProvidesRegistryState for XdgToplevelIconExample {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

impl CompositorHandler for XdgToplevelIconExample {
    fn frame(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        _surface: &smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface,
        _time: u32,
    ) {
        self.draw(conn, qh);
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface,
        _output: &smithay_client_toolkit::reexports::client::protocol::wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface,
        _output: &smithay_client_toolkit::reexports::client::protocol::wl_output::WlOutput,
    ) {
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface,
        _new_transform: smithay_client_toolkit::reexports::client::protocol::wl_output::Transform,
    ) {
    }

    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface,
        _new_factor: i32,
    ) {
    }
}

impl OutputHandler for XdgToplevelIconExample {
    fn output_state(&mut self) -> &mut smithay_client_toolkit::output::OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: smithay_client_toolkit::reexports::client::protocol::wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: smithay_client_toolkit::reexports::client::protocol::wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: smithay_client_toolkit::reexports::client::protocol::wl_output::WlOutput,
    ) {
    }
}

impl ShmHandler for XdgToplevelIconExample {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl WindowHandler for XdgToplevelIconExample {
    fn configure(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        _window: &smithay_client_toolkit::shell::xdg::window::Window,
        configure: smithay_client_toolkit::shell::xdg::window::WindowConfigure,
        _serial: u32,
    ) {
        eprintln!("configure!");
        let (new_w, new_h, redraw) = apply_window_configure(&configure, self.first_configure, (self.width, self.height), (100, 100));
        if redraw {
            self.first_configure = false;
            self.buffer = None;
            self.width = new_w;
            self.height = new_h;
            self.draw(conn, qh);
        } else {
            self.window.commit();
        }
    }

    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _window: &smithay_client_toolkit::shell::xdg::window::Window) {
        std::process::exit(0);
    }
}

delegate_registry!(XdgToplevelIconExample);
delegate_compositor!(XdgToplevelIconExample);
delegate_output!(XdgToplevelIconExample);
delegate_shm!(XdgToplevelIconExample);
delegate_xdg_shell!(XdgToplevelIconExample);
delegate_xdg_window!(XdgToplevelIconExample);
