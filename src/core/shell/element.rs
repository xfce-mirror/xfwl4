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

use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    collections::VecDeque,
    sync::MutexGuard,
    time::Duration,
};

use smithay::{
    backend::{
        input::ButtonState,
        renderer::{
            ImportAll, ImportMem, Renderer, RendererSuper, Texture,
            element::{
                AsRenderElements, Kind,
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
                texture::TextureRenderElement,
            },
            gles::{GlesRenderer, GlesTexture},
        },
    },
    desktop::{
        PopupManager, Window, WindowSurface, WindowSurfaceType,
        space::{RenderZindex, SpaceElement},
        utils::OutputPresentationFeedback,
    },
    input::{
        Seat,
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
            GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, MotionEvent, PointerTarget,
            RelativeMotionEvent,
        },
        touch::TouchTarget,
    },
    output::Output,
    reexports::{
        wayland_protocols::{wp::presentation_time::server::wp_presentation_feedback, xdg::shell::server::xdg_toplevel},
        wayland_server::{Resource, protocol::wl_surface::WlSurface},
    },
    utils::{IsAlive, Logical, Monotonic, Physical, Point, Rectangle, Scale, Serial, Size, Time, user_data::UserDataMap},
    wayland::{
        compositor::{self, SurfaceData as WlSurfaceData},
        dmabuf::DmabufFeedback,
        seat::WaylandFocus,
        shell::xdg::{SurfaceCachedState, XdgToplevelSurfaceData, dialog::ToplevelDialogHint},
    },
};

