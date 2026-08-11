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

use std::fmt;

use smithay::{
    desktop::{WindowSurfaceType, layer_map_for_output, space::SpaceElement},
    input::{
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
            GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData, MotionEvent,
            PointerGrab, PointerInnerHandle, RelativeMotionEvent,
        },
        tablet::tool::{
            AxisFrame as TabletAxisFrame, ButtonEvent as TabletButtonEvent, DownEvent as TabletDownEvent,
            GrabStartData as TabletToolGrabStartData, MotionEvent as TabletMotionEvent, ProximityInEvent, ProximityOutEvent,
            TabletToolGrab, TabletToolInnerHandle, UpEvent as TabletUpEvent,
        },
        touch::{
            DownEvent as TouchDownEvent, GrabStartData as TouchGrabStartData, MotionEvent as TouchMotionEvent, OrientationEvent,
            ShapeEvent, TouchGrab, TouchInnerHandle, UpEvent as TouchUpEvent,
        },
    },
    utils::{Logical, Point},
    wayland::seat::WaylandFocus,
};

use crate::{
    backend::Backend,
    core::{focus::PointerFocusTarget, state::Xfwl4State},
};

pub struct ClickGrab<BackendData: Backend + 'static> {
    start_data: GrabStartData<Xfwl4State<BackendData>>,
    focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
}

pub struct TouchDownGrab<BackendData: Backend + 'static> {
    start_data: TouchGrabStartData<Xfwl4State<BackendData>>,
    focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
    touch_points: usize,
}

pub struct TabletToolDownGrab<BackendData: Backend + 'static> {
    start_data: TabletToolGrabStartData<Xfwl4State<BackendData>>,
    focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
}

impl<BackendData: Backend> ClickGrab<BackendData> {
    pub(in crate::core) fn new(start_data: GrabStartData<Xfwl4State<BackendData>>) -> Self {
        Self {
            focus: start_data.focus.clone(),
            start_data,
        }
    }
}

impl<BackendData: Backend + 'static> fmt::Debug for ClickGrab<BackendData> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClickGrab").field("start_data", &self.start_data).finish()
    }
}

impl<BackendData: Backend> PointerGrab<Xfwl4State<BackendData>> for ClickGrab<BackendData> {
    fn motion(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        data.refresh_grab_focus_location(&mut self.focus, focus.as_ref());
        handle.motion(data, self.focus.clone(), event);
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
            // no more buttons are pressed, release the grab
            handle.unset_grab(self, data, event.serial, event.time, false);
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

    fn start_data(&self) -> &GrabStartData<Xfwl4State<BackendData>> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut Xfwl4State<BackendData>) {}
}

impl<BackendData: Backend> TouchDownGrab<BackendData> {
    pub(in crate::core) fn new(start_data: TouchGrabStartData<Xfwl4State<BackendData>>) -> Self {
        Self {
            focus: start_data.focus.clone(),
            start_data,
            touch_points: 1,
        }
    }
}

impl<BackendData: Backend + 'static> fmt::Debug for TouchDownGrab<BackendData> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TouchDownGrab")
            .field("start_data", &self.start_data)
            .field("touch_points", &self.touch_points)
            .finish()
    }
}

impl<BackendData: Backend> TouchGrab<Xfwl4State<BackendData>> for TouchDownGrab<BackendData> {
    fn down(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
        event: &TouchDownEvent,
    ) {
        data.refresh_grab_focus_location(&mut self.focus, focus.as_ref());
        handle.down(data, self.focus.clone(), event);
        self.touch_points += 1;
    }

    fn up(&mut self, data: &mut Xfwl4State<BackendData>, handle: &mut TouchInnerHandle<'_, Xfwl4State<BackendData>>, event: &TouchUpEvent) {
        handle.up(data, event);
        self.touch_points = self.touch_points.saturating_sub(1);
        if self.touch_points == 0 {
            handle.unset_grab(self, data);
        }
    }

    fn motion(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
        event: &TouchMotionEvent,
    ) {
        data.refresh_grab_focus_location(&mut self.focus, focus.as_ref());
        handle.motion(data, self.focus.clone(), event);
    }

    fn frame(&mut self, data: &mut Xfwl4State<BackendData>, handle: &mut TouchInnerHandle<'_, Xfwl4State<BackendData>>) {
        handle.frame(data);
    }

