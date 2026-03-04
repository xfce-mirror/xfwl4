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
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::{Size, Transform},
};
use tracing::warn;

mod fps_element;

pub use fps_element::{FPS_NUMBERS_PNG, FpsElement};

#[derive(Debug)]
pub struct BackendDebug {
    fps_buffer: MemoryRenderBuffer,
}

impl BackendDebug {
    pub fn new() -> Option<Self> {
        #[allow(deprecated)]
        let fps_image = image::io::Reader::with_format(std::io::Cursor::new(FPS_NUMBERS_PNG), image::ImageFormat::Png)
            .decode()
            .inspect_err(|err| warn!("Failed to decode FPS texture image: {err}"))
            .ok()?;

        let fps_buffer = MemoryRenderBuffer::from_slice(
            &fps_image.to_rgba8(),
            Fourcc::Abgr8888,
            Size::new(fps_image.width() as i32, fps_image.height() as i32),
            1,
            Transform::Normal,
            None,
        );

        Some(Self { fps_buffer })
    }
}

#[derive(Debug)]
pub struct RenderDebug {
    fps: fps_ticker::Fps,
    fps_element: FpsElement,
}

impl RenderDebug {
    pub fn new(backend_debug: &BackendDebug) -> Self {
        Self {
            fps: fps_ticker::Fps::default(),
            fps_element: FpsElement::new(&backend_debug.fps_buffer),
        }
    }

    pub fn update(&mut self) {
        self.fps_element.update_fps(self.fps.avg().round() as u32);
        self.fps.tick();
    }

    pub fn fps_element(&self) -> &FpsElement {
        &self.fps_element
    }
}
