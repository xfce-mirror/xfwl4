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

use std::collections::HashSet;

use smithay::{
    backend::input::{KeyState, TouchSlot},
    desktop::WindowSurface,
    input::{
        Seat, SeatHandler,
        keyboard::{GrabStartData as KeyboardGrabStartData, KeyboardGrab, KeyboardInnerHandle, ModifiersState},
        pointer::{
            AxisFrame, ButtonEvent, Focus, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
            GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
            GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
        },
        touch::{DownEvent, GrabStartData as TouchGrabStartData, TouchGrab},
    },
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Serial},
    wayland::shell::xdg::ToplevelSurface,
};
use xkbcommon::xkb::Keycode;

use crate::{
    backend::Backend,
    core::{
        focus::{KeyboardFocusTarget, PointerFocusTarget},
        shell::WindowElement,
        state::Xfwl4State,
        util::XkbStateGdkExt,
    },
};

pub struct TabwinPointerGrab<BackendData: Backend + 'static> {
    start_data: PointerGrabStartData<Xfwl4State<BackendData>>,
    tabwin: ToplevelSurface,
    target: PointerFocusTarget,
    pointer_over_target: bool,
}

pub struct TabwinTouchGrab<BackendData: Backend + 'static> {
    start_data: TouchGrabStartData<Xfwl4State<BackendData>>,
    tabwin: ToplevelSurface,
    target: PointerFocusTarget,
    touches_down_on_target: HashSet<TouchSlot>,
    touches_on_target: HashSet<TouchSlot>,
}

pub struct TabwinKeyboardGrab<BackendData: Backend + 'static> {
    start_data: KeyboardGrabStartData<Xfwl4State<BackendData>>,
    tabwin: ToplevelSurface,
    target: KeyboardFocusTarget,
}

