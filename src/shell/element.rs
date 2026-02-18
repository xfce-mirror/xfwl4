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

use std::{borrow::Cow, time::Duration};

use smithay::{
    backend::{
        input::ButtonState,
        renderer::{
            ImportAll, ImportMem, Renderer, RendererSuper, Texture,
            element::{AsRenderElements, surface::WaylandSurfaceRenderElement},
            gles::GlesRenderer,
        },
    },
    desktop::{Window, WindowSurface, WindowSurfaceType, space::SpaceElement, utils::OutputPresentationFeedback},
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
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback, wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale, Serial, user_data::UserDataMap},
    wayland::{compositor::SurfaceData as WlSurfaceData, dmabuf::DmabufFeedback, seat::WaylandFocus},
};

use super::ssd::DecorationRenderElement;
use crate::{
    Xfwl4State,
    backend::{AsGlesRenderer, Backend, FromGlesError},
    focus::PointerFocusTarget,
    shell::{WindowProps, xdg::XdgSurfaceProps},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WindowElement(pub Window);

impl WindowElement {
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

    pub fn update_minimized_state(&self, is_minimized: bool) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(_) => {
                self.0
                    .user_data()
                    .get_or_insert(XdgSurfaceProps::default)
                    .0
                    .lock()
                    .unwrap()
                    .is_minimized = is_minimized;
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x11_surface) => {
                let _ = x11_surface.set_hidden(is_minimized);
            }
        }
    }

    pub fn set_shaded(&self, is_shaded: bool) {
        self.0.user_data().get_or_insert(WindowProps::default).0.lock().unwrap().is_shaded = is_shaded;
    }

    pub fn shaded(&self) -> bool {
        self.0.user_data().get_or_insert(WindowProps::default).0.lock().unwrap().is_shaded
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
    fn enter(&self, _seat: &Seat<Xfwl4State<BackendData>>, _data: &mut Xfwl4State<BackendData>, event: &MotionEvent) {
        let mut state = self.0.decoration_state();
        if let Some(window_decorations) = state.window_decorations_mut() {
            window_decorations.pointer_enter(event.location);
        }
    }
    fn motion(&self, _seat: &Seat<Xfwl4State<BackendData>>, _data: &mut Xfwl4State<BackendData>, event: &MotionEvent) {
        let mut state = self.0.decoration_state();
        if let Some(window_decorations) = state.window_decorations_mut() {
            window_decorations.pointer_enter(event.location);
        }
    }
    fn relative_motion(&self, _seat: &Seat<Xfwl4State<BackendData>>, _data: &mut Xfwl4State<BackendData>, _event: &RelativeMotionEvent) {}
    fn button(&self, seat: &Seat<Xfwl4State<BackendData>>, data: &mut Xfwl4State<BackendData>, event: &ButtonEvent) {
        let mut state = self.0.decoration_state();
        if let Some(window_decorations) = state.window_decorations_mut() {
            if event.state == ButtonState::Pressed {
                window_decorations.button_press(seat, data, &self.0, event.serial);
            } else if event.state == ButtonState::Released {
                window_decorations.button_release(seat, data, &self.0, event.serial);
            }
        }
    }
    fn axis(&self, _seat: &Seat<Xfwl4State<BackendData>>, _data: &mut Xfwl4State<BackendData>, _frame: AxisFrame) {}
    fn frame(&self, _seat: &Seat<Xfwl4State<BackendData>>, _data: &mut Xfwl4State<BackendData>) {}
    fn leave(&self, _seat: &Seat<Xfwl4State<BackendData>>, _data: &mut Xfwl4State<BackendData>, _serial: Serial, _time: u32) {
        let mut state = self.0.decoration_state();
        if let Some(window_decorations) = state.window_decorations_mut() {
            window_decorations.pointer_leave();
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
        _seq: Serial,
    ) {
        let mut state = self.0.decoration_state();
        if let Some(window_decorations) = state.window_decorations_mut() {
            window_decorations.pointer_enter(event.location);
            window_decorations.button_press(seat, data, &self.0, event.serial);
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
            window_decorations.button_release(seat, data, &self.0, event.serial);
        }
    }

    fn motion(
        &self,
        _seat: &Seat<Xfwl4State<BackendData>>,
        _data: &mut Xfwl4State<BackendData>,
        event: &smithay::input::touch::MotionEvent,
        _seq: Serial,
    ) {
        let mut state = self.0.decoration_state();
        if let Some(window_decorations) = state.window_decorations_mut() {
            window_decorations.pointer_enter(event.location);
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
}

impl<R: Renderer> std::fmt::Debug for WindowRenderElement<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WindowRenderElement::Window(arg0) => f.debug_tuple("Window").field(arg0).finish(),
            WindowRenderElement::Decoration(arg0) => f.debug_tuple("Decoration").field(arg0).finish(),
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
        let window_bbox = SpaceElement::bbox(&self.0);

        if let Some(window_decorations) = self.decoration_state().window_decorations_mut()
            && !window_bbox.is_empty()
        {
            let is_shaded = self.shaded();
            window_decorations.update_is_shaded_state(is_shaded);

            let window_geo = SpaceElement::geometry(&self.0);
            window_decorations.update_window_size(window_geo.size);

            let decorations_elements: Vec<WindowRenderElement<R>> =
                AsRenderElements::<GlesRenderer>::render_elements::<DecorationRenderElement>(
                    window_decorations,
                    renderer.gles_renderer_mut(),
                    location,
                    scale,
                    alpha,
                )
                .into_iter()
                .map(WindowRenderElement::Decoration)
                .collect();

            if !is_shaded {
                let offset = window_decorations.decorations_offset();
                location += offset.to_f64().to_physical(scale).to_i32_round();
                let window_elements = AsRenderElements::render_elements(&self.0, renderer, location, scale, alpha);
                window_elements.into_iter().chain(decorations_elements).map(C::from).collect()
            } else {
                decorations_elements.into_iter().map(C::from).collect()
            }
        } else {
            AsRenderElements::render_elements(&self.0, renderer, location, scale, alpha)
                .into_iter()
                .map(C::from)
                .collect()
        }
    }
}
