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
mod tabwin;

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
    desktop::{WindowSurface, space::SpaceElement},
    input::{
        Seat,
        keyboard::GrabStartData as KeyboardGrabStartData,
        pointer::{CursorIcon, Focus, GrabStartData as PointerGrabStartData},
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
        drawing::wireframe::Wireframe,
        focus::{KeyboardFocusTarget, PointerFocusTarget},
        shell::{SurfaceData, WindowElement},
        state::Xfwl4State,
    },
};

use self::{moving::SharedMoveState, resize::SharedResizeState};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GrabTrigger {
    Pointer,
    Touch,
    Keyboard,
    /// A privileged, shell-initiated grab (e.g. foreign-toplevel management).  Behaves like
    /// `Keyboard`, but bypasses the focus-ownership check because it acts on windows the
    /// requesting client does not own.
    Shell,
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
    fn restore_window_for_move(&mut self, window: &WindowElement, start_location: Point<f64, Logical>) -> Option<Point<i32, Logical>> {
        let is_maximized = window.maximized();
        let is_tiled = window.tile_mode().is_some();

        if is_maximized || is_tiled {
            let workspace = self.core.workspace_manager.active_workspace_mut();
            if let Some(current_geom) = workspace.window_geometry(window)
                && let Some(saved_geom) = {
                    // Do this in a sub-block to avoid holding 'props' to long, causing a deadlock.
                    window.props().saved_geom
                }
            {
                let x_frac = (start_location.x - current_geom.loc.x as f64) / current_geom.size.w as f64;
                let new_loc = Point::new(start_location.x - saved_geom.size.w as f64 * x_frac, current_geom.loc.y as f64).to_i32_round();

                if is_maximized {
                    self.set_window_unmaximized(window, Some(new_loc));
                } else if is_tiled {
                    self.set_window_untiled(window, Some(new_loc));
                }

                Some(new_loc)
            } else {
                None
            }
        } else {
            None
        }
    }

    fn start_window_move_pre(
        &mut self,
        window: &WindowElement,
        initial_window_location: Point<i32, Logical>,
        start_location: Point<f64, Logical>,
    ) -> Point<i32, Logical> {
        let location = self
            .restore_window_for_move(window, start_location)
            .unwrap_or(initial_window_location);
        window.set_moving_state(true);
        self.core.set_cursor(CursorIcon::AllResize);

        if self.core.config.box_move() {
            let geom = Rectangle::new(location, window.geometry().size);
            self.core.wireframe = Some(Wireframe::new(None, geom, &self.core.config));
        }

        location
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::core) fn start_maybe_window_move(
        &mut self,
        window: WindowElement,
        seat: Seat<Self>,
        serial: Serial,
        trigger: GrabTrigger,
        grab_start_data: Option<PointerGrabStartData<Xfwl4State<BackendData>>>,
    ) {
        self.start_window_move_inner(window, seat, serial, trigger, true, grab_start_data);
    }

    pub(in crate::core) fn start_window_move(&mut self, window: WindowElement, seat: Seat<Self>, serial: Serial, trigger: GrabTrigger) {
        self.start_window_move_inner(window, seat, serial, trigger, false, None);
    }

    #[allow(clippy::too_many_arguments)]
    fn start_window_move_inner(
        &mut self,
        window: WindowElement,
        seat: Seat<Self>,
        serial: Serial,
        trigger: GrabTrigger,
        maybe: bool,
        grab_start_data: Option<PointerGrabStartData<Xfwl4State<BackendData>>>,
    ) {
        if let Some(initial_window_location) = self.core.workspace_manager.active_workspace().window_location(&window) {
            match trigger {
                GrabTrigger::Pointer => {
                    if let Some(pointer) = seat.get_pointer()
                        && let Some(start_data) = grab_start_data.or_else(|| pointer.grab_start_data())
                        && check_move_resize_focus_ownership_pointer(&start_data.focus, window.wl_surface())
                    {
                        let seat_clone = seat.clone();
                        let upgrade = move |state: &mut Xfwl4State<BackendData>, start_data: PointerGrabStartData<_>| {
                            let initial_window_location =
                                state.start_window_move_pre(&window, initial_window_location, start_data.location);
                            let shared = Arc::new(Mutex::new(SharedMoveState {
                                window: window.clone(),
                                initial_window_location,
                                pointer_start_location: start_data.location,
                                pointer_start_window_location: initial_window_location,
                                button_pressed: true,
                                finished: false,
                                skip_next_pointer_motion: false,
                            }));
                            state.core.active_move_grab = Some(shared.clone().into());
                            install_companion_keyboard_move_grab(state, &seat_clone, shared.clone(), serial);
                            install_companion_touch_move_grab(state, &seat_clone, shared.clone(), serial);
                            let grab = PointerMoveSurfaceGrab { start_data, state: shared };
                            (grab, Focus::Clear)
                        };

                        if maybe {
                            let grab = MaybeGrab::new_pointer(upgrade, start_data, seat.clone(), Some(serial));
                            pointer.set_grab(self, grab, serial, Focus::Keep);
                        } else {
                            let (grab, focus) = upgrade(self, start_data);
                            pointer.set_grab(self, grab, serial, focus);
                        }
                    }
                }

                GrabTrigger::Touch => {
                    if let Some(touch) = seat.get_touch()
                        && let Some(start_data) = touch.grab_start_data()
                        && check_move_resize_focus_ownership_pointer(&start_data.focus, window.wl_surface())
                    {
                        let seat_clone = seat.clone();
                        let upgrade = move |state: &mut Xfwl4State<BackendData>, start_data: TouchGrabStartData<_>| {
                            let initial_window_location =
                                state.start_window_move_pre(&window, initial_window_location, start_data.location);
                            let shared = Arc::new(Mutex::new(SharedMoveState {
                                window: window.clone(),
                                initial_window_location,
                                pointer_start_location: start_data.location,
                                pointer_start_window_location: initial_window_location,
                                button_pressed: false,
                                finished: false,
                                skip_next_pointer_motion: false,
                            }));
                            state.core.active_move_grab = Some(shared.clone().into());
                            install_companion_keyboard_move_grab(state, &seat_clone, shared.clone(), serial);
                            install_companion_pointer_move_grab(state, shared.clone(), serial);
                            let grab = TouchMoveSurfaceGrab { start_data, state: shared };
                            (grab, Focus::Clear)
                        };

                        if maybe {
                            let grab = MaybeGrab::new_touch(upgrade, start_data, seat.clone(), Some(serial));
                            touch.set_grab(self, grab, serial);
                        } else {
                            let (grab, _focus) = upgrade(self, start_data);
                            touch.set_grab(self, grab, serial);
                        }
                    }
                }

                GrabTrigger::Keyboard | GrabTrigger::Shell => {
                    if let Some(keyboard) = seat.get_keyboard() {
                        let start_data = keyboard.grab_start_data().unwrap_or_else(|| KeyboardGrabStartData {
                            focus: keyboard.current_focus(),
                        });
                        if trigger == GrabTrigger::Shell
                            || check_move_resize_focus_ownership_keyboard(&start_data.focus, window.wl_surface())
                        {
                            let pointer_location = self.core.pointer.current_location();
                            let initial_window_location = self.start_window_move_pre(&window, initial_window_location, pointer_location);
                            let shared = Arc::new(Mutex::new(SharedMoveState {
                                window: window.clone(),
                                initial_window_location,
                                pointer_start_location: pointer_location,
                                pointer_start_window_location: initial_window_location,
                                button_pressed: false,
                                finished: false,
                                skip_next_pointer_motion: true,
                            }));
                            self.core.active_move_grab = Some(shared.clone().into());
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

    fn start_window_resize_pre(
        &mut self,
        window: &WindowElement,
        wl_surface: &WlSurface,
        new_resize_state: ResizeState,
        full_element_geom: Rectangle<i32, Logical>,
    ) {
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

        if self.core.config.box_resize() {
            self.core.wireframe = Some(Wireframe::new(None, full_element_geom, &self.core.config));
        }

        with_states(wl_surface, move |states| {
            states.data_map.get::<RefCell<SurfaceData>>().unwrap().borrow_mut().resize_state = new_resize_state;
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::core) fn start_maybe_window_resize(
        &mut self,
        window: WindowElement,
        seat: Seat<Self>,
        serial: Serial,
        edges: ResizeEdge,
        trigger: GrabTrigger,
        grab_start_data: Option<PointerGrabStartData<Xfwl4State<BackendData>>>,
    ) {
        self.start_window_resize_inner(window, seat, serial, edges, trigger, true, grab_start_data);
    }

    pub(in crate::core) fn start_window_resize(
        &mut self,
        window: WindowElement,
        seat: Seat<Self>,
        serial: Serial,
        edges: ResizeEdge,
        trigger: GrabTrigger,
    ) {
        self.start_window_resize_inner(window, seat, serial, edges, trigger, false, None);
    }

    #[allow(clippy::too_many_arguments)]
    fn start_window_resize_inner(
        &mut self,
        window: WindowElement,
        seat: Seat<Self>,
        serial: Serial,
        edges: ResizeEdge,
        trigger: GrabTrigger,
        maybe: bool,
        grab_start_data: Option<PointerGrabStartData<Xfwl4State<BackendData>>>,
    ) {
        if let Some(full_element_geom) = self.core.workspace_manager.active_workspace().window_geometry(&window)
            && let Some(wl_surface) = window.wl_surface()
        {
            let mut initial_window_geom = full_element_geom;
            if let Some(window_decorations) = window.decoration_state().window_decorations() {
                initial_window_geom.loc += window_decorations.decorations_offset();
                let e = window_decorations.decorations_extents();
                initial_window_geom.size.w -= e.left + e.right;
                initial_window_geom.size.h -= e.top + e.bottom;
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
                        && let Some(start_data) = grab_start_data.or_else(|| pointer.grab_start_data())
                        && check_move_resize_focus_ownership_pointer(&start_data.focus, window.wl_surface())
                    {
                        let wl_surface = wl_surface.into_owned();
                        let seat_clone = seat.clone();
                        let upgrade = move |state: &mut Xfwl4State<BackendData>, start_data: PointerGrabStartData<_>| {
                            state.start_window_resize_pre(&window, &wl_surface, new_resize_state, full_element_geom);
                            let shared = Arc::new(Mutex::new(SharedResizeState {
                                window: window.clone(),
                                edges,
                                initial_window_location: initial_window_geom.loc,
                                initial_window_size: initial_window_geom.size,
                                last_window_size: initial_window_geom.size,
                                pointer_start_location: start_data.location,
                                pointer_start_size: initial_window_geom.size,
                                button_pressed: true,
                                finished: false,
                                skip_next_pointer_motion: false,
                            }));
                            install_companion_keyboard_resize_grab(state, &seat_clone, shared.clone(), serial);
                            install_companion_touch_resize_grab(state, &seat_clone, shared.clone(), serial);
                            let grab = PointerResizeSurfaceGrab { start_data, state: shared };
                            (grab, Focus::Clear)
                        };

                        if maybe {
                            let grab = MaybeGrab::new_pointer(upgrade, start_data, seat.clone(), Some(serial));
                            pointer.set_grab(self, grab, serial, Focus::Keep);
                        } else {
                            let (grab, _focus) = upgrade(self, start_data);
                            pointer.set_grab(self, grab, serial, Focus::Keep);
                        }
                    }
                }

                GrabTrigger::Touch => {
                    if let Some(touch) = seat.get_touch()
                        && let Some(start_data) = touch.grab_start_data()
                        && check_move_resize_focus_ownership_pointer(&start_data.focus, window.wl_surface())
                    {
                        let wl_surface = wl_surface.into_owned();
                        let seat_clone = seat.clone();
                        let upgrade = move |state: &mut Xfwl4State<BackendData>, start_data: TouchGrabStartData<_>| {
                            state.start_window_resize_pre(&window, &wl_surface, new_resize_state, full_element_geom);
                            let shared = Arc::new(Mutex::new(SharedResizeState {
                                window: window.clone(),
                                edges,
                                initial_window_location: initial_window_geom.loc,
                                initial_window_size: initial_window_geom.size,
                                last_window_size: initial_window_geom.size,
                                pointer_start_location: start_data.location,
                                pointer_start_size: initial_window_geom.size,
                                button_pressed: false,
                                finished: false,
                                skip_next_pointer_motion: false,
                            }));
                            install_companion_keyboard_resize_grab(state, &seat_clone, shared.clone(), serial);
                            install_companion_pointer_resize_grab(state, shared.clone(), serial);
                            let grab = TouchResizeSurfaceGrab { start_data, state: shared };
                            (grab, Focus::Clear)
                        };

                        if maybe {
                            let grab = MaybeGrab::new_touch(upgrade, start_data, seat.clone(), Some(serial));
                            touch.set_grab(self, grab, serial);
                        } else {
                            let (grab, _focus) = upgrade(self, start_data);
                            touch.set_grab(self, grab, serial);
                        }
                    }
                }

                GrabTrigger::Keyboard | GrabTrigger::Shell => {
                    if let Some(keyboard) = seat.get_keyboard() {
                        let start_data = keyboard.grab_start_data().unwrap_or_else(|| KeyboardGrabStartData {
                            focus: keyboard.current_focus(),
                        });
                        if trigger == GrabTrigger::Shell
                            || check_move_resize_focus_ownership_keyboard(&start_data.focus, window.wl_surface())
                        {
                            let wl_surface = wl_surface.into_owned();
                            let pointer_location = self.core.pointer.current_location();
                            self.start_window_resize_pre(&window, &wl_surface, new_resize_state, full_element_geom);
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
                            let grab = KeyboardResizeSurfaceGrab {
                                start_data,
                                state: shared.clone(),
                            };
                            keyboard.set_grab(self, grab, serial);

                            shared.lock().unwrap().skip_next_pointer_motion = true;
                            let element_loc = self
                                .core
                                .workspace_manager
                                .active_workspace()
                                .window_location(&window)
                                .unwrap_or_default();
                            self.warp_pointer_to_resize_edge(&window, element_loc, edges);
                            let mut state = shared.lock().unwrap();
                            state.pointer_start_location = self.core.pointer.current_location();
                            state.pointer_start_size = state.last_window_size;
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
