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
            Axis, AxisSource, ButtonState, Event, InputBackend, InputTime, KeyState, PointerAxisEvent, ProximityState, Switch, SwitchState,
            TabletToolDescriptor, TabletToolEvent, TabletToolTipState, TouchSlot,
        },
        renderer::{
            Bind, ExportMem, ImportAll, ImportDma, ImportMem, Offscreen, Renderer, RendererSuper, Texture,
            gles::{GlesError, GlesFrame, GlesRenderbuffer, GlesRenderer},
        },
    },
    input::{
        keyboard::LedState,
        pointer::AxisFrame,
        tablet::{TabletDescriptor, tool::AxisFrame as TabletAxisFrame},
    },
    output::{Mode, Output},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point},
};

use crate::core::state::Xfwl4Core;

#[cfg(feature = "udev")]
pub mod udev;

pub enum TranslatedInput {
    Keyboard(KeyboardInputEvent),
    Pointer(PointerInputEvent),
    Touch(TouchInputEvent),
    Tablet(TabletInputEvent),
    Switch(SwitchInputEvent),
    DeviceAdded(DeviceCapabilities),
    DeviceRemoved(DeviceCapabilities),
}

pub enum KeyboardInputEvent {
    Key { keycode: u32, state: KeyState, time: InputTime },
}

pub enum PointerInputEvent {
    MotionRelative {
        delta: Point<f64, Logical>,
        delta_unaccel: Point<f64, Logical>,
        time: InputTime,
    },
    MotionAbsolute {
        position: Point<f64, Logical>,
        time: InputTime,
    },
    Button {
        button: u32,
        state: ButtonState,
        time: InputTime,
    },
    Axis {
        frame: AxisFrame,
    },
    GestureSwipeBegin {
        time: InputTime,
        fingers: u32,
    },
    GestureSwipeUpdate {
        time: InputTime,
        delta: Point<f64, Logical>,
    },
    GestureSwipeEnd {
        time: InputTime,
        cancelled: bool,
    },
    GesturePinchBegin {
        time: InputTime,
        fingers: u32,
    },
    GesturePinchUpdate {
        time: InputTime,
        delta: Point<f64, Logical>,
        scale: f64,
        rotation: f64,
    },
    GesturePinchEnd {
        time: InputTime,
        cancelled: bool,
    },
    GestureHoldBegin {
        time: InputTime,
        fingers: u32,
    },
    GestureHoldEnd {
        time: InputTime,
        cancelled: bool,
    },
}

pub enum TouchInputEvent {
    Down {
        slot: TouchSlot,
        position: Point<f64, Logical>,
        assigned_monitor: Option<String>,
        time: InputTime,
    },
    Up {
        slot: TouchSlot,
        time: InputTime,
    },
    Motion {
        slot: TouchSlot,
        position: Point<f64, Logical>,
        assigned_monitor: Option<String>,
        time: InputTime,
    },
    Frame,
    Cancel,
}

pub enum TabletInputEvent {
    ToolProximity(TabletToolProximityData),
    ToolAxis(TabletToolAxisData),
    ToolTip(TabletToolTipData),
    ToolButton(TabletToolButtonData),
}

pub struct TabletToolProximityData {
    pub descriptor: TabletToolDescriptor,
    pub tablet: TabletDescriptor,
    pub state: ProximityState,
    pub position: Point<f64, Logical>,
    pub axis: TabletAxisFrame,
    pub time: InputTime,
}

pub struct TabletToolAxisData {
    pub descriptor: TabletToolDescriptor,
    pub position: Point<f64, Logical>,
    pub axis: TabletAxisFrame,
    pub time: InputTime,
}

pub struct TabletToolTipData {
    pub descriptor: TabletToolDescriptor,
    pub position: Point<f64, Logical>,
    pub tip_state: TabletToolTipState,
    pub time: InputTime,
}

pub struct TabletToolButtonData {
    pub descriptor: TabletToolDescriptor,
    pub button: u32,
    pub state: ButtonState,
    pub time: InputTime,
}

pub struct SwitchInputEvent {
    pub switch: Switch,
    pub state: SwitchState,
}

pub struct DeviceCapabilities {
    pub has_keyboard: bool,
    pub has_pointer: bool,
    pub has_touch: bool,
    pub tablet_descriptor: Option<TabletDescriptor>,
}

#[cfg(feature = "winit")]
pub mod winit;
#[cfg(feature = "x11")]
pub mod x11;

pub trait AsGlesRenderer
where
    Self: Renderer,
{
    fn gles_renderer(&self) -> &GlesRenderer;
    fn gles_renderer_mut(&mut self) -> &mut GlesRenderer;
    fn gles_frame<'a, 'frame, 'buffer>(frame: &'a Self::Frame<'frame, 'buffer>) -> &'a GlesFrame<'frame, 'buffer>;
    fn gles_frame_mut<'a, 'frame, 'buffer>(frame: &'a mut Self::Frame<'frame, 'buffer>) -> &'a mut GlesFrame<'frame, 'buffer>;
}

impl AsGlesRenderer for GlesRenderer {
    fn gles_renderer(&self) -> &GlesRenderer {
        self
    }