use super::ssd::DecorationRenderElement;
use crate::{
    backend::{AsGlesRenderer, Backend, FromGlesError},
    core::{
        config::Xfwl4Config,
        drawing::{
            shadows::{ShadowCache, ShadowKey},
            wireframe::WireframeHolder,
        },
        focus::PointerFocusTarget,
        shell::{
            SurfaceData, TileMode, WindowIcon, WindowProps, WindowPropsInner, WindowState, WorkspaceLocation,
            grabs::{ResizeEdge, ResizeState},
            x11::X11ClientId,
            xdg::{
                XdgSurfaceProps, app_id_for_xdg_toplevel, desktop_app_info_for_xdg_toplevel, icon_for_xdg_toplevel,
                window_title_for_xdg_toplevel,
            },
        },
        state::Xfwl4State,
        util::BTN_LEFT,
        workspaces::WindowStackingLayer,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WindowElement(pub Window);

#[derive(Debug, PartialEq, Eq)]
struct WindowId(u32);

#[derive(Debug, Default, PartialEq, Eq)]
struct ActivatedState(Cell<bool>);
#[derive(Debug, Default, PartialEq, Eq)]
struct IsMoving(Cell<bool>);
#[derive(Debug, Default, PartialEq, Eq)]
struct IsResizing(Cell<bool>);

#[derive(Debug, Default)]
struct ParentWindow(pub RefCell<Option<WindowElement>>);
#[derive(Debug, Default)]
struct ChildWindows(pub RefCell<Vec<WindowElement>>);

impl WindowElement {
    pub fn new(window: Window, id: u32, config: &Xfwl4Config) -> Self {
        let window = Self(window);
        let user_data = window.0.user_data();
        user_data.insert_if_missing(|| WindowId(id));
        user_data.insert_if_missing(ActivatedState::default);
        user_data.insert_if_missing(IsMoving::default);
        user_data.insert_if_missing(IsResizing::default);
        user_data.insert_if_missing(|| config.clone());
        window
    }

    pub fn surface_under(
        &self,
        location: Point<f64, Logical>,
        window_type: WindowSurfaceType,
    ) -> Option<(PointerFocusTarget, Point<i32, Logical>)> {
        let state = self.decoration_state();

        if let Some(window_decorations) = state.window_decorations()
            && window_decorations.point_is_in_decorations(location)
        {
            Some((PointerFocusTarget::SSD(SSD(self.clone())), Point::default()))
        } else {
            let offset = if let Some(window_decorations) = state.window_decorations() {
                window_decorations.decorations_offset()
            } else {
                Point::default()
            };

            let surface_under = self.0.surface_under(location - offset.to_f64(), window_type);
            let (under, loc) = match self.0.underlying_surface() {
                WindowSurface::Wayland(_) => surface_under.map(|(surface, loc)| (PointerFocusTarget::WlSurface(surface), loc)),
                #[cfg(feature = "xwayland")]
                WindowSurface::X11(s) => surface_under.map(|(_, loc)| (PointerFocusTarget::X11Surface(s.clone()), loc)),
            }?;
            Some((under, loc + offset))
        }
    }

    pub fn with_surfaces<F>(&self, processor: F)
    where
        F: FnMut(&WlSurface, &WlSurfaceData),
    {
        self.0.with_surfaces(processor);
    }

    pub fn send_frame<T, F>(&self, output: &Output, time: T, throttle: Option<Duration>, primary_scan_out_output: F)
    where
        T: Into<Duration>,
        F: FnMut(&WlSurface, &WlSurfaceData) -> Option<Output> + Copy,
    {
        self.0.send_frame(output, time, throttle, primary_scan_out_output)
    }

    pub fn send_dmabuf_feedback<'a, P, F>(&self, output: &Output, primary_scan_out_output: P, select_dmabuf_feedback: F)
    where
        P: FnMut(&WlSurface, &WlSurfaceData) -> Option<Output> + Copy,
        F: Fn(&WlSurface, &WlSurfaceData) -> &'a DmabufFeedback + Copy,
    {
        self.0.send_dmabuf_feedback(output, primary_scan_out_output, select_dmabuf_feedback)
    }

    pub fn take_presentation_feedback<F1, F2>(
        &self,
        output_feedback: &mut OutputPresentationFeedback,
        primary_scan_out_output: F1,
        presentation_feedback_flags: F2,
    ) where
        F1: FnMut(&WlSurface, &WlSurfaceData) -> Option<Output> + Copy,
        F2: FnMut(&WlSurface, &WlSurfaceData) -> wp_presentation_feedback::Kind + Copy,
    {
        self.0
            .take_presentation_feedback(output_feedback, primary_scan_out_output, presentation_feedback_flags)
    }

    pub fn window_id(&self) -> u32 {
        self.0
            .user_data()
            .get::<WindowId>()
            .expect("all windows need to be created with a window ID")
            .0
    }

    pub fn x11_client_id(&self) -> Option<&X11ClientId> {
        self.0.user_data().get::<X11ClientId>()
    }

    // Smithay's WindowFocus::same_client_as() is only about the *Wayland* client; for X11 windows,
    // they all share the same Wayland client (the XWayland server's connection).  Smithay needs
    // same_client_as() to work this way or many things break.  But sometimes we need to know the
    // difference.
    pub(in crate::core) fn same_application_as(&self, other: &WindowElement) -> bool {
        match (self.0.underlying_surface(), other.0.underlying_surface()) {
            (WindowSurface::Wayland(_), WindowSurface::Wayland(other_surface)) => self.0.same_client_as(&other_surface.wl_surface().id()),
            #[cfg(feature = "xwayland")]
            (WindowSurface::X11(_), WindowSurface::X11(_)) => {
                if let (Some(win_client_id), Some(other_client_id)) = (self.x11_client_id(), other.x11_client_id()) {
                    win_client_id == other_client_id
                } else {
                    false
                }
            }
            #[cfg(feature = "xwayland")]
            _ => false,
        }
    }

    pub(in crate::core) fn last_user_interaction(&self) -> Option<Time<Monotonic>> {
        self.props().last_user_interaction
    }

    pub fn props(&self) -> MutexGuard<'_, WindowPropsInner> {
        self.0.user_data().get_or_insert(WindowProps::default).0.lock().unwrap()
    }

    fn update_window_icon(&self, window_icon: Option<&WindowIcon>) -> bool {
        let mut props = self.props();

        if props.window_icon.as_ref() != window_icon {
            props.window_icon = window_icon.cloned();
            true
        } else {
            false
        }
    }

    pub(in crate::core) fn stacking_layer(&self) -> WindowStackingLayer {
        self.z_index().try_into().unwrap_or(WindowStackingLayer::Normal)
    }

    pub(in crate::core) fn active(&self) -> bool {
        self.0.user_data().get::<ActivatedState>().is_some_and(|s| s.0.get())
    }

    pub(in crate::core) fn title(&self) -> Option<String> {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(surface) => window_title_for_xdg_toplevel(surface),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(surface) => (!surface.title().is_empty()).then(|| surface.title()),
        }
    }

    pub(in crate::core) fn app_id(&self) -> Option<String> {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(surface) => app_id_for_xdg_toplevel(surface),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(surface) => (!surface.class().is_empty()).then(|| surface.class()),
        }
    }

    pub(in crate::core) fn maximized(&self) -> bool {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(surface) => surface.with_committed_state(|state| {
                state
                    .map(|state| state.states.contains(xdg_toplevel::State::Maximized))
                    .unwrap_or(false)
            }),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(surface) => surface.is_maximized(),
        }
    }

    pub fn minimized(&self) -> bool {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(_) => {
                self.0
                    .user_data()
                    .get_or_insert(XdgSurfaceProps::default)
                    .0
                    .lock()
                    .unwrap()
                    .is_minimized
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x11_surface) => x11_surface.is_hidden(),
        }
    }

    pub fn shaded(&self) -> bool {
        self.props().is_shaded
    }

    pub fn sticky(&self) -> bool {
        self.props().workspace_loc == WorkspaceLocation::All
    }

    pub fn can_tile(&self) -> bool {
        !self.shaded() && !self.modal() && !self.dialog() && !self.fullscreened()
    }

    pub fn tile_mode(&self) -> Option<TileMode> {
        self.props().tile_mode
    }

    pub fn always_on_top(&self) -> bool {
        self.z_index() == RenderZindex::Top as u8
    }

    pub fn always_on_bottom(&self) -> bool {
        self.z_index() == RenderZindex::Bottom as u8
    }

    pub fn normal_stacking(&self) -> bool {
        self.z_index() == RenderZindex::Shell as u8
    }

    pub fn dialog(&self) -> bool {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(surface) => compositor::with_states(surface.wl_surface(), |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .map(|role| {
                        matches!(
                            role.lock().unwrap().dialog_hint,
                            ToplevelDialogHint::Dialog | ToplevelDialogHint::Modal
                        )
                    })
                    .unwrap_or(false)
            }),

            #[cfg(feature = "xwayland")]
            WindowSurface::X11(surface) => {
                use smithay::xwayland::xwm::WmWindowType;
                surface.window_type().is_some_and(|ty| ty == WmWindowType::Dialog)
            }
        }
    }

    pub fn modal(&self) -> bool {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(surface) => compositor::with_states(surface.wl_surface(), |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .map(|role| role.lock().unwrap().dialog_hint == ToplevelDialogHint::Modal)
                    .unwrap_or(false)
            }),

            #[cfg(feature = "xwayland")]
            WindowSurface::X11(surface) => {
                // smithay has this correspond to _NET_WM_STATE_MODAL
                surface.is_popup()
            }
        }
    }

    pub fn fullscreened(&self) -> bool {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(surface) => surface
                .with_committed_state(|state| state.map(|state| state.states.contains(xdg_toplevel::State::Fullscreen)))
                .unwrap_or(false),
            WindowSurface::X11(surface) => surface.is_fullscreen(),
        }
    }

    pub fn min_max_sizes(&self) -> (Size<i32, Logical>, Size<i32, Logical>) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(surface) => compositor::with_states(surface.wl_surface(), |states| {
                let mut guard = states.cached_state.get::<SurfaceCachedState>();
                let data = guard.current();
                (data.min_size, data.max_size)
            }),

            #[cfg(feature = "xwayland")]
            WindowSurface::X11(surface) => surface
                .size_hints()
                .map(|size_hints| {
                    (
                        size_hints.min_size.unwrap_or((0, 0)).into(),
                        size_hints.max_size.unwrap_or((0, 0)).into(),
                    )
                })
                .unwrap_or_else(|| ((0, 0).into(), (0, 0).into())),
        }
    }

    pub fn state(&self) -> WindowState {
        let mut state = WindowState::empty();
        if self.maximized() {
            state |= WindowState::MAXIMIZED;
        }
        if self.minimized() {
            state |= WindowState::MINIMIZED;
        }
        if self.shaded() {
            state |= WindowState::SHADED;
        }
        if self.fullscreened() {
            state |= WindowState::FULLSCREEN;
        }
        if self.sticky() {
            state |= WindowState::STICKY;
        }
        state
    }

    pub fn set_moving_state(&self, is_moving: bool) {
        self.0.user_data().get_or_insert(IsMoving::default).0.set(is_moving);
    }

    pub fn moving(&self) -> bool {
        self.0.user_data().get::<IsMoving>().map(|v| v.0.get()).unwrap_or(false)
    }

    pub fn set_resizing_state(&self, is_resizing: bool) {
        self.0.user_data().get_or_insert(IsResizing::default).0.set(is_resizing);
    }

    pub fn resizing(&self) -> bool {
        self.0.user_data().get::<IsResizing>().map(|v| v.0.get()).unwrap_or(false)
    }

    pub fn close(&self) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(toplevel_surface) => toplevel_surface.send_close(),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x11_surface) => {
                let _ = x11_surface.close();
            }
        }
    }

    pub fn set_parent(&self, parent: Option<WindowElement>) -> bool {
        let mut pw = self.0.user_data().get_or_insert(ParentWindow::default).0.borrow_mut();
        if pw.as_ref() != parent.as_ref() {
            *pw = parent;
            true
        } else {
            false
        }
    }

    pub fn parent(&self) -> Option<WindowElement> {
        self.0.user_data().get::<ParentWindow>().and_then(|pw| pw.0.borrow().clone())
    }

    pub fn has_parent(&self) -> bool {
        self.0
            .user_data()
            .get::<ParentWindow>()
            .map(|pw| pw.0.borrow().is_some())
            .unwrap_or(false)
    }

    pub fn add_child(&self, child: WindowElement) {
        self.0.user_data().get_or_insert(ChildWindows::default).0.borrow_mut().push(child);
    }

    pub fn remove_child(&self, child: &WindowElement) {
        if let Some(cw) = self.0.user_data().get::<ChildWindows>() {
            let mut list = cw.0.borrow_mut();
            if let Some(pos) = list.iter().position(|a_child| a_child == child) {
                list.remove(pos);
            }
        }
    }

    pub fn has_children(&self) -> bool {
        !self
            .0
            .user_data()
            .get::<ChildWindows>()
            .map(|cw| cw.0.borrow().is_empty())
            .unwrap_or(false)
    }

    pub fn children(&self) -> Vec<WindowElement> {
        self.0
            .user_data()
            .get::<ChildWindows>()
            .map(|cw| cw.0.borrow().clone())
            .unwrap_or_default()
    }

    pub fn has_modal_child(&self) -> bool {
        let mut queue = VecDeque::new();
        queue.extend(self.children());
        loop {
            if let Some(window) = queue.pop_front() {
                if window.modal() {
                    break true;
                }
                queue.extend(window.children());
            } else {
                break false;
            }
        }
    }

    pub fn handle_destroyed(&self) {
        let children = self.children();
        let parent = self.parent();

        for child in &children {
            child.set_parent(parent.clone());
        }

        if let Some(parent) = parent {
            for child in children {
                parent.add_child(child);
            }
        }
    }

    #[cfg(feature = "xwayland")]
    #[inline]
    pub fn is_x11(&self) -> bool {
        self.0.is_x11()
    }

    #[inline]
    pub fn is_wayland(&self) -> bool {
        self.0.is_wayland()
    }

    #[inline]
    pub fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        self.0.wl_surface()
    }

    #[inline]
    pub fn user_data(&self) -> &UserDataMap {
        self.0.user_data()
    }
}

