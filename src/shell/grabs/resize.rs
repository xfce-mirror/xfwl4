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
//
// Portions of this file are based on "anvil", an example compositor
// based on the smithay crate, and are licensed under the MIT license
// with the following terms:
//
// Copyright (C) Victor Berger <victor.berger@m4x.org>
// Copyright (C) Drakulix (Victoria Brekenfeld)
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use std::cell::RefCell;

use smithay::{
    desktop::{WindowSurface, space::SpaceElement},
    input::{
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
            GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
            GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
        },
        touch::{GrabStartData as TouchGrabStartData, TouchGrab},
    },
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{IsAlive, Logical, Point, Serial, Size},
    wayland::{compositor::with_states, shell::xdg::SurfaceCachedState},
};
#[cfg(feature = "xwayland")]
use smithay::{utils::Rectangle, xwayland::xwm::ResizeEdge as X11ResizeEdge};

use crate::{
    backend::Backend,
    focus::PointerFocusTarget,
    shell::{SurfaceData, WindowElement},
    state::Xfwl4State,
};

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct ResizeEdge: u32 {
        const TOP = 1;
        const LEFT = 2;
        const RIGHT = 4;
        const BOTTOM = 8;

        const TOP_LEFT = Self::TOP.bits() | Self::LEFT.bits();
        const TOP_RIGHT = Self::TOP.bits() | Self::RIGHT.bits();
        const BOTTOM_LEFT = Self::BOTTOM.bits() | Self::LEFT.bits();
        const BOTTOM_RIGHT = Self::BOTTOM.bits() | Self::RIGHT.bits();
    }
}

impl From<xdg_toplevel::ResizeEdge> for ResizeEdge {
    #[inline]
    fn from(x: xdg_toplevel::ResizeEdge) -> Self {
        match x {
            xdg_toplevel::ResizeEdge::None => Self::empty(),
            xdg_toplevel::ResizeEdge::Top => Self::TOP,
            xdg_toplevel::ResizeEdge::Left => Self::LEFT,
            xdg_toplevel::ResizeEdge::Right => Self::RIGHT,
            xdg_toplevel::ResizeEdge::Bottom => Self::BOTTOM,
            xdg_toplevel::ResizeEdge::TopLeft => Self::TOP_LEFT,
            xdg_toplevel::ResizeEdge::TopRight => Self::TOP_RIGHT,
            xdg_toplevel::ResizeEdge::BottomLeft => Self::BOTTOM_LEFT,
            xdg_toplevel::ResizeEdge::BottomRight => Self::BOTTOM_RIGHT,
            _ => Self::empty(),
        }
    }
}

impl From<ResizeEdge> for xdg_toplevel::ResizeEdge {
    #[inline]
    fn from(x: ResizeEdge) -> Self {
        match x {
            ResizeEdge::TOP => Self::Top,
            ResizeEdge::LEFT => Self::Left,
            ResizeEdge::RIGHT => Self::Right,
            ResizeEdge::BOTTOM => Self::Bottom,
            ResizeEdge::TOP_LEFT => Self::TopLeft,
            ResizeEdge::TOP_RIGHT => Self::TopRight,
            ResizeEdge::BOTTOM_LEFT => Self::BottomLeft,
            ResizeEdge::BOTTOM_RIGHT => Self::BottomRight,
            _ => Self::None,
        }
    }
}

#[cfg(feature = "xwayland")]
impl From<X11ResizeEdge> for ResizeEdge {
    #[inline]
    fn from(edge: X11ResizeEdge) -> Self {
        match edge {
            X11ResizeEdge::Bottom => ResizeEdge::BOTTOM,
            X11ResizeEdge::BottomLeft => ResizeEdge::BOTTOM_LEFT,
            X11ResizeEdge::BottomRight => ResizeEdge::BOTTOM_RIGHT,
            X11ResizeEdge::Left => ResizeEdge::LEFT,
            X11ResizeEdge::Right => ResizeEdge::RIGHT,
            X11ResizeEdge::Top => ResizeEdge::TOP,
            X11ResizeEdge::TopLeft => ResizeEdge::TOP_LEFT,
            X11ResizeEdge::TopRight => ResizeEdge::TOP_RIGHT,
        }
    }
}

