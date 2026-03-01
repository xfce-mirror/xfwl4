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

use std::marker::PhantomData;

use smithay::{
    input::{
        Seat,
        pointer::{
            AxisFrame, ButtonEvent, Focus, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
            GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
            GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
        },
        touch::{GrabStartData as TouchGrabStartData, TouchGrab, TouchInnerHandle, UpEvent},
    },
    utils::{Logical, Point, SERIAL_COUNTER, Serial},
};

use crate::{backend::Backend, core::state::Xfwl4State};

pub struct MaybeGrab<BaseGrab, StartData, BackendData, F>
where
    BackendData: Backend + 'static,
{
    upgrade: Option<F>,
    start_data: StartData,
    seat: Seat<Xfwl4State<BackendData>>,
    serial: Option<Serial>,
    _base_grab_marker: PhantomData<BaseGrab>,
}

impl<BaseGrab, BackendData, F> MaybeGrab<BaseGrab, PointerGrabStartData<Xfwl4State<BackendData>>, BackendData, F>
where
    F: FnOnce(&mut Xfwl4State<BackendData>, PointerGrabStartData<Xfwl4State<BackendData>>) -> (BaseGrab, Focus) + 'static,
    BackendData: Backend + 'static,
{
    pub fn new_pointer(
        upgrade: F,
        start_data: PointerGrabStartData<Xfwl4State<BackendData>>,
        seat: Seat<Xfwl4State<BackendData>>,
        serial: Option<Serial>,
    ) -> Self {
        Self {
            upgrade: Some(upgrade),
            start_data,
            seat,
            serial,
            _base_grab_marker: PhantomData::<BaseGrab>,
        }
    }
}

impl<BaseGrab, BackendData, F> PointerGrab<Xfwl4State<BackendData>>
    for MaybeGrab<BaseGrab, PointerGrabStartData<Xfwl4State<BackendData>>, BackendData, F>
where
    F: FnOnce(&mut Xfwl4State<BackendData>, PointerGrabStartData<Xfwl4State<BackendData>>) -> (BaseGrab, Focus) + Send + 'static,
    BackendData: Backend + 'static,
    BaseGrab: PointerGrab<Xfwl4State<BackendData>> + Send + 'static,
{
    fn motion(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(
            <Xfwl4State<BackendData> as smithay::input::SeatHandler>::PointerFocus,
            Point<f64, Logical>,
        )>,
        event: &MotionEvent,
    ) {
        handle.motion(data, focus, event);

        let diff = event.location - self.start_data.location;
        let dist = (diff.x * diff.x + diff.y * diff.y).sqrt();
        if dist >= data.core.pointer_behavior_settings.dnd_drag_threshold.w as f64
            && let Some(upgrade) = self.upgrade.take()
        {
            let start_data = self.start_data.clone();
            let seat = self.seat.clone();
            let serial = self.serial.unwrap_or(event.serial);
            data.core.handle.insert_idle(move |state| {
                if let Some(pointer) = seat.get_pointer() {
                    let (grab, focus) = upgrade(state, start_data);
                    pointer.set_grab(state, grab, serial, focus);
                }
            });
        }
    }

    fn relative_motion(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(
            <Xfwl4State<BackendData> as smithay::input::SeatHandler>::PointerFocus,
            Point<f64, Logical>,
        )>,
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
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut Xfwl4State<BackendData>, handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>) {
        handle.frame(data);
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

    fn unset(&mut self, _data: &mut Xfwl4State<BackendData>) {}

    fn start_data(&self) -> &PointerGrabStartData<Xfwl4State<BackendData>> {
        &self.start_data
    }
}

impl<BaseGrab, BackendData, F> MaybeGrab<BaseGrab, TouchGrabStartData<Xfwl4State<BackendData>>, BackendData, F>
where
    F: FnOnce(&mut Xfwl4State<BackendData>, TouchGrabStartData<Xfwl4State<BackendData>>) -> (BaseGrab, Focus) + 'static,
    BackendData: Backend + 'static,
{
    pub fn new_touch(
        upgrade: F,
        start_data: TouchGrabStartData<Xfwl4State<BackendData>>,
        seat: Seat<Xfwl4State<BackendData>>,
        serial: Option<Serial>,
    ) -> Self {
        Self {
            upgrade: Some(upgrade),
            start_data,
            seat,
            serial,
            _base_grab_marker: PhantomData::<BaseGrab>,
        }
    }
}

impl<BaseGrab, BackendData, F> TouchGrab<Xfwl4State<BackendData>>
    for MaybeGrab<BaseGrab, TouchGrabStartData<Xfwl4State<BackendData>>, BackendData, F>
where
    F: FnOnce(&mut Xfwl4State<BackendData>, TouchGrabStartData<Xfwl4State<BackendData>>) -> (BaseGrab, Focus) + Send + 'static,
    BackendData: Backend + 'static,
    BaseGrab: TouchGrab<Xfwl4State<BackendData>> + Send + 'static,
{
    fn up(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &UpEvent,
        seq: Serial,
    ) {
        handle.up(data, event, seq);

        if event.slot == self.start_data.slot {
            handle.unset_grab(self, data);
        }
    }

    fn down(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(
            <Xfwl4State<BackendData> as smithay::input::SeatHandler>::TouchFocus,
            Point<f64, Logical>,
        )>,
        event: &smithay::input::touch::DownEvent,
        seq: Serial,
    ) {
        handle.down(data, focus, event, seq);
    }

    fn motion(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(
            <Xfwl4State<BackendData> as smithay::input::SeatHandler>::TouchFocus,
            Point<f64, Logical>,
        )>,
        event: &smithay::input::touch::MotionEvent,
        seq: Serial,
    ) {
        handle.motion(data, focus, event, seq);

        let diff = event.location - self.start_data.location;
        let dist = (diff.x * diff.x + diff.y * diff.y).sqrt();
        if dist >= data.core.pointer_behavior_settings.dnd_drag_threshold.w as f64
            && let Some(upgrade) = self.upgrade.take()
        {
            let start_data = self.start_data.clone();
            let seat = self.seat.clone();
            let serial = self.serial.unwrap_or_else(|| SERIAL_COUNTER.next_serial());
            data.core.handle.insert_idle(move |state| {
                if let Some(touch) = seat.get_touch() {
                    let (grab, _) = upgrade(state, start_data);
                    touch.set_grab(state, grab, serial);
                }
            });
        }
    }

    fn shape(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &smithay::input::touch::ShapeEvent,
        seq: Serial,
    ) {
        handle.shape(data, event, seq);
    }

    fn frame(&mut self, data: &mut Xfwl4State<BackendData>, handle: &mut TouchInnerHandle<'_, Xfwl4State<BackendData>>, seq: Serial) {
        handle.frame(data, seq);
    }

    fn cancel(&mut self, data: &mut Xfwl4State<BackendData>, handle: &mut TouchInnerHandle<'_, Xfwl4State<BackendData>>, seq: Serial) {
        handle.cancel(data, seq);
        handle.unset_grab(self, data);
    }

    fn orientation(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &smithay::input::touch::OrientationEvent,
        seq: Serial,
    ) {
        handle.orientation(data, event, seq);
    }

    fn unset(&mut self, _data: &mut Xfwl4State<BackendData>) {}

    fn start_data(&self) -> &TouchGrabStartData<Xfwl4State<BackendData>> {
        &self.start_data
    }
}
