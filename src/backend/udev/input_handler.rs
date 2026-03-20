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

use smithay::{
    backend::{
        input::{
            AbsolutePositionEvent, Event, GestureBeginEvent, GestureEndEvent, GesturePinchUpdateEvent as _, GestureSwipeUpdateEvent as _,
            InputEvent, KeyboardKeyEvent, PointerButtonEvent, PointerMotionEvent, Switch, SwitchState, TabletToolEvent,
            TabletToolProximityEvent, TabletToolTipState, TouchEvent,
        },
        libinput::LibinputInputBackend,
    },
    reexports::input::{
        DeviceCapability as LibinputDeviceCapability,
        event::{
            switch::{Switch as LibinputSwitch, SwitchState as LibinputSwitchState},
            tablet_tool::TipState as LibinputTipState,
        },
    },
    utils::Size,
    wayland::tablet_manager::TabletDescriptor,
};

use crate::{
    backend::{
        DeviceCapabilities, KeyboardInputEvent, PointerInputEvent, SwitchInputEvent, TabletInputEvent, TabletToolAxisData,
        TabletToolButtonData, TabletToolProximityData, TabletToolTipData, TouchInputEvent, TranslatedInput, build_axis_frame,
        udev::UdevData,
    },
    core::config::PointerConfig,
};

