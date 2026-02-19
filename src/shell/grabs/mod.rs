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

mod maybe;
mod moving;
mod resize;

use std::cell::RefCell;

pub use maybe::*;
pub use moving::*;
pub use resize::*;
use smithay::{
    input::{Seat, pointer::Focus},
    utils::Serial,
    wayland::compositor::with_states,
};

use crate::{
    Xfwl4State,
    backend::Backend,
    cursor::CursorName,
    shell::{SurfaceData, WindowElement},
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GrabTrigger {
    Pointer,
    Touch,
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub(crate) fn start_maybe_window_move(&mut self, window: WindowElement, seat: Seat<Self>, serial: Serial, trigger: GrabTrigger) {
        let initial_window_location = self
            .workspace_manager
            .active_workspace()
            .element_location(&window)
            .unwrap_or_default();

        match trigger {
            GrabTrigger::Pointer => {
                if let Some(pointer) = seat.get_pointer()
                    && let Some(start_data) = pointer.grab_start_data()
                {
                    let grab = MaybeGrab::new_pointer(
                        move |state, start_data| {
                            // TODO: unmaximize window if needed

                            if let Ok(cursor) = state.cursor_theme.load_cursor(CursorName::Fleur) {
                                state.backend_data.set_cursor(cursor);
                            }

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
                    pointer.set_grab(self, grab, serial, Focus::Keep);

                    // TODO: register timer to auto-start move after delay, even with no motion
                }
            }

            GrabTrigger::Touch => {
                if let Some(touch) = seat.get_touch()
                    && let Some(start_data) = touch.grab_start_data()
                {
                    let grab = MaybeGrab::new_touch(
                        move |_state, start_data| {
                            // TODO: unmaximize window if needed
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
                    touch.set_grab(self, grab, serial);
                }
            }
        }
    }

    pub(crate) fn start_maybe_window_resize(
        &mut self,
        window: WindowElement,
        seat: Seat<Self>,
        serial: Serial,
        edges: ResizeEdge,
        trigger: GrabTrigger,
    ) {
        let mut initial_window_geom = self
            .workspace_manager
            .active_workspace()
            .element_geometry(&window)
            .unwrap_or_default();

        if let Some(window_decorations) = window.decoration_state().window_decorations() {
            initial_window_geom.loc += window_decorations.decorations_offset();
            initial_window_geom.size.w -= window_decorations.left_decoration_width() + window_decorations.right_decoration_width();
            initial_window_geom.size.h -= window_decorations.top_decoration_height() + window_decorations.bottom_decoration_height();
        }

        if let Some(wl_surface) = window.wl_surface() {
            let wl_surface = wl_surface.into_owned();
            let new_resize_state = ResizeState::Resizing(ResizeData {
                edges,
                initial_window_location: initial_window_geom.loc,
                initial_window_size: initial_window_geom.size,
            });

            match trigger {
                GrabTrigger::Pointer => {
                    if let Some(pointer) = seat.get_pointer()
                        && let Some(start_data) = pointer.grab_start_data()
                    {
                        let grab = MaybeGrab::new_pointer(
                            move |_state, start_data| {
                                // TODO: unmaximize window if needed

                                with_states(&wl_surface, move |states| {
                                    states.data_map.get::<RefCell<SurfaceData>>().unwrap().borrow_mut().resize_state = new_resize_state;
                                });

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
                        pointer.set_grab(self, grab, serial, Focus::Keep);

                        // TODO: register timer to auto-start resize after delay, even with no motion
                    }
                }

                GrabTrigger::Touch => {
                    if let Some(touch) = seat.get_touch()
                        && let Some(start_data) = touch.grab_start_data()
                    {
                        let grab = MaybeGrab::new_touch(
                            move |_state, start_data| {
                                // TODO: unmaximize window if needed

                                with_states(&wl_surface, move |states| {
                                    states.data_map.get::<RefCell<SurfaceData>>().unwrap().borrow_mut().resize_state = new_resize_state;
                                });

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
                        touch.set_grab(self, grab, serial);
                    }
                }
            }
        }
    }
}
