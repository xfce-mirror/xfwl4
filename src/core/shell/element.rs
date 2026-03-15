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
        PopupManager, Window, WindowSurface, WindowSurfaceType, layer_map_for_output,
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
    utils::{IsAlive, Logical, Physical, Point, Rectangle, SERIAL_COUNTER, Scale, Serial, user_data::UserDataMap},
    wayland::{
        compositor::{self, SurfaceData as WlSurfaceData},
        dmabuf::DmabufFeedback,
        seat::WaylandFocus,
    },
};

use super::ssd::DecorationRenderElement;
use crate::{
    backend::{AsGlesRenderer, Backend, FromGlesError},
    core::{
        config::Xfwl4Config,
        drawing::shadows::{ShadowCache, ShadowKey},
        focus::{KeyboardFocusTarget, PointerFocusTarget},
        shell::{
            SurfaceData, WindowIcon, WindowProps, WindowState,
            grabs::{ResizeEdge, ResizeState},
            xdg::{
                XdgSurfaceProps, app_id_for_xdg_toplevel, desktop_app_info_for_xdg_toplevel, icon_for_xdg_toplevel,
                window_title_for_xdg_toplevel,
            },
        },
        state::Xfwl4State,
        util::BTN_LEFT,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WindowElement(pub Window);

#[derive(Debug, Default, PartialEq, Eq)]
struct ActivatedState(Cell<bool>);
#[derive(Debug, Default, PartialEq, Eq)]
struct IsMoving(Cell<bool>);
#[derive(Debug, Default, PartialEq, Eq)]
struct IsResizing(Cell<bool>);

impl WindowElement {
    pub fn new(window: Window, config: &Xfwl4Config) -> Self {
        let window = Self(window);
        let user_data = window.0.user_data();
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

    // Do not call directly; Xfwl4State will call it through WorkspaceManager
    pub fn update_minimized_state(&self, is_minimized: bool) -> bool {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(_) => {
                let mut inner = self.0.user_data().get_or_insert(XdgSurfaceProps::default).0.lock().unwrap();
                if inner.is_minimized != is_minimized {
                    inner.is_minimized = is_minimized;
                    true
                } else {
                    false
                }
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x11_surface) => {
                if x11_surface.is_hidden() != is_minimized {
                    let _ = x11_surface.set_hidden(is_minimized);
                    true
                } else {
                    false
                }
            }
        }
    }

    fn update_window_icon(&self, window_icon: Option<&WindowIcon>) -> bool {
        let mut props = self.0.user_data().get_or_insert(WindowProps::default).0.lock().unwrap();

        if props.window_icon.as_ref() != window_icon {
            props.window_icon = window_icon.cloned();
            true
        } else {
            false
        }
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

    pub(in crate::core) fn maximized_output(&self) -> Option<Output> {
        let props = self.0.user_data().get_or_insert(WindowProps::default).0.lock().unwrap();
        props.maximized_output.clone()
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
        self.0.user_data().get_or_insert(WindowProps::default).0.lock().unwrap().is_shaded
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

    pub fn fullscreened(&self) -> bool {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(surface) => surface
                .with_committed_state(|state| state.map(|state| state.states.contains(xdg_toplevel::State::Fullscreen)))
                .unwrap_or(false),
            WindowSurface::X11(surface) => surface.is_fullscreen(),
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
pub struct SSD(WindowElement);

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

        if let Some(window_decorations) = self.decoration_state().window_decorations_mut()
            && !window_bbox.is_empty()
        {
            let is_shaded = self.shaded();
            window_decorations.update_is_shaded_state(is_shaded);

            let window_geo = SpaceElement::geometry(&self.0);
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

            if !is_shaded {
                location += decorations_offset.to_f64().to_physical(scale).to_i32_round();
                let window_elements = window_render_elements(&self.0, renderer, location, scale, window_alpha, popup_alpha);
                window_elements.into_iter().chain(decorations_elements).map(C::from).collect()
            } else {
                decorations_elements.into_iter().map(C::from).collect()
            }
        } else {
            window_render_elements(&self.0, renderer, location, scale, window_alpha, popup_alpha)
                .into_iter()
                .map(C::from)
                .collect()
        }
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub(in crate::core) fn maybe_update_window_icon(&mut self, window: &WindowElement) {
        if let Some(window_decorations) = window.decoration_state().window_decorations_mut() {
            match window.0.underlying_surface() {
                WindowSurface::Wayland(surface) => {
                    let scale = Some(self.core.workspace_manager.outputs_for_element(window))
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

    pub(in crate::core) fn activate_window(&mut self, window: &WindowElement, seat: Option<Seat<Self>>) {
        if let Some(workspace) = self.core.workspace_manager.workspace_for_window_mut(window) {
            workspace.raise_window(window, true);

            if workspace.active() {
                let seat = seat.as_ref().unwrap_or(&self.core.seat);
                if let Some(keyboard) = seat.get_keyboard() {
                    let focus = KeyboardFocusTarget::Window(window.0.clone());
                    keyboard.set_focus(self, Some(focus), SERIAL_COUNTER.next_serial());
                }
            }
        }
    }

    pub(in crate::core) fn set_window_minimized(&mut self, window: &WindowElement) {
        if self.core.workspace_manager.set_window_minimized(window) {
            self.core.toplevel_changed(
                window,
                None,
                None,
                WindowState::MINIMIZED,
                WindowState::empty(),
                Vec::new(),
                Vec::new(),
                None,
            );
        }
    }

    pub(in crate::core) fn set_window_unminimized(&mut self, window: &WindowElement, activate: bool) {
        if self.core.workspace_manager.set_window_unminimized(window, activate) {
            self.set_window_shaded(window, false);
            self.core.toplevel_changed(
                window,
                None,
                None,
                WindowState::empty(),
                WindowState::MINIMIZED,
                Vec::new(),
                Vec::new(),
                None,
            );
        }
    }

    pub(in crate::core) fn set_window_maximized(&mut self, window: &WindowElement, is_maximized: bool) {
        let workspace = if let Some(workspace) = self.core.workspace_manager.workspace_for_window_mut(window) {
            workspace
        } else {
            self.core.workspace_manager.active_workspace_mut()
        };

        if is_maximized {
            let outputs_for_window = workspace.outputs_for_element(window);
            if let Some(output) = outputs_for_window.first().or_else(|| {
                // The window hasn't been mapped yet, use the primary output instead
                workspace.outputs().next()
            }) {
                let old_geom = workspace.element_geometry(window);
                let mut inner = window.0.user_data().get_or_insert(WindowProps::default).0.lock().unwrap();
                inner.pre_maximize_geom = old_geom;
                inner.maximized_output = Some(output.clone());

                let layer_map = layer_map_for_output(output);
                let mut geometry = layer_map.non_exclusive_zone();
                drop(layer_map);

                if let Some(window_decorations) = window.decoration_state().window_decorations_mut() {
                    window_decorations.update_maximized_state(true);
                    geometry.size.w -= window_decorations.left_decoration_width() + window_decorations.right_decoration_width();
                    geometry.size.h -= window_decorations.top_decoration_height() + window_decorations.bottom_decoration_height();
                }

                match window.0.underlying_surface() {
                    WindowSurface::Wayland(surface) => {
                        surface.with_pending_state(|state| {
                            state.states.set(xdg_toplevel::State::Maximized);
                            state.size = Some(geometry.size);
                        });
                        workspace.map_element(window.clone(), geometry.loc, false);

                        // The protocol demands us to always reply with a configure,
                        // regardless of we fulfilled the request or not
                        if surface.is_initial_configure_sent() {
                            surface.send_configure();
                        }
                    }

                    #[cfg(feature = "xwayland")]
                    WindowSurface::X11(surface) => {
                        let _ = surface.set_maximized(true);
                        let _ = surface.configure(geometry);
                        workspace.map_element(window.clone(), geometry.loc, false);
                    }
                }

                self.core.toplevel_changed(
                    window,
                    None,
                    None,
                    WindowState::MAXIMIZED,
                    WindowState::empty(),
                    Vec::new(),
                    Vec::new(),
                    None,
                );
            }
        } else {
            if let Some(window_decorations) = window.decoration_state().window_decorations_mut() {
                window_decorations.update_maximized_state(false);
            }

            let mut props = window.0.user_data().get_or_insert(WindowProps::default).0.lock().unwrap();
            props.maximized_output = None;

            match window.0.underlying_surface() {
                WindowSurface::Wayland(surface) => {
                    surface.with_pending_state(|state| {
                        state.states.unset(xdg_toplevel::State::Maximized);
                        state.size = None;
                    });

                    let old_loc = props.pre_maximize_geom.take().map(|geom| geom.loc).unwrap_or_default();
                    workspace.map_element(window.clone(), old_loc, false);

                    // The protocol demands us to always reply with a configure,
                    // regardless of we fulfilled the request or not
                    if surface.is_initial_configure_sent() {
                        surface.send_configure();
                    }
                }

                #[cfg(feature = "xwayland")]
                WindowSurface::X11(surface) => {
                    if let Some(old_geom) = props.pre_maximize_geom.take() {
                        drop(props);
                        let _ = surface.set_maximized(false);
                        let _ = surface.configure(old_geom);
                        workspace.map_element(window.clone(), old_geom.loc, false);
                    }
                }
            }

            self.core.toplevel_changed(
                window,
                None,
                None,
                WindowState::empty(),
                WindowState::MAXIMIZED,
                Vec::new(),
                Vec::new(),
                None,
            );
        }
    }

    pub(in crate::core) fn set_window_filled(&mut self, window: &WindowElement) {
        if window.maximized() {
            self.set_window_maximized(window, false);
        }

        let workspace = if let Some(workspace) = self.core.workspace_manager.workspace_for_window_mut(window) {
            workspace
        } else {
            self.core.workspace_manager.active_workspace_mut()
        };

        let outputs_for_window = workspace.outputs_for_element(window);
        if let Some(output) = outputs_for_window.first().or_else(|| {
            // The window hasn't been mapped yet, use the primary output instead
            workspace.outputs().next()
        }) {
            let layer_map = layer_map_for_output(output);
            let mut geometry = layer_map.non_exclusive_zone();
            drop(layer_map);

            if let Some(window_decorations) = window.decoration_state().window_decorations_mut() {
                geometry.size.w -= window_decorations.left_decoration_width() + window_decorations.right_decoration_width();
                geometry.size.h -= window_decorations.top_decoration_height() + window_decorations.bottom_decoration_height();
            }

            match window.0.underlying_surface() {
                WindowSurface::Wayland(surface) => {
                    surface.with_pending_state(|state| {
                        state.size = Some(geometry.size);
                    });
                    workspace.map_element(window.clone(), geometry.loc, false);

                    if surface.is_initial_configure_sent() {
                        surface.send_configure();
                    }
                }

                #[cfg(feature = "xwayland")]
                WindowSurface::X11(surface) => {
                    let _ = surface.configure(geometry);
                    workspace.map_element(window.clone(), geometry.loc, false);
                }
            }
        }
    }

    pub(in crate::core) fn set_window_shaded(&self, window: &WindowElement, is_shaded: bool) {
        let mut inner = window.0.user_data().get_or_insert(WindowProps::default).0.lock().unwrap();
        let changed = if inner.is_shaded != is_shaded {
            inner.is_shaded = is_shaded;
            true
        } else {
            false
        };

        if changed {
            #[cfg(feature = "xwayland")]
            if let WindowSurface::X11(x11_surface) = window.0.underlying_surface()
                && let Some((x11_conn, _)) = &self.core.x11conn
            {
                use crate::core::util::x11::{get_atom, update_net_wm_state};

                if let Some(net_wm_state_shaded) = get_atom(x11_conn, b"_NET_WM_STATE_SHADED") {
                    let (add, remove) = if is_shaded {
                        (vec![net_wm_state_shaded], vec![])
                    } else {
                        (vec![], vec![net_wm_state_shaded])
                    };
                    update_net_wm_state(x11_conn, x11_surface.window_id(), &add, &remove);
                }
            }
        }
    }

    pub(in crate::core) fn set_window_always_on_top(&mut self, window: &WindowElement, is_always_on_top: bool) {
        if is_always_on_top != window.always_on_top() {
            let z = if is_always_on_top { RenderZindex::Top } else { RenderZindex::Shell };
            window.0.override_z_index(z as u8);

            if let Some(workspace) = self.core.workspace_manager.workspace_for_window_mut(window) {
                workspace.raise_window(window, false);
            }
        }
    }

    pub(in crate::core) fn set_window_always_on_bottom(&mut self, window: &WindowElement, is_always_on_bottom: bool) {
        if is_always_on_bottom != window.always_on_bottom() {
            let z = if is_always_on_bottom {
                RenderZindex::Bottom
            } else {
                RenderZindex::Shell
            };
            window.0.override_z_index(z as u8);

            if let Some(workspace) = self.core.workspace_manager.workspace_for_window_mut(window) {
                workspace.raise_window(window, false);
            }
        }
    }

    pub(in crate::core) fn set_window_normal_stacking(&mut self, window: &WindowElement) {
        if !window.normal_stacking() {
            window.0.override_z_index(RenderZindex::Shell as u8);

            if let Some(workspace) = self.core.workspace_manager.workspace_for_window_mut(window) {
                workspace.raise_window(window, false);
            }
        }
    }

    pub(in crate::core) fn set_window_fullscreen(&mut self, window: &WindowElement, output: Option<Output>) {
        let workspace = self.core.workspace_manager.active_workspace_mut();
        let output_and_geometry = output
            .or_else(|| workspace.outputs_for_element(window).into_iter().next())
            .or_else(|| workspace.outputs().next().cloned())
            .and_then(|output| workspace.output_geometry(&output).map(|geom| (output, geom)));

        if let Some((output, geometry)) = output_and_geometry {
            // NOTE: This is only one part of the solution. We can set the
            // location and configure size here, but the surface should be rendered fullscreen
            // independently from its buffer size

            let (fullscreened, old_fullscreen_window) = match window.0.underlying_surface() {
                WindowSurface::Wayland(surface) => {
                    let (fullscreened, old_fullscreen_window) =
                        if let Ok(client) = self.core.display_handle.get_client(surface.wl_surface().id()) {
                            let wl_output = output.client_outputs(&client).last();

                            window.disable_decorations();
                            surface.with_pending_state(|state| {
                                state.states.set(xdg_toplevel::State::Fullscreen);
                                state.size = Some(geometry.size);
                                state.fullscreen_output = wl_output;
                            });
                            tracing::trace!("Fullscreening: {:?}", window);
                            (true, workspace.set_window_fullscreen(window, &output))
                        } else {
                            (false, None)
                        };

                    // The protocol demands us to always reply with a configure,
                    // regardless of we fulfilled the request or not
                    if surface.is_initial_configure_sent() {
                        surface.send_configure();
                    }

                    (fullscreened, old_fullscreen_window)
                }

                #[cfg(feature = "xwayland")]
                WindowSurface::X11(surface) => {
                    window.disable_decorations();
                    let _ = surface.set_fullscreen(true);
                    let _ = surface.configure(geometry);
                    tracing::trace!("Fullscreening: {:?}", window);
                    (true, workspace.set_window_fullscreen(window, &output))
                }
            };

            self.backend.reset_buffers(&output);

            if let Some(old_fullscreen_window) = old_fullscreen_window {
                self.set_window_unfullscreen(&old_fullscreen_window);
            }

            if fullscreened {
                self.core.toplevel_changed(
                    window,
                    None,
                    None,
                    WindowState::FULLSCREEN,
                    WindowState::empty(),
                    Vec::new(),
                    Vec::new(),
                    None,
                );
            }
        }
    }

    pub(in crate::core) fn set_window_unfullscreen(&mut self, window: &WindowElement) {
        let workspace = self.core.workspace_manager.workspace_for_window_mut(window);

        match window.0.underlying_surface() {
            WindowSurface::Wayland(surface) => {
                surface.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Fullscreen);
                    state.size = None;
                    state.fullscreen_output = None;
                });

                // The protocol demands us to always reply with a configure,
                // regardless of we fulfilled the request or not
                if surface.is_initial_configure_sent() {
                    surface.send_configure();
                }
            }

            #[cfg(feature = "xwayland")]
            WindowSurface::X11(surface) => {
                let _ = surface.set_fullscreen(false);
                if let Some(workspace) = workspace {
                    let _ = surface.configure(workspace.element_bbox(window));
                }
                if !surface.is_decorated() {
                    self.enable_decorations_for_window(window);
                } else {
                    window.disable_decorations();
                }
            }
        }

        if let Some(workspace) = self.core.workspace_manager.workspace_for_window_mut(window)
            && let Some(output) = workspace.set_window_unfullscreen(window)
        {
            self.backend.reset_buffers(&output);
        }

        self.core.toplevel_changed(
            window,
            None,
            None,
            WindowState::empty(),
            WindowState::FULLSCREEN,
            Vec::new(),
            Vec::new(),
            None,
        );
    }

    pub(in crate::core) fn raise_window(&mut self, window: &WindowElement, serial: Serial) {
        if !window.always_on_top() || !window.normal_stacking() {
            self.set_window_normal_stacking(window);
        }

        let active_ws_num = self.core.workspace_manager.active_workspace_index();

        if let Some((ws_num, workspace)) = self.core.workspace_manager.workspace_for_window_with_index_mut(window) {
            workspace.raise_window(window, true);

            if ws_num == active_ws_num
                && let Some(keyboard) = self.core.seat.get_keyboard()
            {
                keyboard.set_focus(self, Some(window.clone().into()), serial);
            }
        }
    }

    pub(in crate::core) fn lower_window(&mut self, window: &WindowElement, serial: Serial) {
        let active_ws_num = self.core.workspace_manager.active_workspace_index();

        if let Some((ws_num, workspace)) = self.core.workspace_manager.workspace_for_window_with_index_mut(window) {
            let was_active = window.active();

            // This is annoying; smithay's Space doesn't give us direct access to order
            // windows, so we have to go through some acrobatics: override the z-index to the
            // bottom layer, "raise" the window (which removes it, re-maps it, and sorts by the
            // elements z-index), and then override the z-index back to the default.
            window.0.override_z_index(RenderZindex::Bottom as u8);
            workspace.raise_element(window, false);
            window.0.override_z_index(RenderZindex::Shell as u8);

            if ws_num == active_ws_num && was_active {
                // Next activate and give focus to the now-top window in the stack.
                if let Some(new_focus) = workspace.elements().last().cloned() {
                    workspace.raise_element(&new_focus, true);
                    if let Some(keyboard) = self.core.seat.get_keyboard() {
                        keyboard.set_focus(self, Some(new_focus.into()), serial);
                    }
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
