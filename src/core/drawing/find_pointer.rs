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
// Cairo drawing code taken from xfce4-find-cursor, by:
//
//   Copyright (C) 2019 Simon Steinbeiß <simon@xfce.org>

use gtk::cairo;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Color32F, ImportMem, Renderer, Texture,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    output::Output,
    utils::{Buffer, Logical, Point, Size, Transform},
};

use crate::{backend::AsGlesRenderer, core::config::Xfwl4Config};

const CIRCLE_RADIUS: i32 = 250;
const CIRCLE_SIZE: i32 = CIRCLE_RADIUS * 2;

pub struct FindPointerOverlay {
    draw_pos_degrees: f64,
    finished: bool,
}

impl FindPointerOverlay {
    pub fn theme_color(config: &Xfwl4Config) -> Color32F {
        config
            .active_color_1()
            .map(|color| Color32F::new(color.red() as f32, color.green() as f32, color.blue() as f32, color.alpha() as f32))
            .unwrap_or_else(|| Color32F::new(1., 0., 0., 1.))
    }

    pub fn new() -> Self {
        Self {
            draw_pos_degrees: 1.,
            finished: false,
        }
    }

    pub fn render_element<R>(
        &mut self,
        renderer: &mut R,
        color: Color32F,
        output: &Output,
        pointer_location: Point<f64, Logical>,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem + AsGlesRenderer,
        R::TextureId: Clone + Texture + Send + 'static,
    {
        if self.draw_pos_degrees > CIRCLE_RADIUS as f64 {
            self.finished = true;
            None
        } else {
            let scale = output.current_scale().fractional_scale();
            match self.draw_buffer(color, scale.max(1.).ceil() as i32) {
                Err(err) => {
                    tracing::warn!("Failed to draw find-pointer surface: {err}");
                    self.finished = true;
                    None
                }
                Ok(buffer) => {
                    let location =
                        pointer_location - output.current_location().to_f64() - Point::new(CIRCLE_RADIUS as f64, CIRCLE_RADIUS as f64);
                    MemoryRenderBufferRenderElement::from_buffer(
                        renderer,
                        location.to_physical(scale),
                        &buffer,
                        None,
                        None,
                        None,
                        Kind::Unspecified,
                    )
                    .ok()
                }
            }
        }
    }

    pub fn tick(&mut self) {
        self.draw_pos_degrees += 3.;
    }

    pub fn finished(&self) -> bool {
        self.finished
    }

    fn draw_buffer(&self, color: Color32F, scale: i32) -> anyhow::Result<MemoryRenderBuffer> {
        let buffer_size = CIRCLE_SIZE * scale;
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, buffer_size, buffer_size)?;
        surface.set_device_scale(scale as f64, scale as f64);

        let cr = cairo::Context::new(&surface)?;

        cr.set_operator(cairo::Operator::Source);
        cr.set_source_rgba(1., 1., 1., 0.);
        cr.paint()?;

        cr.set_line_width(3.);
        cr.translate(CIRCLE_RADIUS as f64, CIRCLE_RADIUS as f64);

        let arcs = if self.draw_pos_degrees > 90. {
            4
        } else if self.draw_pos_degrees > 60. {
            3
        } else if self.draw_pos_degrees > 30. {
            2
        } else {
            1
        };

        let (r, g, b) = (color.r() as f64, color.g() as f64, color.b() as f64);

        cr.arc(0., 0., self.draw_pos_degrees, 0., 2. * std::f64::consts::PI);
        cr.set_source_rgba(r, g, b, 0.5);
        cr.fill()?;

        cr.set_source_rgba(r, g, b, 1.);
        for i in 0..arcs {
            cr.arc(0., 0., self.draw_pos_degrees - (i as f64 * 30.), 0., 2. * std::f64::consts::PI);
            cr.stroke()?;
        }

        drop(cr);

        let size = Size::<_, Buffer>::new(surface.width(), surface.height());
        let data = surface.take_data()?;
        Ok(MemoryRenderBuffer::from_slice(
            &data,
            Fourcc::Argb8888,
            size,
            scale,
            Transform::Normal,
            None,
        ))
    }
}