impl IsAlive for WindowElement {
    #[inline]
    fn alive(&self) -> bool {
        self.0.alive()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SSD(pub WindowElement);

impl IsAlive for SSD {
    #[inline]
    fn alive(&self) -> bool {
        self.0.alive()
    }
}

impl WaylandFocus for SSD {
    #[inline]
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        self.0.wl_surface()
    }
}

impl<BackendData: Backend> PointerTarget<Xfwl4State<BackendData>> for SSD {
    fn enter(&self, seat: &Seat<Xfwl4State<BackendData>>, data: &mut Xfwl4State<BackendData>, event: &MotionEvent) {
        let mut state = self.0.decoration_state();
        if let Some(window_decorations) = state.window_decorations_mut() {
            window_decorations.pointer_motion(seat, data, &self.0, event.serial, event.location);
        }
    }
    fn motion(&self, seat: &Seat<Xfwl4State<BackendData>>, data: &mut Xfwl4State<BackendData>, event: &MotionEvent) {
        let mut state = self.0.decoration_state();
        if let Some(window_decorations) = state.window_decorations_mut() {
            window_decorations.pointer_motion(seat, data, &self.0, event.serial, event.location);
        }
    }
    fn relative_motion(&self, _seat: &Seat<Xfwl4State<BackendData>>, _data: &mut Xfwl4State<BackendData>, _event: &RelativeMotionEvent) {}
    fn button(&self, seat: &Seat<Xfwl4State<BackendData>>, data: &mut Xfwl4State<BackendData>, event: &ButtonEvent) {
        let mut state = self.0.decoration_state();
        if let Some(window_decorations) = state.window_decorations_mut() {
            if event.state == ButtonState::Pressed {
                window_decorations.button_press(seat, data, &self.0, event.button, event.serial);
            } else if event.state == ButtonState::Released {
                window_decorations.button_release(seat, data, &self.0, event.button, event.serial, event.time);
            }
        }
    }
    fn axis(&self, seat: &Seat<Xfwl4State<BackendData>>, data: &mut Xfwl4State<BackendData>, frame: AxisFrame) {
        let mut state = self.0.decoration_state();
        if let Some(window_decorations) = state.window_decorations_mut() {
            window_decorations.pointer_axis(seat, data, &self.0, frame.time, frame.axis);
        }
    }
    fn frame(&self, _seat: &Seat<Xfwl4State<BackendData>>, _data: &mut Xfwl4State<BackendData>) {}
    fn leave(&self, _seat: &Seat<Xfwl4State<BackendData>>, data: &mut Xfwl4State<BackendData>, _serial: Serial, _time: u32) {
        let mut state = self.0.decoration_state();
        if let Some(window_decorations) = state.window_decorations_mut() {
            window_decorations.pointer_leave(data);
        }
    }
    fn gesture_swipe_begin(
        &self,
        _seat: &Seat<Xfwl4State<BackendData>>,
        _data: &mut Xfwl4State<BackendData>,
        _event: &GestureSwipeBeginEvent,
    ) {
    }
    fn gesture_swipe_update(
        &self,
        _seat: &Seat<Xfwl4State<BackendData>>,
        _data: &mut Xfwl4State<BackendData>,
        _event: &GestureSwipeUpdateEvent,
    ) {
    }
    fn gesture_swipe_end(&self, _seat: &Seat<Xfwl4State<BackendData>>, _data: &mut Xfwl4State<BackendData>, _event: &GestureSwipeEndEvent) {
    }
    fn gesture_pinch_begin(
        &self,
        _seat: &Seat<Xfwl4State<BackendData>>,
        _data: &mut Xfwl4State<BackendData>,
        _event: &GesturePinchBeginEvent,
    ) {
    }
    fn gesture_pinch_update(
        &self,
        _seat: &Seat<Xfwl4State<BackendData>>,
        _data: &mut Xfwl4State<BackendData>,
        _event: &GesturePinchUpdateEvent,
    ) {
    }
    fn gesture_pinch_end(&self, _seat: &Seat<Xfwl4State<BackendData>>, _data: &mut Xfwl4State<BackendData>, _event: &GesturePinchEndEvent) {
    }
    fn gesture_hold_begin(
        &self,
        _seat: &Seat<Xfwl4State<BackendData>>,
        _data: &mut Xfwl4State<BackendData>,
        _event: &GestureHoldBeginEvent,
    ) {
    }
    fn gesture_hold_end(&self, _seat: &Seat<Xfwl4State<BackendData>>, _data: &mut Xfwl4State<BackendData>, _event: &GestureHoldEndEvent) {}
}

impl<BackendData: Backend> TouchTarget<Xfwl4State<BackendData>> for SSD {
    fn down(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &smithay::input::touch::DownEvent,
        seq: Serial,
    ) {
        let mut state = self.0.decoration_state();
        if let Some(window_decorations) = state.window_decorations_mut() {
            window_decorations.pointer_motion(seat, data, &self.0, seq, event.location);
            // TODO: pick button based on number of fingers?
            window_decorations.touch_down(seat, data, &self.0, BTN_LEFT, seq);
        }
    }