/// Information about the resize operation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ResizeData {
    /// The edges the surface is being resized with.
    pub edges: ResizeEdge,
    /// The initial window location.
    pub initial_window_location: Point<i32, Logical>,
    /// The initial window size (geometry width and height).
    pub initial_window_size: Size<i32, Logical>,
}

/// State of the resize operation.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum ResizeState {
    /// The surface is not being resized.
    #[default]
    NotResizing,
    /// The surface is currently being resized.
    Resizing(ResizeData),
    /// The resize has finished, and the surface needs to ack the final configure.
    WaitingForFinalAck(ResizeData, Serial, Size<i32, Logical>),
    /// The resize has finished, and the surface needs to commit its final state.
    WaitingForCommit(ResizeData, Size<i32, Logical>),
}

pub struct PointerResizeSurfaceGrab<BackendData: Backend + 'static> {
    pub start_data: PointerGrabStartData<Xfwl4State<BackendData>>,
    pub window: WindowElement,
    pub edges: ResizeEdge,
    pub initial_window_location: Point<i32, Logical>,
    pub initial_window_size: Size<i32, Logical>,
    pub last_window_size: Size<i32, Logical>,
}

impl<BackendData: Backend> PointerGrab<Xfwl4State<BackendData>> for PointerResizeSurfaceGrab<BackendData> {
    fn motion(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        _focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        // While the grab is active, no client has pointer focus
        handle.motion(data, None, event);

        // It is impossible to get `min_size` and `max_size` of dead toplevel, so we return early.
        if !self.window.alive() {
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        }

        let (mut dx, mut dy) = (event.location - self.start_data.location).into();

        let mut new_window_width = self.initial_window_size.w;
        let mut new_window_height = self.initial_window_size.h;

        let left_right = ResizeEdge::LEFT | ResizeEdge::RIGHT;
        let top_bottom = ResizeEdge::TOP | ResizeEdge::BOTTOM;

        if self.edges.intersects(left_right) {
            if self.edges.intersects(ResizeEdge::LEFT) {
                dx = -dx;
            }

            new_window_width = (self.initial_window_size.w as f64 + dx) as i32;
        }

        if self.edges.intersects(top_bottom) {
            if self.edges.intersects(ResizeEdge::TOP) {
                dy = -dy;
            }

            new_window_height = (self.initial_window_size.h as f64 + dy) as i32;
        }

        let (min_size, max_size) = if let Some(surface) = self.window.wl_surface() {
            with_states(&surface, |states| {
                let mut guard = states.cached_state.get::<SurfaceCachedState>();
                let data = guard.current();
                (data.min_size, data.max_size)
            })
        } else {
            ((0, 0).into(), (0, 0).into())
        };

        let min_width = min_size.w.max(1);
        let min_height = min_size.h.max(1);
        let max_width = if max_size.w == 0 { i32::MAX } else { max_size.w };
        let max_height = if max_size.h == 0 { i32::MAX } else { max_size.h };

        new_window_width = new_window_width.max(min_width).min(max_width);
        new_window_height = new_window_height.max(min_height).min(max_height);

        self.last_window_size = (new_window_width, new_window_height).into();

        match &self.window.0.underlying_surface() {
            WindowSurface::Wayland(xdg) => {
                xdg.with_pending_state(|state| {
                    state.states.set(xdg_toplevel::State::Resizing);
                    state.size = Some(self.last_window_size);
                });
                xdg.send_pending_configure();
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x11) => {
                let Some(location) = data.workspace_manager.active_workspace().element_location(&self.window) else {
                    return;
                };
                x11.configure(Rectangle::new(location, self.last_window_size)).unwrap();
            }
        }
    }