impl UdevData {
    pub(crate) fn translate_input_event(&mut self, event: InputEvent<LibinputInputBackend>) -> Option<TranslatedInput> {
        match event {
            InputEvent::DeviceAdded { device } => {
                let mut caps = DeviceCapabilities {
                    has_keyboard: false,
                    has_pointer: false,
                    has_touch: false,
                    tablet_descriptor: None,
                };

                if device.has_capability(LibinputDeviceCapability::Keyboard) {
                    caps.has_keyboard = true;
                    self.keyboards.push(device.clone());
                }

                if device.has_capability(LibinputDeviceCapability::Pointer) || device.has_capability(LibinputDeviceCapability::Touch) {
                    caps.has_pointer = device.has_capability(LibinputDeviceCapability::Pointer);
                    caps.has_touch = device.has_capability(LibinputDeviceCapability::Touch);
                    let config = PointerConfig::new(device.clone());
                    self.pointers.push((device.clone(), config));
                }

                if device.has_capability(LibinputDeviceCapability::TabletTool) {
                    caps.tablet_descriptor = Some(TabletDescriptor::from(&device));
                }

                Some(TranslatedInput::DeviceAdded(caps))
            }

            InputEvent::DeviceRemoved { ref device } => {
                let mut caps = DeviceCapabilities {
                    has_keyboard: false,
                    has_pointer: false,
                    has_touch: false,
                    tablet_descriptor: None,
                };

                if device.has_capability(LibinputDeviceCapability::Keyboard) {
                    caps.has_keyboard = true;
                    self.keyboards.retain(|item| item != device);
                }

                if device.has_capability(LibinputDeviceCapability::Pointer) || device.has_capability(LibinputDeviceCapability::Touch) {
                    caps.has_pointer = device.has_capability(LibinputDeviceCapability::Pointer);
                    caps.has_touch = device.has_capability(LibinputDeviceCapability::Touch);
                    self.pointers.retain(|(item, _)| item != device);
                }

                if device.has_capability(LibinputDeviceCapability::TabletTool) {
                    caps.tablet_descriptor = Some(TabletDescriptor::from(device));
                }

                Some(TranslatedInput::DeviceRemoved(caps))
            }

            InputEvent::Keyboard { event } => Some(TranslatedInput::Keyboard(KeyboardInputEvent::Key {
                keycode: event.key_code().into(),
                state: event.state(),
                time: event.time_msec(),
            })),

            InputEvent::PointerMotion { event } => Some(TranslatedInput::Pointer(PointerInputEvent::MotionRelative {
                delta: event.delta(),
                delta_unaccel: event.delta_unaccel(),
                utime: event.time(),
            })),

            InputEvent::PointerMotionAbsolute { event } => Some(TranslatedInput::Pointer(PointerInputEvent::MotionAbsolute {
                position: event.position_transformed(Size::from((1, 1))),
                time: event.time_msec(),
            })),

            InputEvent::PointerButton { event } => Some(TranslatedInput::Pointer(PointerInputEvent::Button {
                button: event.button_code(),
                state: event.state(),
                time: event.time_msec(),
            })),

            InputEvent::PointerAxis { event } => Some(TranslatedInput::Pointer(PointerInputEvent::Axis {
                frame: build_axis_frame::<LibinputInputBackend>(&event),
            })),

            InputEvent::TabletToolProximity { event } => {
                Some(TranslatedInput::Tablet(TabletInputEvent::ToolProximity(TabletToolProximityData {
                    descriptor: event.tool(),
                    tablet: TabletDescriptor::from(&event.device()),
                    state: event.state(),
                    position: event.position_transformed(Size::from((1, 1))),
                    time: event.time_msec(),
                })))
            }

            InputEvent::TabletToolAxis { event } => {
                let pressure = event.pressure_has_changed().then(|| event.pressure());
                let distance = event.distance_has_changed().then(|| event.distance());
                let tilt = event.tilt_has_changed().then(|| event.tilt());
                let slider = event.slider_has_changed().then(|| event.slider_position());
                let rotation = event.rotation_has_changed().then(|| event.rotation());
                let wheel = event
                    .wheel_has_changed()
                    .then(|| (event.wheel_delta(), event.wheel_delta_discrete()));

                Some(TranslatedInput::Tablet(TabletInputEvent::ToolAxis(TabletToolAxisData {
                    descriptor: event.tool(),
                    tablet: TabletDescriptor::from(&event.device()),
                    position: event.position_transformed(Size::from((1, 1))),
                    pressure,
                    distance,
                    tilt,
                    slider,
                    rotation,
                    wheel,
                    time: event.time_msec(),
                })))
            }

            InputEvent::TabletToolTip { event } => Some(TranslatedInput::Tablet(TabletInputEvent::ToolTip(TabletToolTipData {
                descriptor: event.tool(),
                position: event.position_transformed(Size::from((1, 1))),
                tip_state: match event.tip_state() {
                    LibinputTipState::Up => TabletToolTipState::Up,
                    LibinputTipState::Down => TabletToolTipState::Down,
                },
                time: event.time_msec(),
            }))),

            InputEvent::TabletToolButton { event } => Some(TranslatedInput::Tablet(TabletInputEvent::ToolButton(TabletToolButtonData {
                descriptor: event.tool(),
                button: event.button(),
                state: event.button_state().into(),
                time: event.time_msec(),
            }))),

            InputEvent::GestureSwipeBegin { event } => Some(TranslatedInput::Pointer(PointerInputEvent::GestureSwipeBegin {
                time: event.time_msec(),
                fingers: event.fingers(),
            })),

            InputEvent::GestureSwipeUpdate { event } => Some(TranslatedInput::Pointer(PointerInputEvent::GestureSwipeUpdate {
                time: event.time_msec(),
                delta: event.delta(),
            })),

            InputEvent::GestureSwipeEnd { event } => Some(TranslatedInput::Pointer(PointerInputEvent::GestureSwipeEnd {
                time: event.time_msec(),
                cancelled: event.cancelled(),
            })),

            InputEvent::GesturePinchBegin { event } => Some(TranslatedInput::Pointer(PointerInputEvent::GesturePinchBegin {
                time: event.time_msec(),
                fingers: event.fingers(),
            })),

            InputEvent::GesturePinchUpdate { event } => Some(TranslatedInput::Pointer(PointerInputEvent::GesturePinchUpdate {
                time: event.time_msec(),
                delta: event.delta(),
                scale: event.scale(),
                rotation: event.rotation(),
            })),

            InputEvent::GesturePinchEnd { event } => Some(TranslatedInput::Pointer(PointerInputEvent::GesturePinchEnd {
                time: event.time_msec(),
                cancelled: event.cancelled(),
            })),

            InputEvent::GestureHoldBegin { event } => Some(TranslatedInput::Pointer(PointerInputEvent::GestureHoldBegin {
                time: event.time_msec(),
                fingers: event.fingers(),
            })),

            InputEvent::GestureHoldEnd { event } => Some(TranslatedInput::Pointer(PointerInputEvent::GestureHoldEnd {
                time: event.time_msec(),
                cancelled: event.cancelled(),
            })),

            InputEvent::TouchDown { event } => Some(TranslatedInput::Touch(TouchInputEvent::Down {
                slot: event.slot(),
                position: event.position_transformed(Size::from((1, 1))),
                time: event.time_msec(),
            })),

            InputEvent::TouchUp { event } => Some(TranslatedInput::Touch(TouchInputEvent::Up {
                slot: event.slot(),
                time: event.time_msec(),
            })),

            InputEvent::TouchMotion { event } => Some(TranslatedInput::Touch(TouchInputEvent::Motion {
                slot: event.slot(),
                position: event.position_transformed(Size::from((1, 1))),
                time: event.time_msec(),
            })),

            InputEvent::TouchFrame { .. } => Some(TranslatedInput::Touch(TouchInputEvent::Frame)),

            InputEvent::TouchCancel { .. } => Some(TranslatedInput::Touch(TouchInputEvent::Cancel)),

            InputEvent::SwitchToggle { event } => match event.switch() {
                Some(LibinputSwitch::Lid) => Some(Switch::Lid),
                Some(LibinputSwitch::TabletMode) => Some(Switch::TabletMode),
                _ => None,
            }
            .map(|switch| {
                TranslatedInput::Switch(SwitchInputEvent {
                    switch,
                    state: match event.switch_state() {
                        LibinputSwitchState::On => SwitchState::On,
                        LibinputSwitchState::Off => SwitchState::Off,
                    },
                })
            }),

            InputEvent::Special(_) => None,
        }
    }
}