    fn up(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &smithay::input::touch::UpEvent,
        _seq: Serial,
    ) {
        let mut state = self.0.decoration_state();
        if let Some(window_decorations) = state.window_decorations_mut() {
            // TODO: pick button based on number of fingers?
            window_decorations.button_release(seat, data, &self.0, BTN_LEFT, event.serial, event.time);
        }
    }

    fn motion(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &smithay::input::touch::MotionEvent,
        seq: Serial,
    ) {
        let mut state = self.0.decoration_state();
        if let Some(window_decorations) = state.window_decorations_mut() {
            window_decorations.pointer_motion(seat, data, &self.0, seq, event.location);
        }
    }

    fn frame(&self, _seat: &Seat<Xfwl4State<BackendData>>, _data: &mut Xfwl4State<BackendData>, _seq: Serial) {}

    fn cancel(&self, _seat: &Seat<Xfwl4State<BackendData>>, _data: &mut Xfwl4State<BackendData>, _seq: Serial) {}

    fn shape(
        &self,
        _seat: &Seat<Xfwl4State<BackendData>>,
        _data: &mut Xfwl4State<BackendData>,
        _event: &smithay::input::touch::ShapeEvent,
        _seq: Serial,
    ) {
    }

    fn orientation(
        &self,
        _seat: &Seat<Xfwl4State<BackendData>>,
        _data: &mut Xfwl4State<BackendData>,
        _event: &smithay::input::touch::OrientationEvent,
        _seq: Serial,
    ) {
    }
}

impl SpaceElement for WindowElement {
    fn geometry(&self) -> Rectangle<i32, Logical> {
        let mut geo = SpaceElement::geometry(&self.0);
        let state = self.decoration_state();
        if let Some(decorations) = state.window_decorations() {
            geo.size.w += decorations.left_decoration_width() + decorations.right_decoration_width();
            geo.size.h += decorations.top_decoration_height() + decorations.bottom_decoration_height();
        }
        geo
    }
    fn bbox(&self) -> Rectangle<i32, Logical> {
        let mut bbox = SpaceElement::bbox(&self.0);
        let state = self.decoration_state();
        if let Some(decorations) = state.window_decorations() {
            bbox.size.w += decorations.left_decoration_width() + decorations.right_decoration_width();
            bbox.size.h += decorations.top_decoration_height() + decorations.bottom_decoration_height();
            let (shadow_left, shadow_top, shadow_right, shadow_bottom) = decorations.shadow_extents();
            bbox.loc.x -= shadow_left;
            bbox.loc.y -= shadow_top;
            bbox.size.w += shadow_left + shadow_right;
            bbox.size.h += shadow_top + shadow_bottom;
        }
        bbox
    }
    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool {
        let state = self.decoration_state();
        if let Some(decorations) = state.window_decorations() {
            let offset = decorations.decorations_offset();
            decorations.point_is_in_decorations(*point)
                || SpaceElement::is_in_input_region(&self.0, &(*point - Point::from((offset.x as f64, offset.y as f64))))
        } else {
            SpaceElement::is_in_input_region(&self.0, point)
        }
    }
    fn z_index(&self) -> u8 {
        SpaceElement::z_index(&self.0)
    }

