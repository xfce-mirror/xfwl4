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
    delegate_compositor, delegate_output, delegate_registry, delegate_shm, delegate_xdg_window,
    globals::GlobalData,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            LoopHandle,
            timer::{TimeoutAction, Timer},
        },
        client::{
            Connection, Dispatch, QueueHandle, delegate_dispatch,
            protocol::{
                wl_output::{Transform, WlOutput},
                wl_surface::WlSurface,
            },
        },
        protocols::xdg::{
            decoration::zv1::client::{
                zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
            },
            shell::client::xdg_wm_base::{self, XdgWmBase},
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        xdg::{
            XdgShell,
            window::{Window, WindowConfigure, WindowData, WindowDecorations, WindowHandler},
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use test_clients::wayland::{apply_window_configure, init_event_loop, paint_solid};

const DEFAULT_SIZE: (u32, u32) = (400, 300);

// smithay-client-toolkit's XdgShell pongs from its own xdg_wm_base dispatch with no way to
// intercept it, so this example delegates every xdg shell interface except xdg_wm_base.
struct XdgSlowPing {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,

    window: Window,
    loop_handle: LoopHandle<'static, Self>,
    ping_delay: Duration,

    first_configure: bool,
    buffer: Option<Buffer>,
    width: u32,
    height: u32,
}

impl XdgSlowPing {
    fn draw(&mut self, qh: &QueueHandle<Self>) {
        paint_solid(
            &mut self.pool,
            &mut self.buffer,
            self.window.wl_surface(),
            qh,
            self.width,
            self.height,
            [0x66, 0x44, 0xaa, 0xff],
        );
        self.window.commit();
    }
}

fn main() {
    let ping_delay = Duration::from_secs(std::env::args().nth(1).and_then(|arg| arg.parse().ok()).unwrap_or(10));

    let (_conn, globals, qh, mut event_loop) = init_event_loop::<XdgSlowPing>();

    let compositor = CompositorState::bind(&globals, &qh).unwrap();
    let shm = Shm::bind(&globals, &qh).unwrap();
    let pool = SlotPool::new(DEFAULT_SIZE.0 as usize * DEFAULT_SIZE.1 as usize * 4, &shm).unwrap();
    let xdg_shell = XdgShell::bind(&globals, &qh).unwrap();

    let surface = compositor.create_surface(&qh);
    let window = xdg_shell.create_window(surface, WindowDecorations::RequestServer, &qh);
    window.set_title("Wayland Slow Ping Test");
    window.set_app_id("org.xfce.xfwl4.xdg-slow-ping-test");
    window.set_min_size(Some((100, 100)));
    window.commit();

    let mut state = XdgSlowPing {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        shm,
        pool,
        window,
        loop_handle: event_loop.handle(),
        ping_delay,
        first_configure: true,
        buffer: None,
        width: DEFAULT_SIZE.0,
        height: DEFAULT_SIZE.1,
    };

    eprintln!(
        "Window mapped; will pong xdg_wm_base pings after {}s, and ignore close requests",
        ping_delay.as_secs()
    );

    event_loop.run(Duration::from_millis(16), &mut state, |_state| {}).unwrap();
}

impl Dispatch<XdgWmBase, GlobalData> for XdgSlowPing {
    fn event(
        state: &mut Self,
        xdg_wm_base: &XdgWmBase,
        event: xdg_wm_base::Event,
        _data: &GlobalData,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            eprintln!("ping (serial {serial}); ponging in {}s", state.ping_delay.as_secs());

            let xdg_wm_base = xdg_wm_base.clone();
            state
                .loop_handle
                .insert_source(Timer::from_duration(state.ping_delay), move |_, _, _| {
                    xdg_wm_base.pong(serial);
                    eprintln!("ponged serial {serial}");
                    TimeoutAction::Drop
                })
                .unwrap();
        }
    }
}

impl ProvidesRegistryState for XdgSlowPing {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

impl CompositorHandler for XdgSlowPing {
    fn frame(&mut self, _conn: &Connection, qh: &QueueHandle<Self>, _surface: &WlSurface, _time: u32) {
        self.draw(qh);
    }

    fn surface_enter(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _surface: &WlSurface, _output: &WlOutput) {}

    fn surface_leave(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _surface: &WlSurface, _output: &WlOutput) {}

    fn transform_changed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _surface: &WlSurface, _new_transform: Transform) {}

    fn scale_factor_changed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _surface: &WlSurface, _new_factor: i32) {}
}

impl OutputHandler for XdgSlowPing {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}

    fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}

    fn output_destroyed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}
}

impl ShmHandler for XdgSlowPing {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl WindowHandler for XdgSlowPing {
    fn configure(&mut self, _conn: &Connection, qh: &QueueHandle<Self>, _window: &Window, configure: WindowConfigure, _serial: u32) {
        let (new_w, new_h, redraw) = apply_window_configure(&configure, self.first_configure, (self.width, self.height), DEFAULT_SIZE);
        if redraw {
            self.first_configure = false;
            self.buffer = None;
            self.width = new_w;
            self.height = new_h;
            self.draw(qh);
        } else {
            self.window.commit();
        }
    }

    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _window: &Window) {
        eprintln!("close requested, ignoring");
    }
}

delegate_registry!(XdgSlowPing);
delegate_compositor!(XdgSlowPing);
delegate_output!(XdgSlowPing);
delegate_shm!(XdgSlowPing);
delegate_xdg_window!(XdgSlowPing);
delegate_dispatch!(XdgSlowPing: [ZxdgDecorationManagerV1: GlobalData] => XdgShell);
delegate_dispatch!(XdgSlowPing: [ZxdgToplevelDecorationV1: WindowData] => XdgShell);
