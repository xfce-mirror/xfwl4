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

use std::{borrow::Cow, sync::Arc};

#[cfg(feature = "xwayland")]
use smithay::xwayland::xwm::XwmOfferData;
#[cfg(feature = "xwayland")]
use smithay::xwayland::X11Surface;
pub use smithay::{
    backend::input::KeyState,
    desktop::{LayerSurface, PopupKind},
    input::{
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, PointerTarget, RelativeMotionEvent},
        Seat,
    },
    reexports::wayland_server::{backend::ObjectId, protocol::wl_surface::WlSurface, Resource},
    utils::{IsAlive, Serial},
    wayland::seat::WaylandFocus,
};
use smithay::{
    desktop::{Window, WindowSurface},
    input::{
        dnd::{DndFocus, OfferData, Source},
        pointer::{
            GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
            GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
        },
        touch::TouchTarget,
    },
    reexports::wayland_server::DisplayHandle,
    utils::{Logical, Point},
    wayland::selection::data_device::WlOfferData,
};

use crate::{
    shell::{WindowElement, SSD},
    state::{Xfwl4State, Backend},
};

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum KeyboardFocusTarget {
    Window(Window),
    LayerSurface(LayerSurface),
    Popup(PopupKind),
}

impl IsAlive for KeyboardFocusTarget {
    #[inline]
    fn alive(&self) -> bool {
        match self {
            KeyboardFocusTarget::Window(w) => w.alive(),
            KeyboardFocusTarget::LayerSurface(l) => l.alive(),
            KeyboardFocusTarget::Popup(p) => p.alive(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum PointerFocusTarget {
    WlSurface(WlSurface),
    #[cfg(feature = "xwayland")]
    X11Surface(X11Surface),
    SSD(SSD),
}

impl IsAlive for PointerFocusTarget {
    #[inline]
    fn alive(&self) -> bool {
        match self {
            PointerFocusTarget::WlSurface(w) => w.alive(),
            #[cfg(feature = "xwayland")]
            PointerFocusTarget::X11Surface(w) => w.alive(),
            PointerFocusTarget::SSD(x) => x.alive(),
        }
    }
}

impl From<PointerFocusTarget> for WlSurface {
    #[inline]
    fn from(target: PointerFocusTarget) -> Self {
        target.wl_surface().unwrap().into_owned()
    }
}

impl KeyboardFocusTarget {
    fn inner_keyboard_target<BackendData: Backend>(&self) -> &dyn KeyboardTarget<Xfwl4State<BackendData>> {
        match self {
            Self::Window(w) => match w.underlying_surface() {
                WindowSurface::Wayland(w) => w.wl_surface(),
                #[cfg(feature = "xwayland")]
                WindowSurface::X11(s) => s,
            },
            Self::LayerSurface(l) => l.wl_surface(),
            Self::Popup(p) => p.wl_surface(),
        }
    }
}

impl PointerFocusTarget {
    fn inner_pointer_target<BackendData: Backend>(&self) -> &dyn PointerTarget<Xfwl4State<BackendData>> {
        match self {
            Self::WlSurface(w) => w,
            #[cfg(feature = "xwayland")]
            Self::X11Surface(w) => w,
            Self::SSD(w) => w,
        }
    }

    fn inner_touch_target<BackendData: Backend>(&self) -> &dyn TouchTarget<Xfwl4State<BackendData>> {
        match self {
            Self::WlSurface(w) => w,
            #[cfg(feature = "xwayland")]
            Self::X11Surface(w) => w,
            Self::SSD(w) => w,
        }
    }
}

impl<BackendData: Backend> PointerTarget<Xfwl4State<BackendData>> for PointerFocusTarget {
    fn enter(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &MotionEvent,
    ) {
        self.inner_pointer_target().enter(seat, data, event)
    }
    fn motion(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &MotionEvent,
    ) {
        self.inner_pointer_target().motion(seat, data, event)
    }
    fn relative_motion(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &RelativeMotionEvent,
    ) {
        self.inner_pointer_target().relative_motion(seat, data, event)
    }
    fn button(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &ButtonEvent,
    ) {
        self.inner_pointer_target().button(seat, data, event)
    }
    fn axis(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        frame: AxisFrame,
    ) {
        self.inner_pointer_target().axis(seat, data, frame)
    }
    fn frame(&self, seat: &Seat<Xfwl4State<BackendData>>, data: &mut Xfwl4State<BackendData>) {
        self.inner_pointer_target().frame(seat, data)
    }
    fn leave(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        serial: Serial,
        time: u32,
    ) {
        self.inner_pointer_target().leave(seat, data, serial, time)
    }
    fn gesture_swipe_begin(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &GestureSwipeBeginEvent,
    ) {
        self.inner_pointer_target().gesture_swipe_begin(seat, data, event)
    }
    fn gesture_swipe_update(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &GestureSwipeUpdateEvent,
    ) {
        self.inner_pointer_target()
            .gesture_swipe_update(seat, data, event)
    }
    fn gesture_swipe_end(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &GestureSwipeEndEvent,
    ) {
        self.inner_pointer_target().gesture_swipe_end(seat, data, event)
    }
    fn gesture_pinch_begin(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &GesturePinchBeginEvent,
    ) {
        self.inner_pointer_target().gesture_pinch_begin(seat, data, event)
    }
    fn gesture_pinch_update(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &GesturePinchUpdateEvent,
    ) {
        self.inner_pointer_target()
            .gesture_pinch_update(seat, data, event)
    }
    fn gesture_pinch_end(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &GesturePinchEndEvent,
    ) {
        self.inner_pointer_target().gesture_pinch_end(seat, data, event)
    }
    fn gesture_hold_begin(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &GestureHoldBeginEvent,
    ) {
        self.inner_pointer_target().gesture_hold_begin(seat, data, event)
    }
    fn gesture_hold_end(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &GestureHoldEndEvent,
    ) {
        self.inner_pointer_target().gesture_hold_end(seat, data, event)
    }
}

impl<BackendData: Backend> KeyboardTarget<Xfwl4State<BackendData>> for KeyboardFocusTarget {
    fn enter(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        self.inner_keyboard_target().enter(seat, data, keys, serial)
    }
    fn leave(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        serial: Serial,
    ) {
        self.inner_keyboard_target().leave(seat, data, serial)
    }
    fn key(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        self.inner_keyboard_target()
            .key(seat, data, key, state, serial, time)
    }
    fn modifiers(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        self.inner_keyboard_target()
            .modifiers(seat, data, modifiers, serial)
    }
}

impl<BackendData: Backend> TouchTarget<Xfwl4State<BackendData>> for PointerFocusTarget {
    fn down(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &smithay::input::touch::DownEvent,
        seq: Serial,
    ) {
        self.inner_touch_target().down(seat, data, event, seq)
    }

    fn up(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &smithay::input::touch::UpEvent,
        seq: Serial,
    ) {
        self.inner_touch_target().up(seat, data, event, seq)
    }

    fn motion(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &smithay::input::touch::MotionEvent,
        seq: Serial,
    ) {
        self.inner_touch_target().motion(seat, data, event, seq)
    }

    fn frame(&self, seat: &Seat<Xfwl4State<BackendData>>, data: &mut Xfwl4State<BackendData>, seq: Serial) {
        self.inner_touch_target().frame(seat, data, seq)
    }

    fn cancel(&self, seat: &Seat<Xfwl4State<BackendData>>, data: &mut Xfwl4State<BackendData>, seq: Serial) {
        self.inner_touch_target().cancel(seat, data, seq)
    }

    fn shape(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &smithay::input::touch::ShapeEvent,
        seq: Serial,
    ) {
        self.inner_touch_target().shape(seat, data, event, seq)
    }

    fn orientation(
        &self,
        seat: &Seat<Xfwl4State<BackendData>>,
        data: &mut Xfwl4State<BackendData>,
        event: &smithay::input::touch::OrientationEvent,
        seq: Serial,
    ) {
        self.inner_touch_target().orientation(seat, data, event, seq)
    }
}

impl WaylandFocus for PointerFocusTarget {
    #[inline]
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            PointerFocusTarget::WlSurface(w) => w.wl_surface(),
            #[cfg(feature = "xwayland")]
            PointerFocusTarget::X11Surface(w) => w.wl_surface().map(Cow::Owned),
            PointerFocusTarget::SSD(_) => None,
        }
    }
    #[inline]
    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        match self {
            PointerFocusTarget::WlSurface(w) => w.same_client_as(object_id),
            #[cfg(feature = "xwayland")]
            PointerFocusTarget::X11Surface(w) => w.same_client_as(object_id),
            PointerFocusTarget::SSD(w) => w
                .wl_surface()
                .map(|surface| surface.same_client_as(object_id))
                .unwrap_or(false),
        }
    }
}

impl WaylandFocus for KeyboardFocusTarget {
    #[inline]
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            KeyboardFocusTarget::Window(w) => w.wl_surface(),
            KeyboardFocusTarget::LayerSurface(l) => Some(Cow::Borrowed(l.wl_surface())),
            KeyboardFocusTarget::Popup(p) => Some(Cow::Borrowed(p.wl_surface())),
        }
    }
}