    fn set_activate(&self, activated: bool) {
        SpaceElement::set_activate(&self.0, activated);
        if let Some(state) = self.0.user_data().get::<ActivatedState>() {
            state.0.set(activated);
        }
        if let Some(window_decorations) = self.decoration_state().window_decorations_mut() {
            window_decorations.update_active_state(activated);
        }
    }
    fn output_enter(&self, output: &Output, overlap: Rectangle<i32, Logical>) {
        SpaceElement::output_enter(&self.0, output, overlap);
    }
    fn output_leave(&self, output: &Output) {
        SpaceElement::output_leave(&self.0, output);
    }
    #[profiling::function]
    fn refresh(&self) {
        SpaceElement::refresh(&self.0);
    }
}

// I'd like to write this:
//
//render_elements! {
//    pub WindowRenderElement<R> where R: ImportAll + ImportMem;
//    Window=WaylandSurfaceRenderElement<R>,
//    Decoration=DecorationRenderElement as <GlesRenderer>,
//}
//
// ... but there are several bugs in render_elements! that makes this syntax not work, even though
// it seems like it should be supported.  The macro is a bit beyond my understanding, but I might
// have the LLM take a crack at fixing it at some point, assuming it can fix it without breaking
// other uses.  For now I'll have to define the enum and the impls manually (the latter of which
// I've put in element_impls.rs in order to avoid clutter).
pub enum WindowRenderElement<R: Renderer> {
    Window(WaylandSurfaceRenderElement<R>),
    Decoration(DecorationRenderElement),
    Shadow(TextureRenderElement<GlesTexture>),
    Wireframe(TextureRenderElement<GlesTexture>),
}

impl<R: Renderer> std::fmt::Debug for WindowRenderElement<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WindowRenderElement::Window(arg0) => f.debug_tuple("Window").field(arg0).finish(),
            WindowRenderElement::Decoration(arg0) => f.debug_tuple("Decoration").field(arg0).finish(),
            WindowRenderElement::Shadow(arg0) => f.debug_tuple("Shadow").field(arg0).finish(),
            WindowRenderElement::Wireframe(arg0) => f.debug_tuple("Wireframe").field(arg0).finish(),
        }
    }
}

