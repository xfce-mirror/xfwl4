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
            Axis, AxisSource, ButtonState, Event, InputBackend, KeyState, PointerAxisEvent, ProximityState, TabletToolDescriptor,
            TabletToolTipState, TouchSlot,
        },
        renderer::{
            Bind, ExportMem, ImportAll, ImportDma, ImportMem, Offscreen, Renderer, RendererSuper, Texture,
            gles::{GlesError, GlesFrame, GlesRenderbuffer, GlesRenderer},
        },
    },
    input::{keyboard::LedState, pointer::AxisFrame},
    output::{Mode, Output},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point},
    wayland::tablet_manager::TabletDescriptor,
};

#[cfg(feature = "udev")]
pub mod udev;

pub enum TranslatedInput {
    Keyboard(KeyboardInputEvent),
    Pointer(PointerInputEvent),
    Touch(TouchInputEvent),
    Tablet(TabletInputEvent),
    DeviceAdded(DeviceCapabilities),
    DeviceRemoved(DeviceCapabilities),
}

pub enum KeyboardInputEvent {
    Key { keycode: u32, state: KeyState, time: u32 },
}

pub enum PointerInputEvent {
    MotionRelative {
        delta: Point<f64, Logical>,
        delta_unaccel: Point<f64, Logical>,
        utime: u64,
    },
    MotionAbsolute {
        position: Point<f64, Logical>,
        time: u32,
    },
    Button {
        button: u32,
        state: ButtonState,
        time: u32,
    },
    Axis {
        frame: AxisFrame,
    },
    GestureSwipeBegin {
        time: u32,
        fingers: u32,
    },
    GestureSwipeUpdate {
        time: u32,
        delta: Point<f64, Logical>,
    },
    GestureSwipeEnd {
        time: u32,
        cancelled: bool,
    },
    GesturePinchBegin {
        time: u32,
        fingers: u32,
    },
    GesturePinchUpdate {
        time: u32,
        delta: Point<f64, Logical>,
        scale: f64,
        rotation: f64,
    },
    GesturePinchEnd {
        time: u32,
        cancelled: bool,
    },
    GestureHoldBegin {
        time: u32,
        fingers: u32,
    },
    GestureHoldEnd {
        time: u32,
        cancelled: bool,
    },
}

pub enum TouchInputEvent {
    Down {
        slot: TouchSlot,
        position: Point<f64, Logical>,
        time: u32,
    },
    Up {
        slot: TouchSlot,
        time: u32,
    },
    Motion {
        slot: TouchSlot,
        position: Point<f64, Logical>,
        time: u32,
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
    pub time: u32,
}

pub struct TabletToolAxisData {
    pub descriptor: TabletToolDescriptor,
    pub tablet: TabletDescriptor,
    pub position: Point<f64, Logical>,
    pub pressure: Option<f64>,
    pub distance: Option<f64>,
    pub tilt: Option<(f64, f64)>,
    pub slider: Option<f64>,
    pub rotation: Option<f64>,
    pub wheel: Option<(f64, i32)>,
    pub time: u32,
}

pub struct TabletToolTipData {
    pub descriptor: TabletToolDescriptor,
    pub position: Point<f64, Logical>,
    pub tip_state: TabletToolTipState,
    pub time: u32,
}

pub struct TabletToolButtonData {
    pub descriptor: TabletToolDescriptor,
    pub button: u32,
    pub state: ButtonState,
    pub time: u32,
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

pub trait Backend {
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

    /// Asks the backend to apply a new output mode.  If `mode` is `None`, disable the output.
    ///
    /// Should return the mode that was set (if any).  (Useful in case the backend sets a similar,
    /// but not quite the same, mode than what was requested.)
    fn set_output_mode(&mut self, output: &Output, mode: Option<Mode>) -> anyhow::Result<Option<Mode>>;

    fn switch_vt(&mut self, num: i32);
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

    let mut frame = AxisFrame::new(event.time_msec()).source(event.source());
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