    fn relative_motion(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            // No more buttons are pressed, release the grab.
            handle.unset_grab(self, data, event.serial, event.time, true);

            // If toplevel is dead, we can't resize it, so we return early.
            if !self.window.alive() {
                return;
            }

            match &self.window.0.underlying_surface() {
                WindowSurface::Wayland(xdg) => {
                    xdg.with_pending_state(|state| {
                        state.states.unset(xdg_toplevel::State::Resizing);
                        state.size = Some(self.last_window_size);
                    });
                    xdg.send_pending_configure();
                    if self.edges.intersects(ResizeEdge::TOP_LEFT) {
                        let inner_geometry = SpaceElement::geometry(&self.window.0);
                        let decorations_offset = self
                            .window
                            .decoration_state()
                            .window_decorations()
                            .map(|d| d.decorations_offset())
                            .unwrap_or_default();
                        let workspace = data.workspace_manager.active_workspace_mut();
                        let Some(mut location) = workspace.element_location(&self.window) else {
                            return;
                        };

                        if self.edges.intersects(ResizeEdge::LEFT) {
                            location.x = self.initial_window_location.x + (self.initial_window_size.w - inner_geometry.size.w)
                                - decorations_offset.x;
                        }
                        if self.edges.intersects(ResizeEdge::TOP) {
                            location.y = self.initial_window_location.y + (self.initial_window_size.h - inner_geometry.size.h)
                                - decorations_offset.y;
                        }

                        workspace.map_element(self.window.clone(), location, true);
                    }

                    let last_window_size = self.last_window_size;
                    with_states(&self.window.wl_surface().unwrap(), |states| {
                        let mut data = states.data_map.get::<RefCell<SurfaceData>>().unwrap().borrow_mut();
                        if let ResizeState::Resizing(resize_data) = data.resize_state {
                            data.resize_state = ResizeState::WaitingForFinalAck(resize_data, event.serial, last_window_size);
                        } else {
                            panic!("invalid resize state: {:?}", data.resize_state);
                        }
                    });
                }
                #[cfg(feature = "xwayland")]
                WindowSurface::X11(x11) => {
                    let workspace = data.workspace_manager.active_workspace_mut();
                    let Some(mut location) = workspace.element_location(&self.window) else {
                        return;
                    };
                    if self.edges.intersects(ResizeEdge::TOP_LEFT) {
                        let inner_geometry = SpaceElement::geometry(&self.window.0);
                        let decorations_offset = self
                            .window
                            .decoration_state()
                            .window_decorations()
                            .map(|d| d.decorations_offset())
                            .unwrap_or_default();

                        if self.edges.intersects(ResizeEdge::LEFT) {
                            location.x = self.initial_window_location.x + (self.initial_window_size.w - inner_geometry.size.w)
                                - decorations_offset.x;
                        }
                        if self.edges.intersects(ResizeEdge::TOP) {
                            location.y = self.initial_window_location.y + (self.initial_window_size.h - inner_geometry.size.h)
                                - decorations_offset.y;
                        }

                        workspace.map_element(self.window.clone(), location, true);
                    }
                    x11.configure(Rectangle::new(location, self.last_window_size)).unwrap();

                    let Some(surface) = self.window.wl_surface() else {
                        // X11 Window got unmapped, abort
                        return;
                    };
                    let last_window_size = self.last_window_size;
                    with_states(&surface, |states| {
                        let mut data = states.data_map.get::<RefCell<SurfaceData>>().unwrap().borrow_mut();
                        if let ResizeState::Resizing(resize_data) = data.resize_state {
                            data.resize_state = ResizeState::WaitingForCommit(resize_data, last_window_size);
                        } else {
                            panic!("invalid resize state: {:?}", data.resize_state);
                        }
                    });
                }
            }
        }
    }

    fn axis(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        details: AxisFrame,
    ) {
        handle.axis(data, details)
    }

    fn frame(&mut self, data: &mut Xfwl4State<BackendData>, handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &PointerGrabStartData<Xfwl4State<BackendData>> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut Xfwl4State<BackendData>) {}
}