impl<R> AsRenderElements<R> for WindowElement
where
    R: Renderer + ImportAll + ImportMem + AsGlesRenderer,
    R::TextureId: Clone + Texture + 'static,
    <R as RendererSuper>::Error: FromGlesError,
{
    type RenderElement = WindowRenderElement<R>;

    fn render_elements<C: From<Self::RenderElement>>(
        &self,
        renderer: &mut R,
        mut location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<C> {
        fn window_render_elements<R>(
            window: &Window,
            renderer: &mut R,
            location: Point<i32, Physical>,
            scale: Scale<f64>,
            window_alpha: f32,
            popup_alpha: f32,
        ) -> Vec<WindowRenderElement<R>>
        where
            R: Renderer + ImportAll + ImportMem + AsGlesRenderer,
            R::TextureId: Clone + Texture + 'static,
            <R as RendererSuper>::Error: FromGlesError,
        {
            // If we want to apply opacity to popup menus, we have to "manually" do what Window's
            // AsRenderElements::render_elements() impl does (see
            // src/desktop/space/wayland/window.rs).  Keep this in sync with smithay as smithay
            // gets updated!

            let config = window.user_data().get::<Xfwl4Config>();

            match window.underlying_surface() {
                WindowSurface::Wayland(s) => {
                    let mut render_elements: Vec<WindowRenderElement<R>> = Vec::new();
                    let surface = s.wl_surface();
                    let popup_render_elements = PopupManager::popups_for_surface(surface).flat_map(|(popup, popup_offset)| {
                        let offset = (window.geometry().loc + popup_offset - popup.geometry().loc).to_physical_precise_round(scale);
                        let popup_location = location + offset;

                        let popup_elements: Vec<WindowRenderElement<R>> = render_elements_from_surface_tree(
                            renderer,
                            popup.wl_surface(),
                            popup_location,
                            scale,
                            popup_alpha,
                            Kind::Unspecified,
                        );

                        let shadow_key = config.filter(|config| config.show_popup_shadow()).map(|config| {
                            let frame_size = popup.geometry().size;
                            ShadowKey::from_config(config, frame_size)
                        });

                        let shadow_elem = shadow_key
                            .and_then(|key| {
                                compositor::with_states(popup.wl_surface(), |states| {
                                    let cache = states.data_map.get_or_insert(ShadowCache::new);
                                    cache.render_element(key, renderer.gles_renderer_mut(), popup_location, scale, popup_alpha)
                                })
                            })
                            .map(WindowRenderElement::Shadow);

                        popup_elements.into_iter().chain(shadow_elem).collect::<Vec<_>>()
                    });

                    render_elements.extend(popup_render_elements);

                    render_elements.extend(render_elements_from_surface_tree::<_, WindowRenderElement<R>>(
                        renderer,
                        surface,
                        location,
                        scale,
                        window_alpha,
                        Kind::Unspecified,
                    ));

                    render_elements
                }
                #[cfg(feature = "xwayland")]
                WindowSurface::X11(s) => {
                    use smithay::xwayland::xwm::WmWindowType;

                    let (is_dock, is_popup) = s
                        .window_type()
                        .map(|window_type| {
                            (
                                window_type == WmWindowType::Dock,
                                matches!(
                                    window_type,
                                    WmWindowType::Menu
                                        | WmWindowType::PopupMenu
                                        | WmWindowType::DropdownMenu
                                        | WmWindowType::Tooltip
                                        | WmWindowType::Combo
                                ),
                            )
                        })
                        .unwrap_or((false, false));
                    let alpha = if is_popup { popup_alpha } else { window_alpha };

                    let x11_elements =
                        AsRenderElements::<R>::render_elements::<WindowRenderElement<R>>(s, renderer, location, scale, alpha);

                    let shadow_key = config
                        .filter(|config| (is_popup && config.show_popup_shadow()) || (is_dock && config.show_dock_shadow()))
                        .map(|config| {
                            let frame_size = s.geometry().size;
                            ShadowKey::from_config(config, frame_size)
                        });

                    let shadow_elem = shadow_key
                        .and_then(|key| {
                            let cache = s.user_data().get_or_insert(ShadowCache::new);
                            cache.render_element(key, renderer.gles_renderer_mut(), location, scale, alpha)
                        })
                        .map(WindowRenderElement::Shadow);

                    x11_elements.into_iter().chain(shadow_elem).collect()
                }
            }
        }

        profiling::scope!("WindowElement::render_elements");
        let window_bbox = SpaceElement::bbox(&self.0);

        let config = self.0.user_data().get::<Xfwl4Config>();

        let alpha_modifier = if self.moving() {
            config.map(|config| config.move_opacity()).unwrap_or(100)
        } else if self.resizing() {
            config.map(|config| config.resize_opacity()).unwrap_or(100)
        } else if !self.active() {
            config.map(|config| config.inactive_opacity()).unwrap_or(100)
        } else {
            100
        };

        let window_alpha = alpha * (alpha_modifier as f32 / 100.).clamp(0., 1.);

        let popup_opacity = config.map(|config| config.popup_opacity()).unwrap_or(100);
        let popup_alpha = alpha * (popup_opacity as f32 / 100.).clamp(0., 1.);

        let window_geo = SpaceElement::geometry(&self.0);

        let window_elements = if let Some(window_decorations) = self.decoration_state().window_decorations_mut()
            && !window_bbox.is_empty()
        {
            window_decorations.update_window_size(window_geo.size);

            let decorations_offset = window_decorations.decorations_offset();

            if let Some(wl_surface) = self.wl_surface()
                && let Some(resize_data) = compositor::with_states(&wl_surface, |states| {
                    states
                        .data_map
                        .get::<RefCell<SurfaceData>>()
                        .and_then(|d| match d.borrow().resize_state {
                            ResizeState::Resizing(data) | ResizeState::WaitingForCommit(data) => Some(data),
                            _ => None,
                        })
                })
            {
                if resize_data.edges.intersects(ResizeEdge::LEFT) {
                    let correct_x = resize_data.initial_window_location.x + (resize_data.initial_window_size.w - window_geo.size.w)
                        - decorations_offset.x;
                    location.x = (correct_x as f64 * scale.x).round() as i32;
                }
                if resize_data.edges.intersects(ResizeEdge::TOP) {
                    let correct_y = resize_data.initial_window_location.y + (resize_data.initial_window_size.h - window_geo.size.h)
                        - decorations_offset.y;
                    location.y = (correct_y as f64 * scale.y).round() as i32;
                }
            }

            let decorations_elements: Vec<WindowRenderElement<R>> =
                AsRenderElements::<GlesRenderer>::render_elements::<DecorationRenderElement>(
                    window_decorations,
                    renderer.gles_renderer_mut(),
                    location,
                    scale,
                    window_alpha,
                )
                .into_iter()
                .map(WindowRenderElement::Decoration)
                .collect();

            if !self.shaded() {
                location += decorations_offset.to_f64().to_physical(scale).to_i32_round();
                let window_elements = window_render_elements(&self.0, renderer, location, scale, window_alpha, popup_alpha);
                window_elements.into_iter().chain(decorations_elements).collect::<Vec<_>>()
            } else {
                decorations_elements.into_iter().collect::<Vec<_>>()
            }
        } else {
            if let Some(wl_surface) = self.wl_surface()
                && let Some(resize_data) = compositor::with_states(&wl_surface, |states| {
                    states
                        .data_map
                        .get::<RefCell<SurfaceData>>()
                        .and_then(|d| match d.borrow().resize_state {
                            ResizeState::Resizing(data) | ResizeState::WaitingForCommit(data) => Some(data),
                            _ => None,
                        })
                })
            {
                let geo_offset = window_geo.loc;
                if resize_data.edges.intersects(ResizeEdge::LEFT) {
                    let correct_x =
                        resize_data.initial_window_location.x + (resize_data.initial_window_size.w - window_geo.size.w) - geo_offset.x;
                    location.x = (correct_x as f64 * scale.x).round() as i32;
                }
                if resize_data.edges.intersects(ResizeEdge::TOP) {
                    let correct_y =
                        resize_data.initial_window_location.y + (resize_data.initial_window_size.h - window_geo.size.h) - geo_offset.y;
                    location.y = (correct_y as f64 * scale.y).round() as i32;
                }
            }

            window_render_elements(&self.0, renderer, location, scale, window_alpha, popup_alpha)
                .into_iter()
                .collect::<Vec<_>>()
        };

        let wireframe_element = self
            .0
            .user_data()
            .get::<WireframeHolder>()
            .and_then(|wireframe| wireframe.borrow_mut().render_element(renderer.gles_renderer_mut(), scale))
            .map(WindowRenderElement::Wireframe);

        window_elements.into_iter().chain(wireframe_element).map(C::from).collect()
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub(in crate::core) fn maybe_update_window_icon(&mut self, window: &WindowElement) {
        if let Some(window_decorations) = window.decoration_state().window_decorations_mut() {
            match window.0.underlying_surface() {
                WindowSurface::Wayland(surface) => {
                    let scale = Some(self.core.workspace_manager.outputs_for_window(window))
                        .filter(|outputs| !outputs.is_empty())
                        .unwrap_or_else(|| self.core.workspace_manager.outputs().cloned().collect())
                        .first()
                        .map(|output| output.current_scale().integer_scale())
                        .unwrap_or(1);
                    let app_info = desktop_app_info_for_xdg_toplevel(surface);

                    let icon = icon_for_xdg_toplevel(surface, scale, app_info.as_ref());
                    if window.update_window_icon(icon.as_ref()) {
                        let icon = icon.and_then(|icon| self.window_icon_to_image_data(&icon).ok());
                        window_decorations.update_app_icon(icon);
                    }
                }

                #[cfg(feature = "xwayland")]
                WindowSurface::X11(_surface) => {
                    // XXX: let's do nothing for now, as we don't have a notification mechanism for
                    // x11 window icons yet.
                }
            }
        }
    }

    pub(in crate::core) fn window_for_pointer_focus_target(&self, target: &PointerFocusTarget) -> Option<WindowElement> {
        match target {
            PointerFocusTarget::WlSurface(surface) => self.core.workspace_manager.active_workspace().window_for_surface(surface),
            PointerFocusTarget::X11Surface(surface) => surface
                .wl_surface()
                .and_then(|surface| self.core.workspace_manager.active_workspace().window_for_surface(&surface)),
            PointerFocusTarget::SSD(window) => Some(window.0.clone()),
        }
    }
}
