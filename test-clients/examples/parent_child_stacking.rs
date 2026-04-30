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
        client::{
            Connection, Dispatch, Proxy, QueueHandle,
            protocol::{wl_output::WlOutput, wl_surface::WlSurface},
        },
        protocols::xdg::dialog::v1::client::{xdg_dialog_v1::XdgDialogV1, xdg_wm_dialog_v1::XdgWmDialogV1},
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

struct WindowInfo {
    window: Window,
    label: &'static str,
    color: [u8; 4],
    buffer: Option<Buffer>,
    width: u32,
    height: u32,
    first_configure: bool,
}

struct State {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,
    windows: Vec<WindowInfo>,
}

impl State {
    fn find_window_mut(&mut self, surface: &WlSurface) -> Option<&mut WindowInfo> {
        self.windows.iter_mut().find(|w| w.window.wl_surface() == surface)
    }

    fn draw_window(&mut self, surface: &WlSurface, qh: &QueueHandle<Self>) {
        let info = self.windows.iter_mut().find(|w| w.window.wl_surface() == surface).unwrap();
        paint_solid(
            &mut self.pool,
            &mut info.buffer,
            info.window.wl_surface(),
            qh,
            info.width,
            info.height,
            info.color,
        );
        info.window.commit();
    }
}

fn create_window(
    compositor: &CompositorState,
    xdg_shell: &XdgShell,
    qh: &QueueHandle<State>,
    label: &'static str,
    color: [u8; 4],
    parent: Option<&Window>,
) -> WindowInfo {
    let surface = compositor.create_surface(qh);
    let window = xdg_shell.create_window(surface, WindowDecorations::RequestServer, qh);
    window.set_title(label);
    window.set_app_id("org.xfce.xfwl4.parent-child-stacking-test");
    window.set_min_size(Some((100, 100)));
    if let Some(parent) = parent {
        window.set_parent(Some(parent));
    }
    window.commit();

    WindowInfo {
        window,
        label,
        color,
        buffer: None,
        width: 200,
        height: 150,
        first_configure: true,
    }
}

fn main() {
    let (_conn, globals, qh, mut event_loop) = init_event_loop::<State>();

    let compositor = CompositorState::bind(&globals, &qh).unwrap();
    let shm = Shm::bind(&globals, &qh).unwrap();
    let xdg_shell = XdgShell::bind(&globals, &qh).unwrap();
    let dialog_manager = globals.bind::<XdgWmDialogV1, State, _>(&qh, 1..=1, ()).unwrap();

    // BGRA colors
    let red = [0x00, 0x00, 0xcc, 0xff];
    let green = [0x00, 0xcc, 0x00, 0xff];
    let blue = [0xcc, 0x00, 0x00, 0xff];
    let yellow = [0x00, 0xcc, 0xcc, 0xff];
    let magenta = [0xcc, 0x00, 0xcc, 0xff];
    let cyan = [0xcc, 0xcc, 0x00, 0xff];

    let win_a = create_window(&compositor, &xdg_shell, &qh, "A (parent)", red, None);
    let win_b = create_window(&compositor, &xdg_shell, &qh, "B (parent)", green, None);

    let win_a_child_modal = create_window(&compositor, &xdg_shell, &qh, "A-child-modal", blue, Some(&win_a.window));
    let a_modal_dialog = dialog_manager.get_xdg_dialog(win_a_child_modal.window.xdg_toplevel(), &qh, ());
    a_modal_dialog.set_modal();

    let win_a_child_plain = create_window(&compositor, &xdg_shell, &qh, "A-child-plain", yellow, Some(&win_a.window));

    let win_b_child_dialog = create_window(&compositor, &xdg_shell, &qh, "B-child-dialog", magenta, Some(&win_b.window));
    let _b_dialog = dialog_manager.get_xdg_dialog(win_b_child_dialog.window.xdg_toplevel(), &qh, ());

    let win_b_child_plain = create_window(&compositor, &xdg_shell, &qh, "B-child-plain", cyan, Some(&win_b.window));

    let pool_size = 6 * 200 * 150 * 4;
    let pool = SlotPool::new(pool_size, &shm).unwrap();

    let mut state = State {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        shm,
        pool,
        windows: vec![
            win_a,
            win_b,
            win_a_child_modal,
            win_a_child_plain,
            win_b_child_dialog,
            win_b_child_plain,
        ],
    };

    eprintln!("Windows created:");
    eprintln!("  A (parent)        - red");
    eprintln!("  B (parent)        - green");
    eprintln!("  A-child-modal     - blue   (child of A, xdg-dialog modal)");
    eprintln!("  A-child-plain     - yellow (child of A, no dialog hint)");
    eprintln!("  B-child-dialog    - magenta (child of B, xdg-dialog)");
    eprintln!("  B-child-plain     - cyan   (child of B, no dialog hint)");

    event_loop.run(Duration::from_millis(16), &mut state, |_state| {}).unwrap();
}

impl Dispatch<XdgWmDialogV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &XdgWmDialogV1,
        _event: <XdgWmDialogV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgDialogV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &XdgDialogV1,
        _event: <XdgDialogV1 as Proxy>::Event,
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
    registry_handlers![OutputState];
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
        let (new_w, new_h, redraw) = apply_window_configure(&configure, info.first_configure, (info.width, info.height), (200, 150));
        if redraw {
            info.first_configure = false;
            info.buffer = None;
            info.width = new_w;
            info.height = new_h;
            eprintln!("configure: {} ({}x{})", info.label, new_w, new_h);
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
delegate_xdg_shell!(State);
delegate_xdg_window!(State);
