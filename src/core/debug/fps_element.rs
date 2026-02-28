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
        Frame, ImportAll, Renderer, Texture,
        element::{Element, Id, RenderElement},
        utils::CommitCounter,
    },
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};

pub static FPS_NUMBERS_PNG: &[u8] = include_bytes!("../../../resources/numbers.png");

#[derive(Debug, Clone)]
pub struct FpsElement<T: Texture> {
    id: Id,
    value: u32,
    texture: T,
    commit_counter: CommitCounter,
}

impl<T: Texture> FpsElement<T> {
    pub fn new(texture: T) -> Self {
        FpsElement {
            id: Id::new(),
            texture,
            value: 0,
            commit_counter: CommitCounter::default(),
        }
    }

    pub fn update_fps(&mut self, fps: u32) {
        if self.value != fps {
            self.value = fps;
            self.commit_counter.increment();
        }
    }
}

impl<T> Element for FpsElement<T>
where
    T: Texture + 'static,
{
    fn id(&self) -> &Id {
        &self.id
    }

    fn location(&self, _scale: Scale<f64>) -> Point<i32, Physical> {
        (0, 0).into()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        let digits = if self.value < 10 {
            1
        } else if self.value < 100 {
            2
        } else {
            3
        };
        Rectangle::from_size((24 * digits, 35).into()).to_f64()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        let digits = if self.value < 10 {
            1
        } else if self.value < 100 {
            2
        } else {
            3
        };
        Rectangle::from_size((24 * digits, 35).into()).to_physical_precise_round(scale)
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit_counter
    }
}

impl<R> RenderElement<R> for FpsElement<R::TextureId>
where
    R: Renderer + ImportAll,
    R::TextureId: 'static,
{
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), R::Error> {
        // FIXME: respect the src for cropping
        let scale = dst.size.to_f64() / self.src().size;
        let value_str = std::cmp::min(self.value, 999).to_string();
        let mut offset: Point<f64, Physical> = Point::from((0.0, 0.0));
        for digit in value_str.chars().map(|d| d.to_digit(10).unwrap()) {
            let digit_location = dst.loc.to_f64() + offset;
            let digit_size = Size::<i32, Logical>::from((22, 35)).to_f64().to_physical(scale);
            let dst = Rectangle::new(
                digit_location.to_i32_round(),
                ((digit_size.to_point() + digit_location).to_i32_round() - digit_location.to_i32_round()).to_size(),
            );
            let damage = damage
                .iter()
                .cloned()
                .flat_map(|x| x.intersection(dst))
                .map(|mut x| {
                    x.loc -= dst.loc;
                    x
                })
                .collect::<Vec<_>>();
            let texture_src: Rectangle<i32, Buffer> = match digit {
                9 => Rectangle::from_size((22, 35).into()),
                6 => Rectangle::new((22, 0).into(), (22, 35).into()),
                3 => Rectangle::new((44, 0).into(), (22, 35).into()),
                1 => Rectangle::new((66, 0).into(), (22, 35).into()),
                8 => Rectangle::new((0, 35).into(), (22, 35).into()),
                0 => Rectangle::new((22, 35).into(), (22, 35).into()),
                2 => Rectangle::new((44, 35).into(), (22, 35).into()),
                7 => Rectangle::new((0, 70).into(), (22, 35).into()),
                4 => Rectangle::new((22, 70).into(), (22, 35).into()),
                5 => Rectangle::new((44, 70).into(), (22, 35).into()),
                _ => unreachable!(),
            };

            frame.render_texture_from_to(&self.texture, texture_src.to_f64(), dst, &damage, &[], Transform::Normal, 1.0)?;
            offset += Point::from((24.0, 0.0)).to_physical(scale);
        }

        Ok(())
    }
}