pub enum Xfwl4OfferData<S: Source> {
    Wayland(WlOfferData<S>),
    #[cfg(feature = "xwayland")]
    X11(XwmOfferData<S>),
}

impl<S: Source> OfferData for Xfwl4OfferData<S> {
    fn disable(&self) {
        match self {
            Xfwl4OfferData::Wayland(data) => data.disable(),
            #[cfg(feature = "xwayland")]
            Xfwl4OfferData::X11(data) => data.disable(),
        }
    }

    fn drop(&self) {
        match self {
            Xfwl4OfferData::Wayland(data) => data.drop(),
            #[cfg(feature = "xwayland")]
            Xfwl4OfferData::X11(data) => data.drop(),
        }
    }

    fn validated(&self) -> bool {
        match self {
            Xfwl4OfferData::Wayland(data) => data.validated(),
            #[cfg(feature = "xwayland")]
            Xfwl4OfferData::X11(data) => data.validated(),
        }
    }
}

#[allow(unreachable_patterns)]
impl<BackendData: Backend> DndFocus<Xfwl4State<BackendData>> for PointerFocusTarget {
    type OfferData<S>
        = Xfwl4OfferData<S>
    where
        S: Source;

    fn enter<S: Source>(
        &self,
        data: &mut Xfwl4State<BackendData>,
        dh: &DisplayHandle,
        source: Arc<S>,
        seat: &Seat<Xfwl4State<BackendData>>,
        location: Point<f64, Logical>,
        serial: &Serial,
    ) -> Option<Xfwl4OfferData<S>> {
        match self {
            PointerFocusTarget::WlSurface(surface) => {
                DndFocus::enter(surface, data, dh, source, seat, location, serial)
                    .map(Xfwl4OfferData::Wayland)
            }
            #[cfg(feature = "xwayland")]
            PointerFocusTarget::X11Surface(surface) => {
                DndFocus::enter(surface, data, dh, source, seat, location, serial).map(Xfwl4OfferData::X11)
            }
            _ => None,
        }
    }

