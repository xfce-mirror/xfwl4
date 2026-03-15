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

mod common;
mod maybe;
mod moving;
mod resize;

use std::{
    borrow::Cow,
    cell::RefCell,
    sync::{Arc, Mutex},
};

pub use maybe::*;
pub use moving::*;
pub use resize::*;
use smithay::{
    backend::input::TouchSlot,
    desktop::WindowSurface,
    input::{
        Seat,
        keyboard::GrabStartData as KeyboardGrabStartData,
        pointer::{Focus, GrabStartData as PointerGrabStartData},
        touch::GrabStartData as TouchGrabStartData,
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{Resource, protocol::wl_surface::WlSurface},
    },
    utils::{Logical, Point, Rectangle, Serial},
    wayland::{compositor::with_states, seat::WaylandFocus},
};

use crate::{
    backend::Backend,
    core::{
        cursor::CursorName,
        focus::{KeyboardFocusTarget, PointerFocusTarget},
        shell::{SurfaceData, WindowElement, WindowProps},
        state::Xfwl4State,
    },
};

use self::{moving::SharedMoveState, resize::SharedResizeState};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GrabTrigger {
    Pointer,
    Touch,
    Keyboard,
}

fn install_companion_keyboard_resize_grab<BackendData: Backend + 'static>(
    state: &mut Xfwl4State<BackendData>,
    seat: &Seat<Xfwl4State<BackendData>>,
    shared: Arc<Mutex<SharedResizeState>>,
    serial: Serial,
) {
    if let Some(keyboard) = seat.get_keyboard() {
        let start_data = keyboard.grab_start_data().unwrap_or_else(|| KeyboardGrabStartData {
            focus: keyboard.current_focus(),
        });
        let grab = KeyboardResizeSurfaceGrab { start_data, state: shared };
        keyboard.set_grab(state, grab, serial);
    }
}

fn install_companion_touch_resize_grab<BackendData: Backend + 'static>(
    state: &mut Xfwl4State<BackendData>,
    seat: &Seat<Xfwl4State<BackendData>>,
    shared: Arc<Mutex<SharedResizeState>>,
    serial: Serial,
) {
    if let Some(touch) = seat.get_touch() {
        let start_data = TouchGrabStartData {
            focus: None,
            slot: TouchSlot::from(None::<u32>),
            location: (0.0, 0.0).into(),
        };
        let grab = TouchResizeSurfaceGrab { start_data, state: shared };
        touch.set_grab(state, grab, serial);
    }
}

fn install_companion_pointer_resize_grab<BackendData: Backend + 'static>(
    state: &mut Xfwl4State<BackendData>,
    shared: Arc<Mutex<SharedResizeState>>,
    serial: Serial,
) {
    let pointer = state.core.pointer.clone();
    let location = pointer.current_location();
    let start_data = PointerGrabStartData {
        focus: None,
        button: 0,
        location,
    };
    let grab = PointerResizeSurfaceGrab { start_data, state: shared };
    pointer.set_grab(state, grab, serial, Focus::Clear);
}

fn install_companion_keyboard_move_grab<BackendData: Backend + 'static>(
    state: &mut Xfwl4State<BackendData>,
    seat: &Seat<Xfwl4State<BackendData>>,
    shared: Arc<Mutex<SharedMoveState>>,
    serial: Serial,
) {
    if let Some(keyboard) = seat.get_keyboard() {
        let start_data = keyboard.grab_start_data().unwrap_or_else(|| KeyboardGrabStartData {
            focus: keyboard.current_focus(),
        });
        let grab = KeyboardMoveSurfaceGrab { start_data, state: shared };
        keyboard.set_grab(state, grab, serial);
    }
}

fn install_companion_touch_move_grab<BackendData: Backend + 'static>(
    state: &mut Xfwl4State<BackendData>,
    seat: &Seat<Xfwl4State<BackendData>>,
    shared: Arc<Mutex<SharedMoveState>>,
    serial: Serial,
) {
    if let Some(touch) = seat.get_touch() {
        let start_data = TouchGrabStartData {
            focus: None,
            slot: TouchSlot::from(None::<u32>),
            location: (0.0, 0.0).into(),
        };
        let grab = TouchMoveSurfaceGrab { start_data, state: shared };
        touch.set_grab(state, grab, serial);
    }
}