    fn gles_renderer_mut(&mut self) -> &mut GlesRenderer {
        self
    }

    fn gles_frame<'a, 'frame, 'buffer>(frame: &'a Self::Frame<'frame, 'buffer>) -> &'a GlesFrame<'frame, 'buffer> {
        frame
    }

    fn gles_frame_mut<'a, 'frame, 'buffer>(frame: &'a mut Self::Frame<'frame, 'buffer>) -> &'a mut GlesFrame<'frame, 'buffer> {
        frame
    }
}

pub trait FromGlesError {
    fn from_gles_error(err: GlesError) -> Self;
}

impl FromGlesError for GlesError {
    fn from_gles_error(err: GlesError) -> Self {
        err
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackendType {
    #[cfg(feature = "udev")]
    Tty,
    #[cfg(feature = "winit")]
    Winit,
    #[cfg(feature = "x11")]
    X11,
}

pub trait Backend: Sized {
    const HAS_RELATIVE_MOTION: bool = false;
    const HAS_GESTURES: bool = false;

    type RendererError: std::error::Error + Send + Sync + 'static;
    type RendererTextureId: Texture + Clone + 'static;
    type Renderer<'a>: ExportMem
        + ImportAll
        + ImportDma
        + ImportMem
        + RendererSuper<Error = Self::RendererError, TextureId = Self::RendererTextureId>
        + Offscreen<GlesRenderbuffer>
        + Bind<GlesRenderbuffer>
        + AsMut<GlesRenderer>
    where
        Self: 'a;

    fn backend_type(&self) -> BackendType;
    fn seat_name(&self) -> String;
    fn reset_buffers(&mut self, output: &Output);
    fn early_import(&mut self, surface: &WlSurface);
    fn update_led_state(&mut self, led_state: LedState);

    fn renderer(&mut self, #[cfg(feature = "udev")] node: Option<smithay::backend::drm::DrmNode>) -> anyhow::Result<Self::Renderer<'_>>;
    fn renderer_for_output(&mut self, output: &Output) -> anyhow::Result<Self::Renderer<'_>>;

    #[cfg(any(feature = "udev", feature = "winit"))]
    fn dmabuf_constraints(
        &mut self,
        node: Option<smithay::backend::drm::DrmNode>,
    ) -> Option<smithay::wayland::image_copy_capture::DmabufConstraints>;

    /// Asks the backend to apply a new output mode, enabling the output if needed.
    ///
    /// Should return a boolean telling whether the output needed to be enabled, as well as the
    /// mode that was set (if any).  (Useful in case the backend sets a similar, but not quite the
    /// same, mode than what was requested.)
    fn set_output_mode(&mut self, core: &Xfwl4Core<Self>, output: &Output, mode: Mode) -> anyhow::Result<(bool, Mode)>;
    fn disable_output(&mut self, core: &Xfwl4Core<Self>, output: &Output) -> anyhow::Result<()>;

    fn schedule_render(&mut self, core: &Xfwl4Core<Self>, output: &Output);

    fn switch_vt(&mut self, num: i32);
}

pub(crate) fn build_tablet_axis_frame<B: InputBackend>(event: &impl TabletToolEvent<B>) -> TabletAxisFrame {
    TabletAxisFrame {
        pressure: event.pressure_has_changed().then(|| event.pressure()),
        distance: event.distance_has_changed().then(|| event.distance()),
        tilt: event.tilt_has_changed().then(|| event.tilt()),
        rotation: event.rotation_has_changed().then(|| event.rotation()),
        slider: event.slider_has_changed().then(|| event.slider_position()),
        wheel: event
            .wheel_has_changed()
            .then(|| (event.wheel_delta(), event.wheel_delta_discrete())),
    }
}

pub(crate) fn build_axis_frame<B: InputBackend>(event: &B::PointerAxisEvent) -> AxisFrame {
    let horizontal_amount = event
        .amount(Axis::Horizontal)
        .unwrap_or_else(|| event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.);
    let vertical_amount = event
        .amount(Axis::Vertical)
        .unwrap_or_else(|| event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.);
    let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal);
    let vertical_amount_discrete = event.amount_v120(Axis::Vertical);

    let mut frame = AxisFrame::new(event.time()).source(event.source());
    if horizontal_amount != 0.0 {
        frame = frame.relative_direction(Axis::Horizontal, event.relative_direction(Axis::Horizontal));
        frame = frame.value(Axis::Horizontal, horizontal_amount);
        if let Some(discrete) = horizontal_amount_discrete {
            frame = frame.v120(Axis::Horizontal, discrete as i32);
        }
    }
    if vertical_amount != 0.0 {
        frame = frame.relative_direction(Axis::Vertical, event.relative_direction(Axis::Vertical));
        frame = frame.value(Axis::Vertical, vertical_amount);
        if let Some(discrete) = vertical_amount_discrete {
            frame = frame.v120(Axis::Vertical, discrete as i32);
        }
    }
    if event.source() == AxisSource::Finger {
        if event.amount(Axis::Horizontal) == Some(0.0) {
            frame = frame.stop(Axis::Horizontal);
        }
        if event.amount(Axis::Vertical) == Some(0.0) {
            frame = frame.stop(Axis::Vertical);
        }
    }
    frame
}
