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

use std::{borrow::Cow, cell::RefCell};

pub use maybe::*;
pub use moving::*;
pub use resize::*;
use smithay::{
    desktop::WindowSurface,
    input::{
        Seat,
        keyboard::{GrabStartData as KeyboardGrabStartData, KeyboardHandle},
        pointer::{Focus, GrabStartData as PointerGrabStartData, PointerHandle},
        touch::{GrabStartData as TouchGrabStartData, TouchHandle},
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GrabTrigger {
    Pointer,
    Touch,
    Keyboard,
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    fn unmaximize_for_move(&mut self, window: &WindowElement, start_location: Point<f64, Logical>) {
        // If the window is maximized, we want to unmaximize it, but not return it to its original
        // position.  Instead, we move it such that it's y-pos is the same as it was while
        // maximized, with the x-pos "scaled" so the pointer is above a proportional location along
        // the possibly-narrower titlebar.

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

    fn start_window_move_pre(&mut self, window: &WindowElement, start_location: Point<f64, Logical>, trigger: GrabTrigger) {
        self.unmaximize_for_move(window, start_location);
        window.set_moving_state(true);

        if trigger == GrabTrigger::Pointer || trigger == GrabTrigger::Keyboard {
            self.core.set_cursor(CursorName::Fleur);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_start_window_move<PF, TF, KF>(
        &mut self,
        window: WindowElement,
        seat: Seat<Xfwl4State<BackendData>>,
        serial: Serial,
        trigger: GrabTrigger,
        set_pointer_grab: PF,
        set_touch_grab: TF,
        set_keyboard_grab: KF,
    ) where
        PF: FnOnce(
                &mut Xfwl4State<BackendData>,
                PointerHandle<Xfwl4State<BackendData>>,
                PointerGrabStartData<Xfwl4State<BackendData>>,
                WindowElement,
                Seat<Xfwl4State<BackendData>>,
                Serial,
                Point<i32, Logical>,
            ) + 'static,
        TF: FnOnce(
                &mut Xfwl4State<BackendData>,
                TouchHandle<Xfwl4State<BackendData>>,
                TouchGrabStartData<Xfwl4State<BackendData>>,
                WindowElement,
                Seat<Xfwl4State<BackendData>>,
                Serial,
                Point<i32, Logical>,
            ) + 'static,
        KF: FnOnce(
                &mut Xfwl4State<BackendData>,
                KeyboardHandle<Xfwl4State<BackendData>>,
                KeyboardGrabStartData<Xfwl4State<BackendData>>,
                WindowElement,
                Seat<Xfwl4State<BackendData>>,
                Serial,
                Point<i32, Logical>,
            ) + 'static,
    {
        if let Some(initial_window_location) = self.core.workspace_manager.active_workspace().element_location(&window) {
            match trigger {
                GrabTrigger::Pointer => {
                    if let Some(pointer) = seat.get_pointer()
                        && let Some(start_data) = pointer.grab_start_data()
                        && check_move_resize_focus_ownership_pointer(&start_data.focus, window.wl_surface())
                    {
                        set_pointer_grab(self, pointer, start_data, window, seat, serial, initial_window_location);
                    }
                }

                GrabTrigger::Touch => {
                    if let Some(touch) = seat.get_touch()
                        && let Some(start_data) = touch.grab_start_data()
                        && check_move_resize_focus_ownership_pointer(&start_data.focus, window.wl_surface())
                    {
                        set_touch_grab(self, touch, start_data, window, seat, serial, initial_window_location);
                    }
                }

                GrabTrigger::Keyboard => {
                    if let Some(keyboard) = seat.get_keyboard() {
                        let start_data = keyboard.grab_start_data().unwrap_or_else(|| KeyboardGrabStartData {
                            focus: keyboard.current_focus(),
                        });
                        if check_move_resize_focus_ownership_keyboard(&start_data.focus, window.wl_surface()) {
                            set_keyboard_grab(self, keyboard, start_data, window, seat, serial, initial_window_location);
                        }
                    }
                }
            }
        }
    }

    pub(in crate::core) fn start_maybe_window_move(
        &mut self,
        window: WindowElement,
        seat: Seat<Self>,
        serial: Serial,
        trigger: GrabTrigger,
    ) {
        self.handle_start_window_move(
            window,
            seat,
            serial,
            trigger,
            |state, pointer, start_data, window, seat, serial, initial_window_location| {
                let grab = MaybeGrab::new_pointer(
                    move |state, start_data| {
                        state.start_window_move_pre(&window, start_data.location, GrabTrigger::Pointer);
                        let grab = PointerMoveSurfaceGrab {
                            start_data,
                            window,
                            initial_window_location,
                        };
                        (grab, Focus::Clear)
                    },
                    start_data,
                    seat,
                    Some(serial),
                );
                pointer.set_grab(state, grab, serial, Focus::Keep);

                // TODO: register timer to auto-start move after delay, even with no motion
            },
            |state, touch, start_data, window, seat, serial, initial_window_location| {
                let grab = MaybeGrab::new_touch(
                    move |state, start_data| {
                        state.start_window_move_pre(&window, start_data.location, GrabTrigger::Touch);
                        let grab = TouchMoveSurfaceGrab {
                            start_data,
                            window,
                            initial_window_location,
                        };
                        (grab, Focus::Clear)
                    },
                    start_data,
                    seat,
                    Some(serial),
                );
                touch.set_grab(state, grab, serial);
            },
            |state, keyboard, start_data, window, _seat, serial, initial_window_location| {
                let pointer_location = state.core.pointer.current_location();
                state.start_window_move_pre(&window, pointer_location, GrabTrigger::Keyboard);
                let grab = KeyboardMoveSurfaceGrab {
                    start_data,
                    window,
                    initial_window_location,
                    move_amount: (0, 0).into(),
                };
                keyboard.set_grab(state, grab, serial);

                // TODO: need to set a pointer grab too so the window stops getting events and we
                // can use our custom fleur cursor
            },
        );
    }

    pub(in crate::core) fn start_window_move(&mut self, window: WindowElement, seat: Seat<Self>, serial: Serial, trigger: GrabTrigger) {
        self.handle_start_window_move(
            window,
            seat,
            serial,
            trigger,
            |state, pointer, start_data, window, _seat, serial, initial_window_location| {
                state.start_window_move_pre(&window, start_data.location, GrabTrigger::Pointer);
                let grab = PointerMoveSurfaceGrab {
                    start_data,
                    window,
                    initial_window_location,
                };
                pointer.set_grab(state, grab, serial, Focus::Clear);
            },
            |state, touch, start_data, window, _seat, serial, initial_window_location| {
                state.start_window_move_pre(&window, start_data.location, GrabTrigger::Touch);
                let grab = TouchMoveSurfaceGrab {
                    start_data,
                    window,
                    initial_window_location,
                };
                touch.set_grab(state, grab, serial);
            },
            |state, keyboard, start_data, window, _seat, serial, initial_window_location| {
                let pointer_location = state.core.pointer.current_location();
                state.start_window_move_pre(&window, pointer_location, GrabTrigger::Keyboard);
                let grab = KeyboardMoveSurfaceGrab {
                    start_data,
                    window,
                    initial_window_location,
                    move_amount: (0, 0).into(),
                };
                keyboard.set_grab(state, grab, serial);

                // TODO: need to set a pointer grab too so the window stops getting events and we
                // can use our custom fleur cursor
            },
        );
    }

    fn start_window_resize_pre(&mut self, window: &WindowElement, wl_surface: &WlSurface, new_resize_state: ResizeState) {
        // Here we want to unmaximize the window, but we *don't* want to return it to its original
        // size.
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

    #[allow(clippy::too_many_arguments)]
    fn handle_start_window_resize<PF, TF, KF>(
        &mut self,
        window: WindowElement,
        seat: Seat<Xfwl4State<BackendData>>,
        serial: Serial,
        edges: ResizeEdge,
        trigger: GrabTrigger,
        set_pointer_grab: PF,
        set_touch_grab: TF,
        set_keyboard_grab: KF,
    ) where
        PF: FnOnce(
                &mut Xfwl4State<BackendData>,
                PointerHandle<Xfwl4State<BackendData>>,
                PointerGrabStartData<Xfwl4State<BackendData>>,
                ResizeState,
                WindowElement,
                &WlSurface,
                Seat<Xfwl4State<BackendData>>,
                Serial,
                Rectangle<i32, Logical>,
            ) + 'static,
        TF: FnOnce(
                &mut Xfwl4State<BackendData>,
                TouchHandle<Xfwl4State<BackendData>>,
                TouchGrabStartData<Xfwl4State<BackendData>>,
                ResizeState,
                WindowElement,
                &WlSurface,
                Seat<Xfwl4State<BackendData>>,
                Serial,
                Rectangle<i32, Logical>,
            ) + 'static,
        KF: FnOnce(
                &mut Xfwl4State<BackendData>,
                KeyboardHandle<Xfwl4State<BackendData>>,
                KeyboardGrabStartData<Xfwl4State<BackendData>>,
                ResizeState,
                WindowElement,
                &WlSurface,
                Seat<Xfwl4State<BackendData>>,
                Serial,
                Rectangle<i32, Logical>,
            ) + 'static,
    {
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
            });

            match trigger {
                GrabTrigger::Pointer => {
                    if let Some(pointer) = seat.get_pointer()
                        && let Some(start_data) = pointer.grab_start_data()
                        && check_move_resize_focus_ownership_pointer(&start_data.focus, window.wl_surface())
                    {
                        let wl_surface = wl_surface.into_owned();
                        set_pointer_grab(
                            self,
                            pointer,
                            start_data,
                            new_resize_state,
                            window,
                            &wl_surface,
                            seat,
                            serial,
                            initial_window_geom,
                        );
                    }
                }

                GrabTrigger::Touch => {
                    if let Some(touch) = seat.get_touch()
                        && let Some(start_data) = touch.grab_start_data()
                        && check_move_resize_focus_ownership_pointer(&start_data.focus, window.wl_surface())
                    {
                        let wl_surface = wl_surface.into_owned();
                        set_touch_grab(
                            self,
                            touch,
                            start_data,
                            new_resize_state,
                            window,
                            &wl_surface,
                            seat,
                            serial,
                            initial_window_geom,
                        );
                    }
                }

                GrabTrigger::Keyboard => {
                    if let Some(keyboard) = seat.get_keyboard() {
                        let start_data = keyboard.grab_start_data().unwrap_or_else(|| KeyboardGrabStartData {
                            focus: keyboard.current_focus(),
                        });
                        if check_move_resize_focus_ownership_keyboard(&start_data.focus, window.wl_surface()) {
                            let wl_surface = wl_surface.into_owned();
                            set_keyboard_grab(
                                self,
                                keyboard,
                                start_data,
                                new_resize_state,
                                window,
                                &wl_surface,
                                seat,
                                serial,
                                initial_window_geom,
                            );
                        }
                    }
                }
            }
        }
    }

    pub(in crate::core) fn start_maybe_window_resize(
        &mut self,
        window: WindowElement,
        seat: Seat<Self>,
        serial: Serial,
        edges: ResizeEdge,
        trigger: GrabTrigger,
    ) {
        self.handle_start_window_resize(
            window,
            seat,
            serial,
            edges,
            trigger,
            move |state, pointer, start_data, new_resize_state, window, wl_surface, seat, serial, initial_window_geom| {
                let wl_surface = wl_surface.clone();
                let grab = MaybeGrab::new_pointer(
                    move |state, start_data| {
                        state.start_window_resize_pre(&window, &wl_surface, new_resize_state);
                        let grab = PointerResizeSurfaceGrab {
                            edges,
                            start_data,
                            window,
                            initial_window_location: initial_window_geom.loc,
                            initial_window_size: initial_window_geom.size,
                            last_window_size: initial_window_geom.size,
                        };
                        (grab, Focus::Clear)
                    },
                    start_data,
                    seat,
                    Some(serial),
                );
                pointer.set_grab(state, grab, serial, Focus::Keep);

                // TODO: register timer to auto-start resize after delay, even with no motion
            },
            move |state, touch, start_data, new_resize_state, window, wl_surface, seat, serial, initial_window_geom| {
                let wl_surface = wl_surface.clone();
                let grab = MaybeGrab::new_touch(
                    move |state, start_data| {
                        state.start_window_resize_pre(&window, &wl_surface, new_resize_state);
                        let grab = TouchResizeSurfaceGrab {
                            edges,
                            start_data,
                            window,
                            initial_window_location: initial_window_geom.loc,
                            initial_window_size: initial_window_geom.size,
                            last_window_size: initial_window_geom.size,
                        };
                        (grab, Focus::Clear)
                    },
                    start_data,
                    seat,
                    Some(serial),
                );
                touch.set_grab(state, grab, serial);
            },
            move |state, keyboard, start_data, new_resize_state, window, wl_surface, _seat, serial, initial_window_geom| {
                state.start_window_resize_pre(&window, wl_surface, new_resize_state);
                let grab = KeyboardResizeSurfaceGrab {
                    start_data,
                    window,
                    edges: ResizeEdge::BOTTOM_RIGHT,
                    initial_window_location: initial_window_geom.loc,
                    initial_window_size: initial_window_geom.size,
                    last_window_location: initial_window_geom.loc,
                    last_window_size: initial_window_geom.size,
                };
                keyboard.set_grab(state, grab, serial);

                // TODO: need to set a pointer grab too so the window stops getting events and we
                // can use our custom fleur cursor
            },
        );
    }

    pub(in crate::core) fn start_window_resize(
        &mut self,
        window: WindowElement,
        seat: Seat<Self>,
        serial: Serial,
        edges: ResizeEdge,
        trigger: GrabTrigger,
    ) {
        self.handle_start_window_resize(
            window,
            seat,
            serial,
            edges,
            trigger,
            move |state, pointer, start_data, new_resize_state, window, wl_surface, _seat, serial, initial_window_geom| {
                state.start_window_resize_pre(&window, wl_surface, new_resize_state);
                let grab = PointerResizeSurfaceGrab {
                    edges,
                    start_data,
                    window,
                    initial_window_location: initial_window_geom.loc,
                    initial_window_size: initial_window_geom.size,
                    last_window_size: initial_window_geom.size,
                };
                pointer.set_grab(state, grab, serial, Focus::Keep);
            },
            move |state, touch, start_data, new_resize_state, window, wl_surface, _seat, serial, initial_window_geom| {
                state.start_window_resize_pre(&window, wl_surface, new_resize_state);
                let grab = TouchResizeSurfaceGrab {
                    edges,
                    start_data,
                    window,
                    initial_window_location: initial_window_geom.loc,
                    initial_window_size: initial_window_geom.size,
                    last_window_size: initial_window_geom.size,
                };
                touch.set_grab(state, grab, serial);
            },
            move |state, keyboard, start_data, new_resize_state, window, wl_surface, _seat, serial, initial_window_geom| {
                state.start_window_resize_pre(&window, wl_surface, new_resize_state);
                let grab = KeyboardResizeSurfaceGrab {
                    start_data,
                    window,
                    edges: ResizeEdge::BOTTOM_RIGHT,
                    initial_window_location: initial_window_geom.loc,
                    initial_window_size: initial_window_geom.size,
                    last_window_location: initial_window_geom.loc,
                    last_window_size: initial_window_geom.size,
                };
                keyboard.set_grab(state, grab, serial);

                // TODO: need to set a pointer grab too so the window stops getting events and we
                // can use our custom fleur cursor
            },
        );
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