impl<BackendData: Backend + 'static> PointerGrab<Xfwl4State<BackendData>> for TabwinPointerGrab<BackendData> {
    fn motion(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(<Xfwl4State<BackendData> as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        self.pointer_over_target = focus.as_ref().is_some_and(|(target, _)| *target == self.target);
        let tabwin_focus = focus.filter(|(target, _)| *target == self.target);
        handle.motion(data, tabwin_focus, event);
    }

    fn relative_motion(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(<Xfwl4State<BackendData> as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        self.pointer_over_target = focus.as_ref().is_some_and(|(target, _)| *target == self.target);
        let tabwin_focus = focus.filter(|(target, _)| *target == self.target);
        handle.relative_motion(data, tabwin_focus, event);
    }

    fn button(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &ButtonEvent,
    ) {
        if self.pointer_over_target {
            handle.button(data, event);
        } else {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        details: AxisFrame,
    ) {
        if self.pointer_over_target {
            handle.axis(data, details);
        }
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

    fn frame(&mut self, data: &mut Xfwl4State<BackendData>, handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>) {
        handle.frame(data);
    }

    fn unset(&mut self, data: &mut Xfwl4State<BackendData>) {
        if data.core.tabwin_grabs_active {
            data.core.tabwin_grabs_active = false;
            if let Some(keyboard) = data.core.seat.get_keyboard() {
                keyboard.unset_grab(data);
            }
            if let Some(touch) = data.core.seat.clone().get_touch() {
                touch.unset_grab(data);
            }
            self.tabwin.send_close();
            data.core.cycling_windows = false;
        }
    }

    fn start_data(&self) -> &PointerGrabStartData<Xfwl4State<BackendData>> {
        &self.start_data
    }
}

impl<BackendData: Backend + 'static> TouchGrab<Xfwl4State<BackendData>> for TabwinTouchGrab<BackendData> {
    fn motion(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(<Xfwl4State<BackendData> as SeatHandler>::TouchFocus, Point<f64, Logical>)>,
        event: &smithay::input::touch::MotionEvent,
        mut seq: Serial,
    ) {
        if let Some((target, location)) = focus
            && target == self.target
        {
            if !self.touches_down_on_target.contains(&event.slot) {
                self.touches_down_on_target.insert(event.slot);

                let down = DownEvent {
                    slot: event.slot,
                    location,
                    serial: seq,
                    time: event.time,
                };
                handle.down(data, Some((target.clone(), location)), &down, seq);
                seq = SERIAL_COUNTER.next_serial();
            }

            self.touches_on_target.insert(event.slot);
            handle.motion(data, Some((target, location)), event, seq);
        } else {
            self.touches_on_target.remove(&event.slot);
        }
    }

    fn down(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(<Xfwl4State<BackendData> as SeatHandler>::TouchFocus, Point<f64, Logical>)>,
        event: &smithay::input::touch::DownEvent,
        seq: Serial,
    ) {
        if let Some((target, location)) = focus
            && target == self.target
        {
            self.touches_on_target.insert(event.slot);
            handle.down(data, Some((target, location)), event, seq);
        }
    }

    fn up(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &smithay::input::touch::UpEvent,
        seq: Serial,
    ) {
        if self.touches_down_on_target.remove(&event.slot) {
            handle.up(data, event, seq);
        }
    }

    fn cancel(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        seq: Serial,
    ) {
        self.touches_down_on_target.clear();
        self.touches_on_target.clear();
        handle.cancel(data, seq);
    }

    fn orientation(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &smithay::input::touch::OrientationEvent,
        seq: Serial,
    ) {
        if self.touches_down_on_target.contains(&event.slot) {
            handle.orientation(data, event, seq);
        }
    }

    fn shape(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &smithay::input::touch::ShapeEvent,
        seq: Serial,
    ) {
        if self.touches_down_on_target.contains(&event.slot) {
            handle.shape(data, event, seq);
        }
    }

    fn frame(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        seq: Serial,
    ) {
        handle.frame(data, seq);
    }

    fn unset(&mut self, data: &mut Xfwl4State<BackendData>) {
        if data.core.tabwin_grabs_active {
            data.core.tabwin_grabs_active = false;
            if let Some(keyboard) = data.core.seat.get_keyboard() {
                keyboard.unset_grab(data);
            }
            let serial = SERIAL_COUNTER.next_serial();
            let time = data.core.clock.now().as_millis();
            let pointer = data.core.pointer.clone();
            pointer.unset_grab(data, serial, time);
            self.tabwin.send_close();
            data.core.cycling_windows = false;
        }
    }

    fn start_data(&self) -> &TouchGrabStartData<Xfwl4State<BackendData>> {
        &self.start_data
    }
}

impl<BackendData: Backend + 'static> KeyboardGrab<Xfwl4State<BackendData>> for TabwinKeyboardGrab<BackendData> {
    fn set_focus(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut KeyboardInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<<Xfwl4State<BackendData> as SeatHandler>::KeyboardFocus>,
        serial: Serial,
    ) {
        if let Some(target) = focus
            && target == self.target
        {
            handle.set_focus(data, Some(target), serial);
        }
    }

    fn input(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut KeyboardInnerHandle<'_, Xfwl4State<BackendData>>,
        keycode: Keycode,
        state: KeyState,
        modifiers: Option<ModifiersState>,
        serial: Serial,
        time: u32,
    ) {
        handle.input(data, keycode, state, modifiers, serial, time);

        if state == KeyState::Released {
            let keysym_handle = handle.keysym_handle(keycode);
            let keysym = keysym_handle.modified_sym();
            let xkb = keysym_handle.xkb().lock().unwrap();
            // SAFETY: I drop the xkb state immediately; xkb handle itself lives longer.
            let modifier_mask = unsafe { xkb.state() }.gdk_modifier_mask();
            drop(xkb);

            tracing::debug!(
                keysym = ::xkbcommon::xkb::keysym_get_name(keysym),
                ?modifier_mask,
                "tabwin grab key-release",
            );
        }
    }

    fn unset(&mut self, data: &mut Xfwl4State<BackendData>) {
        if data.core.tabwin_grabs_active {
            data.core.tabwin_grabs_active = false;
            let serial = SERIAL_COUNTER.next_serial();
            let time = data.core.clock.now().as_millis();
            let pointer = data.core.pointer.clone();
            pointer.unset_grab(data, serial, time);
            if let Some(touch) = data.core.seat.clone().get_touch() {
                touch.unset_grab(data);
            }
            self.tabwin.send_close();
            data.core.cycling_windows = false;
        }
    }

    fn start_data(&self) -> &KeyboardGrabStartData<Xfwl4State<BackendData>> {
        &self.start_data
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub(in crate::core) fn start_tabwin_grab(&mut self, tabwin: WindowElement, seat: Seat<Self>, tabwin_geo: Rectangle<i32, Logical>) {
        if !self.core.tabwin_grabs_active
            && let WindowSurface::Wayland(surface) = tabwin.0.underlying_surface()
        {
            if let Some(pointer) = seat.get_pointer() {
                let target = PointerFocusTarget::WlSurface(surface.wl_surface().clone());
                let grab = TabwinPointerGrab {
                    start_data: pointer.grab_start_data().unwrap_or_else(|| PointerGrabStartData {
                        focus: None,
                        button: 0,
                        location: pointer.current_location(),
                    }),
                    tabwin: surface.clone(),
                    target: target.clone(),
                    pointer_over_target: true,
                };
                let serial = SERIAL_COUNTER.next_serial();
                pointer.set_grab(self, grab, serial, Focus::Clear);

                let location = pointer.current_location();
                let focus = tabwin_geo.to_f64().contains(location).then(|| (target, tabwin_geo.loc.to_f64()));
                pointer.motion(
                    self,
                    focus,
                    &MotionEvent {
                        location,
                        serial,
                        time: self.core.clock.now().as_millis(),
                    },
                );
                pointer.frame(self);

                self.core.tabwin_grabs_active = true;
            }

            if let Some(touch) = seat.get_touch() {
                let target = PointerFocusTarget::WlSurface(surface.wl_surface().clone());
                let grab = TabwinTouchGrab {
                    start_data: touch.grab_start_data().unwrap_or_else(|| TouchGrabStartData {
                        focus: None,
                        slot: TouchSlot::from(None::<u32>),
                        location: (0., 0.).into(),
                    }),
                    tabwin: surface.clone(),
                    target,
                    touches_down_on_target: HashSet::default(),
                    touches_on_target: HashSet::default(),
                };
                touch.set_grab(self, grab, SERIAL_COUNTER.next_serial());
                self.core.tabwin_grabs_active = true;
            }

            if let Some(keyboard) = seat.get_keyboard() {
                let target = KeyboardFocusTarget::Window(tabwin.0.clone());
                let grab = TabwinKeyboardGrab {
                    start_data: keyboard.grab_start_data().unwrap_or_else(|| KeyboardGrabStartData {
                        focus: Some(target.clone()),
                    }),
                    tabwin: surface.clone(),
                    target,
                };
                keyboard.set_grab(self, grab, SERIAL_COUNTER.next_serial());
                self.core.tabwin_grabs_active = true;
            }
        }
    }
}
