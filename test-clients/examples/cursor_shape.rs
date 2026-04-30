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
    delegate_compositor, delegate_output, delegate_pointer, delegate_registry, delegate_seat, delegate_shm, delegate_xdg_shell,
    delegate_xdg_window,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::EventLoop,
        calloop_wayland_source::WaylandSource,
        client::{
            Connection, Dispatch, Proxy, QueueHandle,
            globals::registry_queue_init,
            protocol::{wl_output::WlOutput, wl_pointer::WlPointer, wl_seat::WlSeat, wl_shm, wl_surface::WlSurface},
        },
        protocols::wp::cursor_shape::v1::client::{
            wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1},
            wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
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

struct WindowInfo {
    window: Window,
    label: &'static str,
    color: [u8; 4],
    shape: Shape,
    buffer: Option<Buffer>,
    width: u32,
    height: u32,
    first_configure: bool,
}

struct State {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    shm: Shm,
    pool: SlotPool,
    cursor_shape_manager: WpCursorShapeManagerV1,
    pointer: Option<(WlPointer, WpCursorShapeDeviceV1)>,
    windows: Vec<WindowInfo>,
}

impl State {
    fn find_window_mut(&mut self, surface: &WlSurface) -> Option<&mut WindowInfo> {
        self.windows.iter_mut().find(|w| w.window.wl_surface() == surface)
    }

    fn draw_window(&mut self, surface: &WlSurface, qh: &QueueHandle<Self>) {
        let info = self.windows.iter_mut().find(|w| w.window.wl_surface() == surface).unwrap();

        let buffer = info.buffer.get_or_insert_with(|| {
            self.pool
                .create_buffer(
                    info.width as i32,
                    info.height as i32,
                    info.width as i32 * 4,
                    wl_shm::Format::Argb8888,
                )
                .unwrap()
                .0
        });

        let canvas = match self.pool.canvas(buffer) {
            Some(canvas) => canvas,
            None => {
                let (backup_buffer, canvas) = self
                    .pool
                    .create_buffer(
                        info.width as i32,
                        info.height as i32,
                        info.width as i32 * 4,
                        wl_shm::Format::Argb8888,
                    )
                    .unwrap();
                *buffer = backup_buffer;
                canvas
            }
        };

        for pixel in canvas.chunks_exact_mut(4) {
            pixel[0] = info.color[0];
            pixel[1] = info.color[1];
            pixel[2] = info.color[2];
            pixel[3] = info.color[3];
        }

        let surface = info.window.wl_surface();
        surface.damage_buffer(0, 0, info.width as i32, info.height as i32);
        surface.frame(qh, surface.clone());

        buffer.attach_to(surface).unwrap();
        info.window.commit();
    }
}

fn create_window(
    compositor: &CompositorState,
    xdg_shell: &XdgShell,
    qh: &QueueHandle<State>,
    label: &'static str,
    color: [u8; 4],
    shape: Shape,
) -> WindowInfo {
    let surface = compositor.create_surface(qh);
    let window = xdg_shell.create_window(surface, WindowDecorations::RequestServer, qh);
    window.set_title(label);
    window.set_app_id("org.xfce.xfwl4.cursor-shape-test");
    window.set_min_size(Some((100, 100)));
    window.commit();

    WindowInfo {
        window,
        label,
        color,
        shape,
        buffer: None,
        width: 240,
        height: 160,
        first_configure: true,
    }
}

