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

use std::sync::{Arc, Mutex};

use smithay::{
    backend::input::KeyState,
    input::{
        SeatHandler,
        keyboard::{GrabStartData as KeyboardGrabStartData, KeyboardGrab, KeyboardInnerHandle, ModifiersState},
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
            GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
            GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
        },
        touch::{GrabStartData as TouchGrabStartData, TouchGrab},
    },
    utils::{Logical, Point, SERIAL_COUNTER, Serial},
};
use xkbcommon::xkb::Keycode;

use crate::{
    backend::Backend,
    core::{
        cursor::CursorName,
        focus::PointerFocusTarget,
        shell::{
            WindowElement,
            grabs::common::{MoveResizeAction, keyboard_move_resize_get_action},
        },
        state::Xfwl4State,
    },
};

const KEY_MOVE_BASE: i32 = 16;

pub(super) struct SharedMoveState {
    pub(super) window: WindowElement,
    pub(super) initial_window_location: Point<i32, Logical>,
    pub(super) pointer_start_location: Point<f64, Logical>,
    pub(super) pointer_start_window_location: Point<i32, Logical>,
    pub(super) button_pressed: bool,
    pub(super) finished: bool,
    pub(super) skip_next_pointer_motion: bool,
}

pub(super) fn warp_pointer_to_window_center<BackendData: Backend>(
    data: &mut Xfwl4State<BackendData>,
    window: &WindowElement,
    window_location: Point<i32, Logical>,
) -> Point<f64, Logical> {
    let geometry = data
        .core
        .wireframe
        .as_ref()
        .map(|wireframe| wireframe.geometry())
        .unwrap_or_else(|| {
            let workspace = data.core.workspace_manager.active_workspace_mut();
            workspace.element_geometry(window).unwrap_or_default()
        });

    let size = geometry.size;
    let location: Point<f64, Logical> = ((window_location.x + size.w / 2) as f64, (window_location.y + size.h / 2) as f64).into();

    let pointer = data.core.pointer.clone();
    let event = MotionEvent {
        location,
        serial: SERIAL_COUNTER.next_serial(),
        time: data.core.now().as_millis(),
    };
    pointer.motion(data, None, &event);
    location
}

fn finish_move_cleanup<BackendData: Backend>(state: &mut SharedMoveState, data: &mut Xfwl4State<BackendData>) {
    state.finished = true;
    state.window.set_moving_state(false);
    data.core.set_cursor(CursorName::Default);
    data.core.wireframe = None;
}

// -- Pointer move grab --

pub struct PointerMoveSurfaceGrab<BackendData: Backend + 'static> {
    pub(super) start_data: PointerGrabStartData<Xfwl4State<BackendData>>,
    pub(super) state: Arc<Mutex<SharedMoveState>>,
}

impl<BackendData: Backend> PointerGrab<Xfwl4State<BackendData>> for PointerMoveSurfaceGrab<BackendData> {
    fn motion(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        _focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);

