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
    backend::renderer::{
        ImportMem, Renderer, RendererSuper,
        element::{
            AsRenderElements, Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
        },
    },
    utils::{Logical, Physical, Point, Rectangle, Scale, Size},
};

pub static FPS_NUMBERS_PNG: &[u8] = include_bytes!("../../../resources/numbers.png");

#[derive(Debug, Clone)]
pub struct FpsElement {
    value: u32,
    buffer: MemoryRenderBuffer,
}

impl FpsElement {
    pub fn new(buffer: &MemoryRenderBuffer) -> Self {
        FpsElement {
            buffer: buffer.clone(),
            value: 0,
        }
    }

    pub fn update_fps(&mut self, fps: u32) {
        self.value = fps;
    }
}

impl<R> AsRenderElements<R> for FpsElement
where
    R: Renderer + ImportMem,
    <R as RendererSuper>::TextureId: Clone + Send + 'static,
{
    type RenderElement = MemoryRenderBufferRenderElement<R>;

    fn render_elements<C: From<Self::RenderElement>>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        _alpha: f32,
    ) -> Vec<C> {
        let value_str = std::cmp::min(self.value, 999).to_string();
        value_str
            .chars()
            .filter_map(|d| d.to_digit(10))
            .enumerate()
            .filter_map(|(i, digit)| {
                let offset = Point::<f64, Physical>::from((i as f64 * 24.0 * scale.x, 0.0));
                let digit_location = location.to_f64() + offset;
                MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    digit_location,
                    &self.buffer,
                    None,
                    Some(digit_src(digit)),
                    None,
                    Kind::Unspecified,
                )
                .ok()
                .map(C::from)
            })
            .collect()
    }
}

fn digit_src(digit: u32) -> Rectangle<f64, Logical> {
    let (x, y) = match digit {
        9 => (0, 0),
        6 => (22, 0),
        3 => (44, 0),
        1 => (66, 0),
        8 => (0, 35),
        0 => (22, 35),
        2 => (44, 35),
        7 => (0, 70),
        4 => (22, 70),
        5 => (44, 70),
        _ => unreachable!(),
    };
    Rectangle::new((x, y).into(), Size::from((22, 35))).to_f64()
}