fn install_companion_pointer_move_grab<BackendData: Backend + 'static>(
    state: &mut Xfwl4State<BackendData>,
    shared: Arc<Mutex<SharedMoveState>>,
    serial: Serial,
) {
    let pointer = state.core.pointer.clone();
    let location = pointer.current_location();
    let start_data = PointerGrabStartData {
        focus: None,
        button: 0,
        location,
    };
    let grab = PointerMoveSurfaceGrab { start_data, state: shared };
    pointer.set_grab(state, grab, serial, Focus::Clear);
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    fn unmaximize_for_move(&mut self, window: &WindowElement, start_location: Point<f64, Logical>) {
        let workspace = self.core.workspace_manager.active_workspace_mut();
        if let Some(maximized_geom) = workspace.element_geometry(window)
            && let Some(unmaximized_geom) = window
                .0
                .user_data()
                .get_or_insert(WindowProps::default)
                .0
                .lock()
                .unwrap()
                .pre_maximize_geom
                .take()
        {
            let x_frac = maximized_geom.size.w as f64 / (start_location.x - maximized_geom.loc.x as f64);
            let new_geom = Rectangle::new(
                Point::new(
                    maximized_geom.loc.x as f64 + unmaximized_geom.size.w as f64 * x_frac,
                    maximized_geom.loc.y as f64,
                )
                .to_i32_round(),
                unmaximized_geom.size,
            );

            match window.0.underlying_surface() {
                WindowSurface::Wayland(surface) => {
                    surface.with_pending_state(|state| {
                        state.states.unset(xdg_toplevel::State::Maximized);
                        state.size = None;
                    });

                    self.core
                        .workspace_manager
                        .active_workspace_mut()
                        .map_element(window.clone(), new_geom.loc, false);

                    if surface.is_initial_configure_sent() {
                        surface.send_configure();
                    }
                }

                #[cfg(feature = "xwayland")]
                WindowSurface::X11(surface) => {
                    let _ = surface.set_maximized(false);
                    let _ = surface.configure(new_geom);
                    workspace.map_element(window.clone(), new_geom.loc, false);
                }
            }
        }
    }

    fn start_window_move_pre(&mut self, window: &WindowElement, start_location: Point<f64, Logical>) {
        self.unmaximize_for_move(window, start_location);
        window.set_moving_state(true);
        self.core.set_cursor(CursorName::Fleur);
    }

    pub(in crate::core) fn start_maybe_window_move(
        &mut self,
        window: WindowElement,
        seat: Seat<Self>,
        serial: Serial,
        trigger: GrabTrigger,
        grab_start_data: Option<PointerGrabStartData<Xfwl4State<BackendData>>>,
    ) {
        if let Some(initial_window_location) = self.core.workspace_manager.active_workspace().element_location(&window) {
            match trigger {
                GrabTrigger::Pointer => {
                    if let Some(pointer) = seat.get_pointer()
                        && let Some(start_data) = grab_start_data.or_else(|| pointer.grab_start_data())
                        && check_move_resize_focus_ownership_pointer(&start_data.focus, window.wl_surface())
                    {
                        let seat_clone = seat.clone();
                        let grab = MaybeGrab::new_pointer(
                            move |state, start_data| {
                                state.start_window_move_pre(&window, start_data.location);
                                let pointer_location = start_data.location;
                                let shared = Arc::new(Mutex::new(SharedMoveState {
                                    window: window.clone(),
                                    initial_window_location,
                                    pointer_start_location: pointer_location,
                                    pointer_start_window_location: initial_window_location,
                                    button_pressed: true,
                                    finished: false,
                                    skip_next_pointer_motion: false,
                                }));
                                install_companion_keyboard_move_grab(state, &seat_clone, shared.clone(), serial);
                                install_companion_touch_move_grab(state, &seat_clone, shared.clone(), serial);
                                let grab = PointerMoveSurfaceGrab { start_data, state: shared };
                                (grab, Focus::Clear)
                            },
                            start_data,
                            seat.clone(),
                            Some(serial),
                        );
                        pointer.set_grab(self, grab, serial, Focus::Keep);
                    }
                }

                GrabTrigger::Touch => {
                    if let Some(touch) = seat.get_touch()
                        && let Some(start_data) = touch.grab_start_data()
                        && check_move_resize_focus_ownership_pointer(&start_data.focus, window.wl_surface())
                    {
                        let seat_clone = seat.clone();
                        let grab = MaybeGrab::new_touch(
                            move |state, start_data| {
                                state.start_window_move_pre(&window, start_data.location);
                                let pointer_location = start_data.location;
                                let shared = Arc::new(Mutex::new(SharedMoveState {
                                    window: window.clone(),
                                    initial_window_location,
                                    pointer_start_location: pointer_location,
                                    pointer_start_window_location: initial_window_location,
                                    button_pressed: false,
                                    finished: false,
                                    skip_next_pointer_motion: false,
                                }));
                                install_companion_keyboard_move_grab(state, &seat_clone, shared.clone(), serial);
                                install_companion_pointer_move_grab(state, shared.clone(), serial);
                                let grab = TouchMoveSurfaceGrab { start_data, state: shared };
                                (grab, Focus::Clear)
                            },
                            start_data,
                            seat.clone(),
                            Some(serial),
                        );
                        touch.set_grab(self, grab, serial);
                    }
                }

                GrabTrigger::Keyboard => {
                    if let Some(keyboard) = seat.get_keyboard() {
                        let start_data = keyboard.grab_start_data().unwrap_or_else(|| KeyboardGrabStartData {
                            focus: keyboard.current_focus(),
                        });
                        if check_move_resize_focus_ownership_keyboard(&start_data.focus, window.wl_surface()) {
                            let pointer_location = self.core.pointer.current_location();
                            self.start_window_move_pre(&window, pointer_location);
                            let shared = Arc::new(Mutex::new(SharedMoveState {
                                window: window.clone(),
                                initial_window_location,
                                pointer_start_location: pointer_location,
                                pointer_start_window_location: initial_window_location,
                                button_pressed: false,
                                finished: false,
                                skip_next_pointer_motion: true,
                            }));
                            install_companion_pointer_move_grab(self, shared.clone(), serial);
                            install_companion_touch_move_grab(self, &seat, shared.clone(), serial);
                            let warp_target = moving::warp_pointer_to_window_center(self, &window, initial_window_location);
                            shared.lock().unwrap().pointer_start_location = warp_target;
                            let grab = KeyboardMoveSurfaceGrab { start_data, state: shared };
                            keyboard.set_grab(self, grab, serial);
                        }
                    }
                }
            }
        }
    }

    pub(in crate::core) fn start_window_move(&mut self, window: WindowElement, seat: Seat<Self>, serial: Serial, trigger: GrabTrigger) {
        if let Some(initial_window_location) = self.core.workspace_manager.active_workspace().element_location(&window) {
            match trigger {
                GrabTrigger::Pointer => {
                    if let Some(pointer) = seat.get_pointer()
                        && let Some(start_data) = pointer.grab_start_data()
                        && check_move_resize_focus_ownership_pointer(&start_data.focus, window.wl_surface())
                    {
                        let pointer_location = start_data.location;
                        self.start_window_move_pre(&window, pointer_location);
                        let shared = Arc::new(Mutex::new(SharedMoveState {
                            window: window.clone(),
                            initial_window_location,
                            pointer_start_location: pointer_location,
                            pointer_start_window_location: initial_window_location,
                            button_pressed: true,
                            finished: false,
                            skip_next_pointer_motion: false,
                        }));
                        install_companion_keyboard_move_grab(self, &seat, shared.clone(), serial);
                        install_companion_touch_move_grab(self, &seat, shared.clone(), serial);
                        let grab = PointerMoveSurfaceGrab { start_data, state: shared };
                        pointer.set_grab(self, grab, serial, Focus::Clear);
                    }
                }

                GrabTrigger::Touch => {
                    if let Some(touch) = seat.get_touch()
                        && let Some(start_data) = touch.grab_start_data()
                        && check_move_resize_focus_ownership_pointer(&start_data.focus, window.wl_surface())
                    {
                        let pointer_location = start_data.location;
                        self.start_window_move_pre(&window, pointer_location);
                        let shared = Arc::new(Mutex::new(SharedMoveState {
                            window: window.clone(),
                            initial_window_location,
                            pointer_start_location: pointer_location,
                            pointer_start_window_location: initial_window_location,
                            button_pressed: false,
                            finished: false,
                            skip_next_pointer_motion: false,
                        }));
                        install_companion_keyboard_move_grab(self, &seat, shared.clone(), serial);
                        install_companion_pointer_move_grab(self, shared.clone(), serial);
                        let grab = TouchMoveSurfaceGrab { start_data, state: shared };
                        touch.set_grab(self, grab, serial);
                    }
                }

                GrabTrigger::Keyboard => {
                    if let Some(keyboard) = seat.get_keyboard() {
                        let start_data = keyboard.grab_start_data().unwrap_or_else(|| KeyboardGrabStartData {
                            focus: keyboard.current_focus(),
                        });
                        if check_move_resize_focus_ownership_keyboard(&start_data.focus, window.wl_surface()) {
                            let pointer_location = self.core.pointer.current_location();
                            self.start_window_move_pre(&window, pointer_location);
                            let shared = Arc::new(Mutex::new(SharedMoveState {
                                window: window.clone(),
                                initial_window_location,
                                pointer_start_location: pointer_location,
                                pointer_start_window_location: initial_window_location,
                                button_pressed: false,
                                finished: false,
                                skip_next_pointer_motion: true,
                            }));
                            install_companion_pointer_move_grab(self, shared.clone(), serial);
                            install_companion_touch_move_grab(self, &seat, shared.clone(), serial);
                            let warp_target = moving::warp_pointer_to_window_center(self, &window, initial_window_location);
                            shared.lock().unwrap().pointer_start_location = warp_target;
                            let grab = KeyboardMoveSurfaceGrab { start_data, state: shared };
                            keyboard.set_grab(self, grab, serial);
                        }
                    }
                }
            }
        }
    }

    fn start_window_resize_pre(&mut self, window: &WindowElement, wl_surface: &WlSurface, new_resize_state: ResizeState) {
        match window.0.underlying_surface() {
            WindowSurface::Wayland(surface) => {
                surface.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Maximized);
                });

                if surface.is_initial_configure_sent() {
                    surface.send_configure();
                }
            }

            #[cfg(feature = "xwayland")]
            WindowSurface::X11(surface) => {
                let _ = surface.set_maximized(false);
            }
        }

        window.set_resizing_state(true);

        with_states(wl_surface, move |states| {
            states.data_map.get::<RefCell<SurfaceData>>().unwrap().borrow_mut().resize_state = new_resize_state;
        });
    }

    pub(in crate::core) fn start_maybe_window_resize(
        &mut self,
        window: WindowElement,
        seat: Seat<Self>,
        serial: Serial,
        edges: ResizeEdge,
        trigger: GrabTrigger,
    ) {
        if let Some(mut initial_window_geom) = self.core.workspace_manager.active_workspace().element_geometry(&window)
            && let Some(wl_surface) = window.wl_surface()
        {
            if let Some(window_decorations) = window.decoration_state().window_decorations() {
                initial_window_geom.loc += window_decorations.decorations_offset();
                initial_window_geom.size.w -= window_decorations.left_decoration_width() + window_decorations.right_decoration_width();
                initial_window_geom.size.h -= window_decorations.top_decoration_height() + window_decorations.bottom_decoration_height();
            }

            let new_resize_state = ResizeState::Resizing(ResizeData {
                edges,
                initial_window_location: initial_window_geom.loc,
                initial_window_size: initial_window_geom.size,
                warp_pointer: matches!(trigger, GrabTrigger::Keyboard),
                warp_in_progress: false,
            });

            match trigger {
                GrabTrigger::Pointer => {
                    if let Some(pointer) = seat.get_pointer()
                        && let Some(start_data) = pointer.grab_start_data()
                        && check_move_resize_focus_ownership_pointer(&start_data.focus, window.wl_surface())
                    {
                        let wl_surface = wl_surface.into_owned();
                        let seat_clone = seat.clone();
                        let grab = MaybeGrab::new_pointer(
                            move |state, start_data| {
                                state.start_window_resize_pre(&window, &wl_surface, new_resize_state);
                                let pointer_location = start_data.location;
                                let shared = Arc::new(Mutex::new(SharedResizeState {
                                    window: window.clone(),
                                    edges,
                                    initial_window_location: initial_window_geom.loc,
                                    initial_window_size: initial_window_geom.size,
                                    last_window_size: initial_window_geom.size,
                                    pointer_start_location: pointer_location,
                                    pointer_start_size: initial_window_geom.size,
                                    button_pressed: true,
                                    finished: false,
                                    skip_next_pointer_motion: false,
                                }));
                                install_companion_keyboard_resize_grab(state, &seat_clone, shared.clone(), serial);
                                install_companion_touch_resize_grab(state, &seat_clone, shared.clone(), serial);
                                let grab = PointerResizeSurfaceGrab { start_data, state: shared };
                                (grab, Focus::Clear)
                            },
                            start_data,
                            seat.clone(),
                            Some(serial),
                        );
                        pointer.set_grab(self, grab, serial, Focus::Keep);
                    }
                }

                GrabTrigger::Touch => {
                    if let Some(touch) = seat.get_touch()
                        && let Some(start_data) = touch.grab_start_data()
                        && check_move_resize_focus_ownership_pointer(&start_data.focus, window.wl_surface())
                    {
                        let wl_surface = wl_surface.into_owned();
                        let seat_clone = seat.clone();
                        let grab = MaybeGrab::new_touch(
                            move |state, start_data| {
                                state.start_window_resize_pre(&window, &wl_surface, new_resize_state);
                                let pointer_location = start_data.location;
                                let shared = Arc::new(Mutex::new(SharedResizeState {
                                    window: window.clone(),
                                    edges,
                                    initial_window_location: initial_window_geom.loc,
                                    initial_window_size: initial_window_geom.size,
                                    last_window_size: initial_window_geom.size,
                                    pointer_start_location: pointer_location,
                                    pointer_start_size: initial_window_geom.size,
                                    button_pressed: false,
                                    finished: false,
                                    skip_next_pointer_motion: false,
                                }));
                                install_companion_keyboard_resize_grab(state, &seat_clone, shared.clone(), serial);
                                install_companion_pointer_resize_grab(state, shared.clone(), serial);
                                let grab = TouchResizeSurfaceGrab { start_data, state: shared };
                                (grab, Focus::Clear)
                            },
                            start_data,
                            seat.clone(),
                            Some(serial),
                        );
                        touch.set_grab(self, grab, serial);
                    }
                }

                GrabTrigger::Keyboard => {
                    if let Some(keyboard) = seat.get_keyboard() {
                        let start_data = keyboard.grab_start_data().unwrap_or_else(|| KeyboardGrabStartData {
                            focus: keyboard.current_focus(),
                        });
                        if check_move_resize_focus_ownership_keyboard(&start_data.focus, window.wl_surface()) {
                            let wl_surface = wl_surface.into_owned();
                            let pointer_location = self.core.pointer.current_location();
                            self.start_window_resize_pre(&window, &wl_surface, new_resize_state);
                            let shared = Arc::new(Mutex::new(SharedResizeState {
                                window: window.clone(),
                                edges: ResizeEdge::BOTTOM_RIGHT,
                                initial_window_location: initial_window_geom.loc,
                                initial_window_size: initial_window_geom.size,
                                last_window_size: initial_window_geom.size,
                                pointer_start_location: pointer_location,
                                pointer_start_size: initial_window_geom.size,
                                button_pressed: false,
                                finished: false,
                                skip_next_pointer_motion: false,
                            }));
                            install_companion_pointer_resize_grab(self, shared.clone(), serial);
                            install_companion_touch_resize_grab(self, &seat, shared.clone(), serial);
                            let grab = KeyboardResizeSurfaceGrab { start_data, state: shared };
                            keyboard.set_grab(self, grab, serial);
                        }
                    }
                }
            }
        }
    }

    pub(in crate::core) fn start_window_resize(
        &mut self,
        window: WindowElement,
        seat: Seat<Self>,
        serial: Serial,
        edges: ResizeEdge,
        trigger: GrabTrigger,
    ) {
        if let Some(mut initial_window_geom) = self.core.workspace_manager.active_workspace().element_geometry(&window)
            && let Some(wl_surface) = window.wl_surface()
        {
            if let Some(window_decorations) = window.decoration_state().window_decorations() {
                initial_window_geom.loc += window_decorations.decorations_offset();
                initial_window_geom.size.w -= window_decorations.left_decoration_width() + window_decorations.right_decoration_width();
                initial_window_geom.size.h -= window_decorations.top_decoration_height() + window_decorations.bottom_decoration_height();
            }

            let new_resize_state = ResizeState::Resizing(ResizeData {
                edges,
                initial_window_location: initial_window_geom.loc,
                initial_window_size: initial_window_geom.size,
                warp_pointer: matches!(trigger, GrabTrigger::Keyboard),
                warp_in_progress: false,
            });

            match trigger {
                GrabTrigger::Pointer => {
                    if let Some(pointer) = seat.get_pointer()
                        && let Some(start_data) = pointer.grab_start_data()
                        && check_move_resize_focus_ownership_pointer(&start_data.focus, window.wl_surface())
                    {
                        let wl_surface = wl_surface.into_owned();
                        let pointer_location = start_data.location;
                        self.start_window_resize_pre(&window, &wl_surface, new_resize_state);
                        let shared = Arc::new(Mutex::new(SharedResizeState {
                            window: window.clone(),
                            edges,
                            initial_window_location: initial_window_geom.loc,
                            initial_window_size: initial_window_geom.size,
                            last_window_size: initial_window_geom.size,
                            pointer_start_location: pointer_location,
                            pointer_start_size: initial_window_geom.size,
                            button_pressed: true,
                            finished: false,
                            skip_next_pointer_motion: false,
                        }));
                        install_companion_keyboard_resize_grab(self, &seat, shared.clone(), serial);
                        install_companion_touch_resize_grab(self, &seat, shared.clone(), serial);
                        let grab = PointerResizeSurfaceGrab { start_data, state: shared };
                        pointer.set_grab(self, grab, serial, Focus::Keep);
                    }
                }

                GrabTrigger::Touch => {
                    if let Some(touch) = seat.get_touch()
                        && let Some(start_data) = touch.grab_start_data()
                        && check_move_resize_focus_ownership_pointer(&start_data.focus, window.wl_surface())
                    {
                        let wl_surface = wl_surface.into_owned();
                        let pointer_location = start_data.location;
                        self.start_window_resize_pre(&window, &wl_surface, new_resize_state);
                        let shared = Arc::new(Mutex::new(SharedResizeState {
                            window: window.clone(),
                            edges,
                            initial_window_location: initial_window_geom.loc,
                            initial_window_size: initial_window_geom.size,
                            last_window_size: initial_window_geom.size,
                            pointer_start_location: pointer_location,
                            pointer_start_size: initial_window_geom.size,
                            button_pressed: false,
                            finished: false,
                            skip_next_pointer_motion: false,
                        }));
                        install_companion_keyboard_resize_grab(self, &seat, shared.clone(), serial);
                        install_companion_pointer_resize_grab(self, shared.clone(), serial);
                        let grab = TouchResizeSurfaceGrab { start_data, state: shared };
                        touch.set_grab(self, grab, serial);
                    }
                }

                GrabTrigger::Keyboard => {
                    if let Some(keyboard) = seat.get_keyboard() {
                        let start_data = keyboard.grab_start_data().unwrap_or_else(|| KeyboardGrabStartData {
                            focus: keyboard.current_focus(),
                        });
                        if check_move_resize_focus_ownership_keyboard(&start_data.focus, window.wl_surface()) {
                            let wl_surface = wl_surface.into_owned();
                            let pointer_location = self.core.pointer.current_location();
                            self.start_window_resize_pre(&window, &wl_surface, new_resize_state);
                            let shared = Arc::new(Mutex::new(SharedResizeState {
                                window: window.clone(),
                                edges: ResizeEdge::BOTTOM_RIGHT,
                                initial_window_location: initial_window_geom.loc,
                                initial_window_size: initial_window_geom.size,
                                last_window_size: initial_window_geom.size,
                                pointer_start_location: pointer_location,
                                pointer_start_size: initial_window_geom.size,
                                button_pressed: false,
                                finished: false,
                                skip_next_pointer_motion: false,
                            }));
                            install_companion_pointer_resize_grab(self, shared.clone(), serial);
                            install_companion_touch_resize_grab(self, &seat, shared.clone(), serial);
                            let grab = KeyboardResizeSurfaceGrab { start_data, state: shared };
                            keyboard.set_grab(self, grab, serial);
                        }
                    }
                }
            }
        }
    }
}

fn check_move_resize_focus_ownership_pointer(
    focus: &Option<(PointerFocusTarget, Point<f64, Logical>)>,
    owner: Option<Cow<'_, WlSurface>>,
) -> bool {
    match (focus, owner) {
        (Some((focus, _)), Some(owner)) => focus.same_client_as(&owner.id()),
        _ => false,
    }
}

fn check_move_resize_focus_ownership_keyboard(focus: &Option<KeyboardFocusTarget>, owner: Option<Cow<'_, WlSurface>>) -> bool {
    match (focus, owner) {
        (Some(focus), Some(owner)) => focus.same_client_as(&owner.id()),
        _ => false,
    }
}