        let mut state = self.state.lock().unwrap();
        if !state.finished {
            // Ensure that xfwl4's cursor takes precedece over anything the client tries to set.
            data.core.cursor_status = smithay::input::pointer::CursorImageStatus::default_named();

            if state.skip_next_pointer_motion {
                state.skip_next_pointer_motion = false;
                state.pointer_start_location = event.location;
            } else {
                let delta = event.location - state.pointer_start_location;
                let new_location = state.pointer_start_window_location.to_f64() + delta;
                let window = state.window.clone();
                drop(state);

                if let Some(wireframe) = data.core.wireframe.as_mut() {
                    wireframe.update_location(new_location.to_i32_round());
                } else {
                    data.core
                        .workspace_manager
                        .active_workspace_mut()
                        .map_element(window, new_location.to_i32_round(), true);
                }
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

        let mut state = self.state.lock().unwrap();
        if !state.finished {
            if !handle.current_pressed().is_empty() {
                state.button_pressed = true;
            } else if state.button_pressed {
                if let Some(wireframe) = data.core.wireframe.as_ref() {
                    data.core
                        .workspace_manager
                        .active_workspace_mut()
                        .map_element(state.window.clone(), wireframe.geometry().loc, true);
                }

                finish_move_cleanup(&mut state, data);
                drop(state);
                handle.unset_grab(self, data, event.serial, event.time, true);
                let seat = data.core.seat.clone();
                if let Some(keyboard) = seat.get_keyboard() {
                    keyboard.unset_grab(data);
                }
                if let Some(touch) = seat.get_touch() {
                    touch.unset_grab(data);
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

    fn unset(&mut self, data: &mut Xfwl4State<BackendData>) {
        let mut state = self.state.lock().unwrap();
        if !state.finished {
            finish_move_cleanup(&mut state, data);
            drop(state);
            let seat = data.core.seat.clone();
            if let Some(keyboard) = seat.get_keyboard() {
                keyboard.unset_grab(data);
            }
            if let Some(touch) = seat.get_touch() {
                touch.unset_grab(data);
            }
        }
    }
}

// -- Touch move grab --

pub struct TouchMoveSurfaceGrab<BackendData: Backend + 'static> {
    pub(super) start_data: TouchGrabStartData<Xfwl4State<BackendData>>,
    pub(super) state: Arc<Mutex<SharedMoveState>>,
}

impl<BackendData: Backend> TouchGrab<Xfwl4State<BackendData>> for TouchMoveSurfaceGrab<BackendData> {
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
        seq: Serial,
    ) {
        if event.slot != self.start_data.slot {
            return;
        }

        let mut state = self.state.lock().unwrap();
        if !state.finished {
            if let Some(wireframe) = data.core.wireframe.as_ref() {
                data.core
                    .workspace_manager
                    .active_workspace_mut()
                    .map_element(state.window.clone(), wireframe.geometry().loc, true);
            }

            finish_move_cleanup(&mut state, data);
            drop(state);
            handle.up(data, event, seq);
            handle.unset_grab(self, data);
            let pointer = data.core.pointer.clone();
            pointer.unset_grab(data, SERIAL_COUNTER.next_serial(), data.core.clock.now().as_millis());
            let seat = data.core.seat.clone();
            if let Some(keyboard) = seat.get_keyboard() {
                keyboard.unset_grab(data);
            }
        }
    }

    fn motion(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        _handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
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

        let state = self.state.lock().unwrap();
        if !state.finished {
            let delta = event.location - state.pointer_start_location;
            let new_location = state.pointer_start_window_location.to_f64() + delta;
            let window = state.window.clone();
            drop(state);

            if let Some(wireframe) = data.core.wireframe.as_mut() {
                wireframe.update_location(new_location.to_i32_round());
            } else {
                data.core
                    .workspace_manager
                    .active_workspace_mut()
                    .map_element(window, new_location.to_i32_round(), true);
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

    fn unset(&mut self, data: &mut Xfwl4State<BackendData>) {
        let mut state = self.state.lock().unwrap();
        if !state.finished {
            finish_move_cleanup(&mut state, data);
            drop(state);
            let pointer = data.core.pointer.clone();
            pointer.unset_grab(data, SERIAL_COUNTER.next_serial(), data.core.clock.now().as_millis());
            let seat = data.core.seat.clone();
            if let Some(keyboard) = seat.get_keyboard() {
                keyboard.unset_grab(data);
            }
        }
    }
}

// -- Keyboard move grab --

pub struct KeyboardMoveSurfaceGrab<BackendData: Backend + 'static> {
    pub(super) start_data: KeyboardGrabStartData<Xfwl4State<BackendData>>,
    pub(super) state: Arc<Mutex<SharedMoveState>>,
}

impl<BackendData: Backend + 'static> KeyboardGrab<Xfwl4State<BackendData>> for KeyboardMoveSurfaceGrab<BackendData> {
    fn input(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut KeyboardInnerHandle<'_, Xfwl4State<BackendData>>,
        keycode: Keycode,
        key_state: KeyState,
        _modifiers: Option<ModifiersState>,
        serial: Serial,
        _time: u32,
    ) {
        {
            let state = self.state.lock().unwrap();
            if state.finished {
                return;
            }
        }

        if let Some(action) = keyboard_move_resize_get_action(data, handle, keycode, key_state) {
            let key_move = if data.core.config.snap_to_border() || data.core.config.snap_to_windows() {
                KEY_MOVE_BASE.max(data.core.config.snap_width() + 1)
            } else {
                KEY_MOVE_BASE
            };

            let reposition = match action {
                MoveResizeAction::Left | MoveResizeAction::Right | MoveResizeAction::Up | MoveResizeAction::Down => {
                    let delta: Point<i32, Logical> = match action {
                        MoveResizeAction::Left => (-key_move, 0).into(),
                        MoveResizeAction::Right => (key_move, 0).into(),
                        MoveResizeAction::Up => (0, -key_move).into(),
                        MoveResizeAction::Down => (0, key_move).into(),
                        _ => unreachable!(),
                    };
                    let (window, new_loc) = {
                        let state = self.state.lock().unwrap();
                        let current_loc = data
                            .core
                            .wireframe
                            .as_ref()
                            .map(|wireframe| wireframe.geometry().loc)
                            .unwrap_or_else(|| {
                                data.core
                                    .workspace_manager
                                    .active_workspace()
                                    .element_location(&state.window)
                                    .unwrap_or(state.pointer_start_window_location)
                            });
                        (state.window.clone(), current_loc + delta)
                    };

                    if let Some(wireframe) = data.core.wireframe.as_mut() {
                        wireframe.update_location(new_loc);
                    } else {
                        data.core
                            .workspace_manager
                            .active_workspace_mut()
                            .map_element(window, new_loc, false);
                    }

                    {
                        let mut state = self.state.lock().unwrap();
                        state.pointer_start_window_location = new_loc;
                        state.skip_next_pointer_motion = true;
                    }
                    true
                }

                MoveResizeAction::Finish => {
                    {
                        let mut state = self.state.lock().unwrap();

                        if let Some(wireframe) = data.core.wireframe.as_ref() {
                            data.core.workspace_manager.active_workspace_mut().map_element(
                                state.window.clone(),
                                wireframe.geometry().loc,
                                true,
                            );
                        }

                        finish_move_cleanup(&mut state, data);
                    }

                    handle.unset_grab(self, data, serial, true);
                    let pointer = data.core.pointer.clone();
                    pointer.unset_grab(data, serial, data.core.clock.now().as_millis());
                    if let Some(touch) = data.core.seat.clone().get_touch() {
                        touch.unset_grab(data);
                    }
                    false
                }

                MoveResizeAction::Cancel => {
                    let (window, initial_loc) = {
                        let state = self.state.lock().unwrap();
                        (state.window.clone(), state.initial_window_location)
                    };

                    if data.core.wireframe.is_none() {
                        data.core
                            .workspace_manager
                            .active_workspace_mut()
                            .map_element(window, initial_loc, false);
                    }

                    {
                        let mut state = self.state.lock().unwrap();
                        state.pointer_start_window_location = initial_loc;
                        state.skip_next_pointer_motion = true;
                        finish_move_cleanup(&mut state, data);
                    }
                    handle.unset_grab(self, data, serial, true);
                    let pointer = data.core.pointer.clone();
                    pointer.unset_grab(data, serial, data.core.clock.now().as_millis());
                    if let Some(touch) = data.core.seat.clone().get_touch() {
                        touch.unset_grab(data);
                    }
                    true
                }
            };

            if reposition {
                let (window, window_location) = {
                    let state = self.state.lock().unwrap();
                    (state.window.clone(), state.pointer_start_window_location)
                };
                let warp_target = warp_pointer_to_window_center(data, &window, window_location);
                {
                    let mut state = self.state.lock().unwrap();
                    state.pointer_start_location = warp_target;
                }
            }
        }
    }

    fn set_focus(
        &mut self,
        _data: &mut Xfwl4State<BackendData>,
        _handle: &mut KeyboardInnerHandle<'_, Xfwl4State<BackendData>>,
        _focus: Option<<Xfwl4State<BackendData> as SeatHandler>::KeyboardFocus>,
        _serial: Serial,
    ) {
    }

    fn unset(&mut self, data: &mut Xfwl4State<BackendData>) {
        let mut state = self.state.lock().unwrap();
        if !state.finished {
            finish_move_cleanup(&mut state, data);
            drop(state);
            let pointer = data.core.pointer.clone();
            pointer.unset_grab(data, SERIAL_COUNTER.next_serial(), data.core.clock.now().as_millis());
            let seat = data.core.seat.clone();
            if let Some(touch) = seat.get_touch() {
                touch.unset_grab(data);
            }
        }
    }

    fn start_data(&self) -> &KeyboardGrabStartData<Xfwl4State<BackendData>> {
        &self.start_data
    }
}