pub struct TouchResizeSurfaceGrab<BackendData: Backend + 'static> {
    pub start_data: TouchGrabStartData<Xfwl4State<BackendData>>,
    pub window: WindowElement,
    pub edges: ResizeEdge,
    pub initial_window_location: Point<i32, Logical>,
    pub initial_window_size: Size<i32, Logical>,
    pub last_window_size: Size<i32, Logical>,
}

impl<BackendData: Backend> TouchGrab<Xfwl4State<BackendData>> for TouchResizeSurfaceGrab<BackendData> {
    fn down(
        &mut self,
        _data: &mut Xfwl4State<BackendData>,
        _handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        _focus: Option<(
            <Xfwl4State<BackendData> as smithay::input::SeatHandler>::TouchFocus,
            Point<f64, Logical>,
        )>,
        _event: &smithay::input::touch::DownEvent,
        _seq: Serial,
    ) {
    }

    fn up(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &smithay::input::touch::UpEvent,
        _seq: Serial,
    ) {
        if event.slot != self.start_data.slot {
            return;
        }
        handle.unset_grab(self, data);

        // If toplevel is dead, we can't resize it, so we return early.
        if !self.window.alive() {
            return;
        }

        match self.window.0.underlying_surface() {
            WindowSurface::Wayland(xdg) => {
                xdg.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Resizing);
                    state.size = Some(self.last_window_size);
                });
                xdg.send_pending_configure();
                if self.edges.intersects(ResizeEdge::TOP_LEFT) {
                    let inner_geometry = SpaceElement::geometry(&self.window.0);
                    let decorations_offset = self
                        .window
                        .decoration_state()
                        .window_decorations()
                        .map(|d| d.decorations_offset())
                        .unwrap_or_default();
                    let workspace = data.workspace_manager.active_workspace_mut();
                    let Some(mut location) = workspace.element_location(&self.window) else {
                        return;
                    };

                    if self.edges.intersects(ResizeEdge::LEFT) {
                        location.x =
                            self.initial_window_location.x + (self.initial_window_size.w - inner_geometry.size.w) - decorations_offset.x;
                    }
                    if self.edges.intersects(ResizeEdge::TOP) {
                        location.y =
                            self.initial_window_location.y + (self.initial_window_size.h - inner_geometry.size.h) - decorations_offset.y;
                    }

                    workspace.map_element(self.window.clone(), location, true);
                }