fn main() {
    let conn = Connection::connect_to_env().unwrap();
    let (globals, event_queue) = registry_queue_init(&conn).unwrap();
    let qh = event_queue.handle();
    let mut event_loop = EventLoop::<State>::try_new().unwrap();
    let loop_handle = event_loop.handle();
    WaylandSource::new(conn.clone(), event_queue).insert(loop_handle).unwrap();

    let compositor = CompositorState::bind(&globals, &qh).unwrap();
    let shm = Shm::bind(&globals, &qh).unwrap();
    let xdg_shell = XdgShell::bind(&globals, &qh).unwrap();
    let cursor_shape_manager = globals
        .bind::<WpCursorShapeManagerV1, State, _>(&qh, 1..=2, ())
        .expect("compositor does not support wp_cursor_shape_manager_v1");

    // BGRA colors
    let red = [0x00, 0x00, 0xcc, 0xff];
    let green = [0x00, 0xcc, 0x00, 0xff];

    let win_a = create_window(&compositor, &xdg_shell, &qh, "Crosshair", red, Shape::Crosshair);
    let win_b = create_window(&compositor, &xdg_shell, &qh, "Zoom-In", green, Shape::ZoomIn);

    let pool_size = 2 * 240 * 160 * 4;
    let pool = SlotPool::new(pool_size, &shm).unwrap();

    let mut state = State {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        shm,
        pool,
        cursor_shape_manager,
        pointer: None,
        windows: vec![win_a, win_b],
    };

    eprintln!("Hover over each window to see its assigned cursor shape:");
    eprintln!("  Crosshair (red)   - Shape::Crosshair");
    eprintln!("  Zoom-In   (green) - Shape::ZoomIn");

    event_loop.run(Duration::from_millis(16), &mut state, |_state| {}).unwrap();
}

impl Dispatch<WpCursorShapeManagerV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WpCursorShapeManagerV1,
        _event: <WpCursorShapeManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpCursorShapeDeviceV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WpCursorShapeDeviceV1,
        _event: <WpCursorShapeDeviceV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

impl CompositorHandler for State {
    fn frame(&mut self, _conn: &Connection, qh: &QueueHandle<Self>, surface: &WlSurface, _time: u32) {
        self.draw_window(surface, qh);
    }

    fn surface_enter(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _surface: &WlSurface, _output: &WlOutput) {}

    fn surface_leave(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _surface: &WlSurface, _output: &WlOutput) {}

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _new_transform: smithay_client_toolkit::reexports::client::protocol::wl_output::Transform,
    ) {
    }

    fn scale_factor_changed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _surface: &WlSurface, _new_factor: i32) {}
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}

    fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}

    fn output_destroyed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {}

    fn new_capability(&mut self, _conn: &Connection, qh: &QueueHandle<Self>, seat: WlSeat, capability: Capability) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            let pointer = self.seat_state.get_pointer(qh, &seat).expect("Failed to create pointer");
            let device = self.cursor_shape_manager.get_pointer(&pointer, qh, ());
            self.pointer = Some((pointer, device));
        }
    }

    fn remove_capability(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat, capability: Capability) {
        if capability == Capability::Pointer
            && let Some((pointer, device)) = self.pointer.take()
        {
            device.destroy();
            pointer.release();
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {}
}

impl PointerHandler for State {
    fn pointer_frame(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _pointer: &WlPointer, events: &[PointerEvent]) {
        for event in events {
            if let PointerEventKind::Enter { serial } = event.kind
                && let Some((_, device)) = self.pointer.as_ref()
                && let Some(info) = self.windows.iter().find(|w| w.window.wl_surface() == &event.surface)
            {
                device.set_shape(serial, info.shape);
                eprintln!("Set cursor shape on {} to {:?}", info.label, info.shape);
            }
        }
    }
}

impl WindowHandler for State {
    fn configure(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        window: &Window,
        configure: smithay_client_toolkit::shell::xdg::window::WindowConfigure,
        _serial: u32,
    ) {
        let surface = window.wl_surface().clone();
        let info = self.find_window_mut(&surface).unwrap();

        let new_width = configure.new_size.0.map(|w| w.get()).unwrap_or(info.width);
        let new_height = configure.new_size.1.map(|h| h.get()).unwrap_or(info.height);

        if info.first_configure || new_width != info.width || new_height != info.height {
            info.first_configure = false;
            info.buffer = None;
            info.width = new_width;
            info.height = new_height;
            self.draw_window(&surface, qh);
        } else {
            info.window.commit();
        }
    }

    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _window: &Window) {
        std::process::exit(0);
    }
}

delegate_registry!(State);
delegate_compositor!(State);
delegate_output!(State);
delegate_shm!(State);
delegate_seat!(State);
delegate_pointer!(State);
delegate_xdg_shell!(State);
delegate_xdg_window!(State);
