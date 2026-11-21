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

//! Test client for xdg_toplevel state requests issued before the toplevel is mapped.
//!
//! Each request is sent in order before the first commit, or, with `--after-map`, after
//! the first buffer commit, so the pre-map and post-map paths can be compared.  Every
//! configure the compositor sends is printed, along with the advertised outputs and the
//! output the surface enters, so the configured size can be compared with the output it
//! should have come from.

use std::{num::NonZeroU32, time::Duration};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_dispatch2, delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::client::{Connection, QueueHandle, protocol::wl_output},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
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

const DEFAULT_WIDTH: u32 = 400;
const DEFAULT_HEIGHT: u32 = 300;

#[derive(Clone, Copy, Debug)]
enum Request {
    Maximize,
    Unmaximize,
    Fullscreen(Option<usize>),
    Unfullscreen,
    Minimize,
}

struct XdgInitialStateExample {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,

    window: Window,

    outputs: Vec<wl_output::WlOutput>,
    requests: Vec<Request>,
    after_map: bool,
    requests_applied: bool,

    configure_count: u32,
    first_configure: bool,
    buffer: Option<Buffer>,
    width: u32,
    height: u32,
}

impl XdgInitialStateExample {
    fn apply_request(&self, request: Request) {
        eprintln!(">>> {request:?}");
        match request {
            Request::Maximize => self.window.set_maximized(),
            Request::Unmaximize => self.window.unset_maximized(),
            Request::Fullscreen(None) => self.window.set_fullscreen(None),
            Request::Fullscreen(Some(index)) => match self.outputs.get(index) {
                Some(output) => self.window.set_fullscreen(Some(output)),
                None => {
                    eprintln!("    (no output index {index}; sending set_fullscreen() without one)");
                    self.window.set_fullscreen(None);
                }
            },
            Request::Unfullscreen => self.window.unset_fullscreen(),
            Request::Minimize => self.window.set_minimized(),
        }
    }

    fn output_label(&self, output: &wl_output::WlOutput) -> String {
        match self.outputs.iter().position(|candidate| candidate == output) {
            Some(index) => index.to_string(),
            None => "?".to_owned(),
        }
    }

    fn print_output(&self, output: &wl_output::WlOutput) {
        let label = self.output_label(output);
        match self.output_state.info(output) {
            Some(info) => eprintln!(
                "output[{label}] name={} logical_position={:?} logical_size={:?} scale={}",
                info.name.as_deref().unwrap_or("<unnamed>"),
                info.logical_position,
                info.logical_size,
                info.scale_factor,
            ),
            None => eprintln!("output[{label}] name=<unknown>"),
        }
    }

    fn print_outputs(&self) {
        if self.outputs.is_empty() {
            eprintln!("outputs: none advertised");
        }
        for output in &self.outputs {
            self.print_output(output);
        }
    }

    fn draw(&mut self, _conn: &Connection, qh: &QueueHandle<Self>) {
        paint_solid(
            &mut self.pool,
            &mut self.buffer,
            self.window.wl_surface(),
            qh,
            self.width,
            self.height,
            [0xff, 0x33, 0x99, 0xcc],
        );
        self.window.commit();
    }
}

fn usage(reason: &str) -> ! {
    eprintln!("error: {reason}");
    eprintln!();
    eprintln!("Usage: xdg_initial_state [--after-map] REQUEST...");
    eprintln!();
    eprintln!("Sends each REQUEST, in order, before the first commit.  With --after-map, the");
    eprintln!("requests are sent after the first buffer commit instead, so the pre-map and");
    eprintln!("post-map paths can be compared with the same sequence.");
    eprintln!();
    eprintln!("Requests:");
    eprintln!("  maximize        xdg_toplevel.set_maximized");
    eprintln!("  unmaximize      xdg_toplevel.unset_maximized");
    eprintln!("  fullscreen      xdg_toplevel.set_fullscreen, without an output");
    eprintln!("  fullscreen:N    xdg_toplevel.set_fullscreen on the Nth advertised output");
    eprintln!("  unfullscreen    xdg_toplevel.unset_fullscreen");
    eprintln!("  minimize        xdg_toplevel.set_minimized");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  xdg_initial_state maximize");
    eprintln!("  xdg_initial_state minimize");
    eprintln!("  xdg_initial_state maximize fullscreen");
    eprintln!("  xdg_initial_state fullscreen maximize");
    eprintln!("  xdg_initial_state maximize fullscreen unfullscreen");
    eprintln!("  xdg_initial_state fullscreen:1");
    eprintln!("  xdg_initial_state --after-map maximize fullscreen");
    std::process::exit(2);
}