    fn cancel(&mut self, data: &mut Xfwl4State<BackendData>, handle: &mut TouchInnerHandle<'_, Xfwl4State<BackendData>>) {
        handle.cancel(data);
        handle.unset_grab(self, data);
    }

    fn shape(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &ShapeEvent,
    ) {
        handle.shape(data, event);
    }

    fn orientation(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &OrientationEvent,
    ) {
        handle.orientation(data, event);
    }

    fn start_data(&self) -> &TouchGrabStartData<Xfwl4State<BackendData>> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut Xfwl4State<BackendData>) {}
}

impl<BackendData: Backend> TabletToolDownGrab<BackendData> {
    pub(in crate::core) fn new(start_data: TabletToolGrabStartData<Xfwl4State<BackendData>>) -> Self {
        Self {
            focus: start_data.focus.clone(),
            start_data,
        }
    }
}

impl<BackendData: Backend + 'static> fmt::Debug for TabletToolDownGrab<BackendData> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TabletToolDownGrab").field("start_data", &self.start_data).finish()
    }
}

impl<BackendData: Backend> TabletToolGrab<Xfwl4State<BackendData>> for TabletToolDownGrab<BackendData> {
    fn proximity_in(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut TabletToolInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
        event: &ProximityInEvent,
    ) {
        handle.proximity_in(data, focus, event);
    }

    fn proximity_out(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut TabletToolInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &ProximityOutEvent,
    ) {
        handle.proximity_out(data, event);
        handle.unset_grab(self, data, event.serial, event.time, false);
    }

    fn motion(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut TabletToolInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
        event: &TabletMotionEvent,
    ) {
        data.refresh_grab_focus_location(&mut self.focus, focus.as_ref());
        handle.motion(data, self.focus.clone(), event);
    }

    fn down(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut TabletToolInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &TabletDownEvent,
    ) {
        handle.down(data, event);
    }

    fn up(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut TabletToolInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &TabletUpEvent,
    ) {
        handle.up(data, event);
        handle.unset_grab(self, data, event.serial, event.time, true);
    }

    fn button(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut TabletToolInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &TabletButtonEvent,
    ) {
        handle.button(data, event);
    }

    fn axis(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut TabletToolInnerHandle<'_, Xfwl4State<BackendData>>,
        frame: TabletAxisFrame,
    ) {
        handle.axis(data, frame);
    }

    fn frame(&mut self, data: &mut Xfwl4State<BackendData>, handle: &mut TabletToolInnerHandle<'_, Xfwl4State<BackendData>>, time: u32) {
        handle.frame(data, time);
    }

    fn start_data(&self) -> &TabletToolGrabStartData<Xfwl4State<BackendData>> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut Xfwl4State<BackendData>) {}
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    fn refresh_grab_focus_location(
        &self,
        grab_focus: &mut Option<(PointerFocusTarget, Point<f64, Logical>)>,
        current_focus: Option<&(PointerFocusTarget, Point<f64, Logical>)>,
    ) {
        if let Some((target, loc)) = grab_focus.as_mut() {
            if let Some(current) = self.location_for_pointer_focus(target) {
                *loc = current;
            } else if let Some((new_target, new_location)) = current_focus
                && *new_target == *target
            {
                *loc = *new_location;
            }
        }
    }

    fn location_for_pointer_focus(&self, focus: &PointerFocusTarget) -> Option<Point<f64, Logical>> {
        let surface = focus.wl_surface()?;
        self.core
            .workspace_manager
            .outputs()
            .find_map(|output| {
                let output_loc = self.core.workspace_manager.output_geometry(output)?.loc;
                let map = layer_map_for_output(output);
                let layer = map.layer_for_surface(&surface, WindowSurfaceType::TOPLEVEL)?;
                Some((output_loc + map.layer_geometry(layer)?.loc).to_f64())
            })
            .or_else(|| {
                let window = self.window_for_surface(&surface)?;
                let mut loc = self.core.workspace_manager.window_location(&window)?;
                loc -= SpaceElement::geometry(&window).loc;
                if let Some(decorations) = window.decoration_state().window_decorations() {
                    loc += decorations.decorations_offset();
                }
                Some(loc.to_f64())
            })
    }
}