                let last_window_size = self.last_window_size;
                with_states(&self.window.wl_surface().unwrap(), |states| {
                    let mut data = states.data_map.get::<RefCell<SurfaceData>>().unwrap().borrow_mut();
                    if let ResizeState::Resizing(resize_data) = data.resize_state {
                        data.resize_state = ResizeState::WaitingForFinalAck(resize_data, event.serial, last_window_size);
                    } else {
                        panic!("invalid resize state: {:?}", data.resize_state);
                    }
                });
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x11) => {
                let workspace = data.workspace_manager.active_workspace_mut();
                let Some(mut location) = workspace.element_location(&self.window) else {
                    return;
                };
                if self.edges.intersects(ResizeEdge::TOP_LEFT) {
                    let inner_geometry = SpaceElement::geometry(&self.window.0);
                    let decorations_offset = self
                        .window
                        .decoration_state()
                        .window_decorations()
                        .map(|d| d.decorations_offset())
                        .unwrap_or_default();

                    if self.edges.intersects(ResizeEdge::LEFT) {
                        location.x =
                            self.initial_window_location.x + (self.initial_window_size.w - inner_geometry.size.w) - decorations_offset.x;
                    }
                    if self.edges.intersects(ResizeEdge::TOP) {
                        location.y =
                            self.initial_window_location.y + (self.initial_window_size.h - inner_geometry.size.h) - decorations_offset.y;
                    }

                    workspace.map_element(self.window.clone(), location, true);
                }
                x11.configure(Rectangle::new(location, self.last_window_size)).unwrap();

                let Some(surface) = self.window.wl_surface() else {
                    // X11 Window got unmapped, abort
                    return;
                };
                let last_window_size = self.last_window_size;
                with_states(&surface, |states| {
                    let mut data = states.data_map.get::<RefCell<SurfaceData>>().unwrap().borrow_mut();
                    if let ResizeState::Resizing(resize_data) = data.resize_state {
                        data.resize_state = ResizeState::WaitingForCommit(resize_data, last_window_size);
                    } else {
                        panic!("invalid resize state: {:?}", data.resize_state);
                    }
                });
            }
        }
    }

    fn motion(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        _focus: Option<(
            <Xfwl4State<BackendData> as smithay::input::SeatHandler>::TouchFocus,
            Point<f64, Logical>,
        )>,
        event: &smithay::input::touch::MotionEvent,
        _seq: Serial,
    ) {
        if event.slot != self.start_data.slot {
            return;
        }

        // It is impossible to get `min_size` and `max_size` of dead toplevel, so we return early.
        if !self.window.alive() {
            handle.unset_grab(self, data);
            return;
        }

        let (mut dx, mut dy) = (event.location - self.start_data.location).into();

        let mut new_window_width = self.initial_window_size.w;
        let mut new_window_height = self.initial_window_size.h;

        let left_right = ResizeEdge::LEFT | ResizeEdge::RIGHT;
        let top_bottom = ResizeEdge::TOP | ResizeEdge::BOTTOM;

        if self.edges.intersects(left_right) {
            if self.edges.intersects(ResizeEdge::LEFT) {
                dx = -dx;
            }

            new_window_width = (self.initial_window_size.w as f64 + dx) as i32;
        }

        if self.edges.intersects(top_bottom) {
            if self.edges.intersects(ResizeEdge::TOP) {
                dy = -dy;
            }

            new_window_height = (self.initial_window_size.h as f64 + dy) as i32;
        }

        let (min_size, max_size) = if let Some(surface) = self.window.wl_surface() {
            with_states(&surface, |states| {
                let mut guard = states.cached_state.get::<SurfaceCachedState>();
                let data = guard.current();
                (data.min_size, data.max_size)
            })
        } else {
            ((0, 0).into(), (0, 0).into())
        };

        let min_width = min_size.w.max(1);
        let min_height = min_size.h.max(1);
        let max_width = if max_size.w == 0 { i32::MAX } else { max_size.w };
        let max_height = if max_size.h == 0 { i32::MAX } else { max_size.h };

        new_window_width = new_window_width.max(min_width).min(max_width);
        new_window_height = new_window_height.max(min_height).min(max_height);

        self.last_window_size = (new_window_width, new_window_height).into();

        match self.window.0.underlying_surface() {
            WindowSurface::Wayland(xdg) => {
                xdg.with_pending_state(|state| {
                    state.states.set(xdg_toplevel::State::Resizing);
                    state.size = Some(self.last_window_size);
                });
                xdg.send_pending_configure();
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x11) => {
                if let Some(location) = data.workspace_manager.active_workspace().element_location(&self.window) {
                    x11.configure(Rectangle::new(location, self.last_window_size)).unwrap();
                }
            }
        }
    }

    fn frame(
        &mut self,
        _data: &mut Xfwl4State<BackendData>,
        _handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        _seq: Serial,
    ) {
    }

    fn cancel(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        seq: Serial,
    ) {
        handle.cancel(data, seq);
        handle.unset_grab(self, data);
    }

    fn shape(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &smithay::input::touch::ShapeEvent,
        seq: Serial,
    ) {
        handle.shape(data, event, seq);
    }

    fn orientation(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &smithay::input::touch::OrientationEvent,
        seq: Serial,
    ) {
        handle.orientation(data, event, seq);
    }

    fn start_data(&self) -> &smithay::input::touch::GrabStartData<Xfwl4State<BackendData>> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut Xfwl4State<BackendData>) {}
}