fn main() {
    let mut after_map = false;
    let mut requests = Vec::new();

    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--after-map" => after_map = true,
            "maximize" => requests.push(Request::Maximize),
            "unmaximize" => requests.push(Request::Unmaximize),
            "fullscreen" => requests.push(Request::Fullscreen(None)),
            "unfullscreen" => requests.push(Request::Unfullscreen),
            "minimize" => requests.push(Request::Minimize),
            other => match other.strip_prefix("fullscreen:") {
                Some(index) => match index.parse::<usize>() {
                    Ok(index) => requests.push(Request::Fullscreen(Some(index))),
                    Err(_) => usage(&format!("invalid output index in '{other}'")),
                },
                None => usage(&format!("unknown argument '{other}'")),
            },
        }
    }

    if requests.is_empty() {
        usage("no requests given");
    }

    let (_conn, globals, qh, mut event_loop) = init_event_loop::<XdgInitialStateExample>();

    let compositor = CompositorState::bind(&globals, &qh).unwrap();
    let shm = Shm::bind(&globals, &qh).unwrap();
    let pool = SlotPool::new(DEFAULT_WIDTH as usize * DEFAULT_HEIGHT as usize * 4, &shm).unwrap();
    let xdg_shell = XdgShell::bind(&globals, &qh).unwrap();

    let surface = compositor.create_surface(&qh);
    let window = xdg_shell.create_window(surface, WindowDecorations::RequestServer, &qh);
    window.set_title("XDG initial state test");
    window.set_app_id("org.xfce.xfwl4.xdg-initial-state-test");

    let output_state = OutputState::new(&globals, &qh);
    let outputs = output_state.outputs().collect::<Vec<_>>();

    let mut state = XdgInitialStateExample {
        registry_state: RegistryState::new(&globals),
        output_state,
        shm,
        pool,
        window,
        outputs,
        requests: requests.clone(),
        after_map,
        requests_applied: false,
        configure_count: 0,
        first_configure: true,
        buffer: None,
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
    };

    eprintln!("outputs advertised by the compositor:");
    state.print_outputs();

    if after_map {
        eprintln!("mapping the window, then sending the requests:");
        state.window.commit();
    } else {
        eprintln!("sending the requests before the first commit:");
        for request in requests {
            state.apply_request(request);
        }
        state.window.commit();
    }

    event_loop.run(Duration::from_millis(16), &mut state, |_state| {}).unwrap();
}

impl ProvidesRegistryState for XdgInitialStateExample {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

impl CompositorHandler for XdgInitialStateExample {
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
        output: &wl_output::WlOutput,
    ) {
        let label = self.output_label(output);
        let name = self
            .output_state
            .info(output)
            .and_then(|info| info.name)
            .unwrap_or_else(|| "<unnamed>".to_owned());
        eprintln!("surface entered output[{label}] {name}");
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface,
        output: &wl_output::WlOutput,
    ) {
        let label = self.output_label(output);
        eprintln!("surface left output[{label}]");
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

impl OutputHandler for XdgInitialStateExample {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.print_output(&output);
    }

    fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.print_output(&output);
    }

    fn output_destroyed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let label = self.output_label(&output);
        eprintln!("output[{label}] destroyed");
    }
}

impl ShmHandler for XdgInitialStateExample {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl WindowHandler for XdgInitialStateExample {
    fn configure(&mut self, conn: &Connection, qh: &QueueHandle<Self>, _window: &Window, configure: WindowConfigure, serial: u32) {
        self.configure_count += 1;

        let size = match (configure.new_size.0.map(NonZeroU32::get), configure.new_size.1.map(NonZeroU32::get)) {
            (Some(width), Some(height)) => format!("{width}x{height}"),
            _ => "none".to_owned(),
        };
        eprintln!(
            "configure #{} serial={} size={} suggested_bounds={:?} state={:?} decoration={:?}",
            self.configure_count, serial, size, configure.suggested_bounds, configure.state, configure.decoration_mode,
        );

        let (new_width, new_height, redraw) = apply_window_configure(
            &configure,
            self.first_configure,
            (self.width, self.height),
            (DEFAULT_WIDTH, DEFAULT_HEIGHT),
        );
        if redraw {
            self.first_configure = false;
            self.buffer = None;
            self.width = new_width;
            self.height = new_height;
            self.draw(conn, qh);
        } else {
            self.window.commit();
        }

        if self.after_map && !self.requests_applied {
            self.requests_applied = true;
            for request in self.requests.clone() {
                self.apply_request(request);
            }
        }
    }

    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _window: &Window) {
        std::process::exit(0);
    }
}

delegate_registry!(XdgInitialStateExample);
delegate_dispatch2!(XdgInitialStateExample);