    fn motion<S: Source>(
        &self,
        data: &mut Xfwl4State<BackendData>,
        offer: Option<&mut Xfwl4OfferData<S>>,
        seat: &Seat<Xfwl4State<BackendData>>,
        location: Point<f64, Logical>,
        time: u32,
    ) {
        match self {
            PointerFocusTarget::WlSurface(surface) => {
                let offer = match offer {
                    Some(Xfwl4OfferData::Wayland(ref mut offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::motion(surface, data, offer, seat, location, time)
            }
            #[cfg(feature = "xwayland")]
            PointerFocusTarget::X11Surface(surface) => {
                let offer = match offer {
                    Some(Xfwl4OfferData::X11(ref mut offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::motion(surface, data, offer, seat, location, time)
            }
            _ => {}
        }
    }

    fn leave<S: Source>(
        &self,
        data: &mut Xfwl4State<BackendData>,
        offer: Option<&mut Xfwl4OfferData<S>>,
        seat: &Seat<Xfwl4State<BackendData>>,
    ) {
        match self {
            PointerFocusTarget::WlSurface(surface) => {
                let offer = match offer {
                    Some(Xfwl4OfferData::Wayland(ref mut offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::leave(surface, data, offer, seat)
            }
            #[cfg(feature = "xwayland")]
            PointerFocusTarget::X11Surface(surface) => {
                let offer = match offer {
                    Some(Xfwl4OfferData::X11(ref mut offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::leave(surface, data, offer, seat)
            }
            _ => {}
        }
    }

    fn drop<S: Source>(
        &self,
        data: &mut Xfwl4State<BackendData>,
        offer: Option<&mut Xfwl4OfferData<S>>,
        seat: &Seat<Xfwl4State<BackendData>>,
    ) {
        match self {
            PointerFocusTarget::WlSurface(surface) => {
                let offer = match offer {
                    Some(Xfwl4OfferData::Wayland(ref mut offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::drop(surface, data, offer, seat)
            }
            #[cfg(feature = "xwayland")]
            PointerFocusTarget::X11Surface(surface) => {
                let offer = match offer {
                    Some(Xfwl4OfferData::X11(ref mut offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::drop(surface, data, offer, seat)
            }
            _ => {}
        }
    }
}

impl From<WlSurface> for PointerFocusTarget {
    #[inline]
    fn from(value: WlSurface) -> Self {
        PointerFocusTarget::WlSurface(value)
    }
}

impl From<&WlSurface> for PointerFocusTarget {
    #[inline]
    fn from(value: &WlSurface) -> Self {
        PointerFocusTarget::from(value.clone())
    }
}

impl From<PopupKind> for PointerFocusTarget {
    #[inline]
    fn from(value: PopupKind) -> Self {
        PointerFocusTarget::from(value.wl_surface())
    }
}

#[cfg(feature = "xwayland")]
impl From<X11Surface> for PointerFocusTarget {
    #[inline]
    fn from(value: X11Surface) -> Self {
        PointerFocusTarget::X11Surface(value)
    }
}

#[cfg(feature = "xwayland")]
impl From<&X11Surface> for PointerFocusTarget {
    #[inline]
    fn from(value: &X11Surface) -> Self {
        PointerFocusTarget::from(value.clone())
    }
}

impl From<WindowElement> for KeyboardFocusTarget {
    #[inline]
    fn from(w: WindowElement) -> Self {
        KeyboardFocusTarget::Window(w.0.clone())
    }
}

impl From<LayerSurface> for KeyboardFocusTarget {
    #[inline]
    fn from(l: LayerSurface) -> Self {
        KeyboardFocusTarget::LayerSurface(l)
    }
}

impl From<PopupKind> for KeyboardFocusTarget {
    #[inline]
    fn from(p: PopupKind) -> Self {
        KeyboardFocusTarget::Popup(p)
    }
}

impl From<KeyboardFocusTarget> for PointerFocusTarget {
    #[inline]
    fn from(value: KeyboardFocusTarget) -> Self {
        match value {
            KeyboardFocusTarget::Window(w) => match w.underlying_surface() {
                WindowSurface::Wayland(w) => PointerFocusTarget::from(w.wl_surface()),
                #[cfg(feature = "xwayland")]
                WindowSurface::X11(s) => PointerFocusTarget::from(s),
            },
            KeyboardFocusTarget::LayerSurface(surface) => PointerFocusTarget::from(surface.wl_surface()),
            KeyboardFocusTarget::Popup(popup) => PointerFocusTarget::from(popup.wl_surface()),
        }
    }
}
