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
            AxisFrame, ButtonEvent, CursorIcon, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
            GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
            GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
        },
        touch::{GrabStartData as TouchGrabStartData, TouchGrab},
    },
    utils::{Logical, Point, SERIAL_COUNTER, Serial},
};
use xkbcommon::xkb::Keycode;

use smithay::desktop::{layer_map_for_output, space::SpaceElement};
use smithay::utils::Rectangle;

use crate::{
    backend::Backend,
    core::{
        focus::PointerFocusTarget,
        shell::{
            TileZone, WindowElement,
            grabs::common::{MoveResizeAction, keyboard_move_resize_get_action},
            tile_zone_for_pointer,
        },
        snap,
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

pub(in crate::core) struct ActiveMoveGrab {
    shared: Arc<Mutex<SharedMoveState>>,
}

impl ActiveMoveGrab {
    pub(in crate::core) fn window(&self) -> WindowElement {
        self.shared.lock().unwrap().window.clone()
    }

    /// Reset the stored starting location of the grab after an external pointer warp and/or window
    /// relocation (e.g. cross-edge workspace switch).  Subsequent motion events will be treated as
    /// deltas from the provided pointer and window positions.
    pub(in crate::core) fn reset_location_after_warp(
        &self,
        new_pointer_location: Point<f64, Logical>,
        new_window_location: Point<i32, Logical>,
    ) {
        let mut state = self.shared.lock().unwrap();
        state.pointer_start_location = new_pointer_location;
        state.pointer_start_window_location = new_window_location;
        state.skip_next_pointer_motion = true;
    }
}

impl From<Arc<Mutex<SharedMoveState>>> for ActiveMoveGrab {
    fn from(shared: Arc<Mutex<SharedMoveState>>) -> Self {
        Self { shared }
    }
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
            workspace.window_geometry(window).unwrap_or_default()
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
    data.core.set_cursor(CursorIcon::Default);
    data.core.wireframe = None;
    data.core.active_move_grab = None;
}

struct SnapGeometries {
    border_rects: Vec<Rectangle<i32, Logical>>,
    window_rects: Vec<Rectangle<i32, Logical>>,
}

fn collect_snap_geometries<BackendData: Backend>(
    data: &Xfwl4State<BackendData>,
    window: &WindowElement,
    snap_to_border: bool,
    snap_to_windows: bool,
) -> SnapGeometries {
    let outputs: Vec<_> = data.core.workspace_manager.outputs().cloned().collect();

    let border_rects = if snap_to_border {
        outputs
            .iter()
            .filter_map(|o| {
                let geo = data.core.workspace_manager.output_geometry(o)?;
                let zone = layer_map_for_output(o).non_exclusive_zone();
                Some(Rectangle::new(geo.loc + zone.loc, zone.size))
            })
            .collect()
    } else {
        Vec::new()
    };

    let window_rects = if snap_to_windows {
        let workspace = data.core.workspace_manager.active_workspace();
        let mut rects: Vec<_> = workspace
            .visible_windows()
            .filter(|w| *w != window)
            .filter_map(|w| workspace.window_geometry(w))
            .collect();

        for output in &outputs {
            let geo = data.core.workspace_manager.output_geometry(output);
            if let Some(geo) = geo {
                let layer_map = layer_map_for_output(output);
                rects.extend(layer_map.layers().filter_map(|surface| {
                    let layer_geo = layer_map.layer_geometry(surface)?;
                    Some(Rectangle::new(geo.loc + layer_geo.loc, layer_geo.size))
                }));
            }
        }

        rects
    } else {
        Vec::new()
    };

    SnapGeometries {
        border_rects,
        window_rects,
    }
}

fn handle_move_motion<BackendData: Backend>(
    data: &mut Xfwl4State<BackendData>,
    window: &WindowElement,
    pointer: Point<f64, Logical>,
    new_location: Point<i32, Logical>,
) {
    let zone = if data.core.config.tile_on_move() && !data.core.config.wrap_windows() {
        data.core
            .workspace_manager
            .output_under(pointer)
            .next()
            .and_then(|output| data.core.workspace_manager.output_geometry(output))
            .and_then(|geom| tile_zone_for_pointer(pointer, geom))
    } else {
        None
    };

    match zone {
        Some(TileZone::Tile(mode)) => {
            if window.tile_mode() != Some(mode) {
                data.set_window_tiled(window, mode, Some(pointer));
            }
        }
        Some(TileZone::Maximize) => {
            if !window.maximized() {
                data.set_window_maximized(window, Some(pointer));
            }
        }
        None => {
            if window.tile_mode().is_some() {
                data.set_window_untiled(window, Some(new_location));
            } else if window.maximized() {
                data.set_window_unmaximized(window, Some(new_location));
            } else {
                apply_move_location(data, window, new_location, true);
            }
        }
    }
}

fn apply_move_location<BackendData: Backend>(
    data: &mut Xfwl4State<BackendData>,
    window: &WindowElement,
    new_location: Point<i32, Logical>,
    activate: bool,
) {
    let snap_to_border = data.core.config.snap_to_border();
    let snap_to_windows = data.core.config.snap_to_windows();
    let snapped = if snap_to_border || snap_to_windows {
        let frame_size = window.geometry().size;
        let snap_width = data.core.config.snap_width();
        let SnapGeometries {
            border_rects,
            window_rects,
        } = collect_snap_geometries(data, window, snap_to_border, snap_to_windows);

        let prev = if data.core.config.snap_resist() {
            data.core
                .wireframe
                .as_ref()
                .map(|wf| wf.geometry().loc)
                .or_else(|| data.core.workspace_manager.active_workspace().window_location(window))
        } else {
            None
        };

        let after_border = if snap_to_border {
            snap::snap_move_to_border(new_location, prev, frame_size, &border_rects, snap_width)
        } else {
            new_location
        };

        if snap_to_windows {
            snap::snap_move_to_windows(after_border, prev, frame_size, &window_rects, snap_width)
        } else {
            after_border
        }
    } else {
        new_location
    };

    if let Some(wireframe) = data.core.wireframe.as_mut() {
        wireframe.update_location(snapped);
    } else {
        data.core.workspace_manager.relocate_window(window, snapped, activate);
    }
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
            // Ensure that xfwl4's cursor takes precedence over anything the client tries to set.
            data.core.cursor_status = smithay::input::pointer::CursorImageStatus::default_named();
            data.core.set_cursor(CursorIcon::AllResize);

            if state.skip_next_pointer_motion {
                state.skip_next_pointer_motion = false;
                state.pointer_start_location = event.location;
            } else {
                let delta = event.location - state.pointer_start_location;
                let new_location = (state.pointer_start_window_location.to_f64() + delta).to_i32_round();
                let window = state.window.clone();
                drop(state);

                handle_move_motion(data, &window, event.location, new_location);
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
                        .relocate_window(&state.window, wireframe.geometry().loc, true);
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

                let location = handle.current_location();
                let focus = data.surface_under(location);
                handle.motion(
                    data,
                    focus,
                    &MotionEvent {
                        location,
                        serial: SERIAL_COUNTER.next_serial(),
                        time: data.core.clock.now().as_millis(),
                    },
                );
                handle.frame(data);
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
                    .relocate_window(&state.window, wireframe.geometry().loc, true);
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
            let new_location = (state.pointer_start_window_location.to_f64() + delta).to_i32_round();
            let window = state.window.clone();
            drop(state);

            handle_move_motion(data, &window, event.location, new_location);
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
            match action {
                MoveResizeAction::Left | MoveResizeAction::Right | MoveResizeAction::Up | MoveResizeAction::Down => {
                    let delta: Point<f64, Logical> = match action {
                        MoveResizeAction::Left => (-(KEY_MOVE_BASE as f64), 0.0).into(),
                        MoveResizeAction::Right => (KEY_MOVE_BASE as f64, 0.0).into(),
                        MoveResizeAction::Up => (0.0, -(KEY_MOVE_BASE as f64)).into(),
                        MoveResizeAction::Down => (0.0, KEY_MOVE_BASE as f64).into(),
                        _ => unreachable!(),
                    };

                    let pointer = data.core.pointer.clone();
                    let location = pointer.current_location() + delta;
                    pointer.motion(
                        data,
                        None,
                        &MotionEvent {
                            location,
                            serial: SERIAL_COUNTER.next_serial(),
                            time: data.core.clock.now().as_millis(),
                        },
                    );
                    pointer.frame(data);
                }

                MoveResizeAction::Finish => {
                    {
                        let mut state = self.state.lock().unwrap();

                        if let Some(wireframe) = data.core.wireframe.as_ref() {
                            data.core
                                .workspace_manager
                                .relocate_window(&state.window, wireframe.geometry().loc, true);
                        }

                        finish_move_cleanup(&mut state, data);
                    }

                    handle.unset_grab(self, data, serial, true);
                    let pointer = data.core.pointer.clone();
                    pointer.unset_grab(data, serial, data.core.clock.now().as_millis());
                    if let Some(touch) = data.core.seat.clone().get_touch() {
                        touch.unset_grab(data);
                    }
                }

                MoveResizeAction::Cancel => {
                    let (window, initial_loc) = {
                        let state = self.state.lock().unwrap();
                        (state.window.clone(), state.initial_window_location)
                    };

                    if data.core.wireframe.is_none() {
                        data.core.workspace_manager.relocate_window(&window, initial_loc, false);
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
                }
            };
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
