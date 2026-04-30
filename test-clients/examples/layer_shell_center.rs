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
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::client::{Connection, QueueHandle},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use test_clients::wayland::{apply_layer_surface_configure, init_event_loop, paint_solid};

const WIDTH: u32 = 300;
const HEIGHT: u32 = 200;

struct LayerShellExample {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,

    layer_surface: LayerSurface,

    first_configure: bool,
    buffer: Option<Buffer>,
    width: u32,
    height: u32,
}

impl LayerShellExample {
    fn draw(&mut self, _conn: &Connection, qh: &QueueHandle<Self>) {
        // Dark blue-gray fill (BGRA)
        paint_solid(
            &mut self.pool,
            &mut self.buffer,
            self.layer_surface.wl_surface(),
            qh,
            self.width,
            self.height,
            [0x60, 0x50, 0x40, 0xff],
        );
        self.layer_surface.commit();
    }
}

fn main() {
    let (_conn, globals, qh, mut event_loop) = init_event_loop::<LayerShellExample>();

    let compositor = CompositorState::bind(&globals, &qh).unwrap();
    let shm = Shm::bind(&globals, &qh).unwrap();
    let pool = SlotPool::new(WIDTH as usize * HEIGHT as usize * 4, &shm).unwrap();
    let layer_shell = LayerShell::bind(&globals, &qh).unwrap();

    let surface = compositor.create_surface(&qh);
    let layer_surface = layer_shell.create_layer_surface(&qh, surface, Layer::Top, Some("layer-shell-center-test"), None);
    layer_surface.set_size(WIDTH, HEIGHT);
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer_surface.commit();

    let mut state = LayerShellExample {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        shm,
        pool,
        layer_surface,
        first_configure: true,
        buffer: None,
        width: WIDTH,
        height: HEIGHT,
    };

    event_loop.run(Duration::from_millis(16), &mut state, |_state| {}).unwrap();
}

impl LayerShellHandler for LayerShellExample {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        std::process::exit(0);
    }

    fn configure(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let (new_w, new_h, redraw) = apply_layer_surface_configure(&configure, self.first_configure, (self.width, self.height));
        if redraw {
            self.first_configure = false;
            self.buffer = None;
            self.width = new_w;
            self.height = new_h;
            self.draw(conn, qh);
        } else {
            self.layer_surface.commit();
        }
    }
}

impl ProvidesRegistryState for LayerShellExample {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

impl CompositorHandler for LayerShellExample {
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

impl OutputHandler for LayerShellExample {
    fn output_state(&mut self) -> &mut OutputState {
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

impl ShmHandler for LayerShellExample {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_registry!(LayerShellExample);
delegate_compositor!(LayerShellExample);
delegate_output!(LayerShellExample);
delegate_shm!(LayerShellExample);
delegate_layer!(LayerShellExample);
