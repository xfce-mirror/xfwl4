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

use gtk::cairo;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Color32F, ImportMem, Renderer, Texture,
            element::{Id, Kind, texture::TextureRenderElement},
            gles::{GlesRenderer, GlesTexture},
        },
    },
    input::{Seat, SeatHandler},
    reexports::wayland_server::{Client, backend::ClientId},
    utils::{Logical, Point, Rectangle, Scale, Size, Transform},
};

use crate::core::{
    config::Xfwl4Config,
    focus::{KeyboardFocusTarget, PointerFocusTarget},
    util::SeatFocusExt,
};

const OUTLINE_WIDTH: f64 = 2.;

pub struct Wireframe {
    owner: Option<Client>,
    color: Color32F,
    geometry: Rectangle<i32, Logical>,
    texture: Option<GlesTexture>,
    texture_id: Id,
}

impl Wireframe {
    pub fn new(owner: Option<Client>, geometry: Rectangle<i32, Logical>, config: &Xfwl4Config) -> Self {
        let color = config
            .active_color_1()
            .map(|color| Color32F::new(color.red() as f32, color.green() as f32, color.blue() as f32, color.alpha() as f32))
            .unwrap_or_else(|| Color32F::new(0.3, 0.3, 0.3, 1.));
        Self {
            owner,
            color,
            geometry,
            texture: None,
            texture_id: Id::new(),
        }
    }

    pub fn update_location(&mut self, location: Point<i32, Logical>) {
        if self.geometry.loc != location {
            self.geometry.loc = location;
        }
    }

    pub fn update_size(&mut self, size: Size<i32, Logical>) {
        if self.geometry.size != size {
            self.geometry.size = size;
            self.texture = None;
        }
    }

    pub fn is_owned_by(&self, client_id: ClientId) -> bool {
        self.owner.as_ref().is_some_and(|owner| owner.id() == client_id)
    }

    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        self.geometry
    }

    pub fn render_element(
        &mut self,
        renderer: &mut GlesRenderer,
        output_location: Point<i32, Logical>,
        scale: Scale<f64>,
    ) -> Option<TextureRenderElement<GlesTexture>> {
        if let Some(texture) = self.texture(renderer, scale).cloned() {
            let location = (self.geometry.loc - output_location).to_f64().to_physical_precise_round(scale);
            let src = Rectangle::from_size(texture.size().to_logical(1, Transform::Normal)).to_f64();
            Some(TextureRenderElement::from_static_texture(
                self.texture_id.clone(),
                renderer.context_id(),
                location,
                texture,
                1,
                Transform::Normal,
                None,
                Some(src),
                Some(self.geometry.size),
                None,
                Kind::Unspecified,
            ))
        } else {
            None
        }
    }

    fn texture(&mut self, renderer: &mut GlesRenderer, scale: Scale<f64>) -> Option<&GlesTexture> {
        if self.texture.is_none() {
            let buffer_size = self.geometry.size.to_f64().to_buffer(scale, Transform::Normal).to_i32_round();

            let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, buffer_size.w, buffer_size.h)
                .inspect_err(|err| {
                    tracing::warn!(
                        "Failed to create {}x{} image surface for wireframe: {err}",
                        buffer_size.w,
                        buffer_size.h
                    )
                })
                .ok()?;
            let cr = cairo::Context::new(&surface)
                .inspect_err(|err| tracing::warn!("Failed to create cairo context: {err}"))
                .ok()?;
            cr.set_line_width(OUTLINE_WIDTH);
            cr.set_line_join(cairo::LineJoin::Miter);

            cr.set_operator(cairo::Operator::Source);
            cr.set_source_rgba(self.color.r() as f64, self.color.g() as f64, self.color.b() as f64, 0.5);
            cr.paint().inspect_err(|err| tracing::warn!("cairo_paint failed: {err}")).ok()?;

            cr.set_source_rgba(self.color.r() as f64, self.color.g() as f64, self.color.b() as f64, 1.);
            cr.rectangle(
                (OUTLINE_WIDTH / 2.).floor(),
                (OUTLINE_WIDTH / 2.).floor(),
                buffer_size.w as f64 - OUTLINE_WIDTH,
                buffer_size.h as f64 - OUTLINE_WIDTH,
            );
            cr.stroke().inspect_err(|err| tracing::warn!("cairo_stroke failed: {err}")).ok()?;

            drop(cr);

            let stride = surface.stride() as usize;
            let row_bytes = buffer_size.w as usize * 4;
            let data = surface
                .data()
                .inspect_err(|err| tracing::warn!("unable to get data from cairo surface: {err}"))
                .ok()?;

            let tight: Vec<u8> = if stride == row_bytes {
                data.to_vec()
            } else {
                (0..buffer_size.h as usize)
                    .flat_map(|y| &data[y * stride..y * stride + row_bytes])
                    .copied()
                    .collect()
            };

            self.texture = renderer
                .import_memory(&tight, Fourcc::Argb8888, buffer_size, false)
                .inspect_err(|err| tracing::warn!("failed to import wireframe texture memory: {err}"))
                .ok();
            self.texture_id = Id::new();
        }

        self.texture.as_ref()
    }

    pub(in crate::core) fn should_render<D>(&self, seat: &Seat<D>) -> bool
    where
        D: SeatHandler<KeyboardFocus = KeyboardFocusTarget, PointerFocus = PointerFocusTarget> + 'static,
    {
        self.owner.as_ref().is_none_or(|owner| {
            seat.keyboard_client().is_some_and(|client| client == *owner) || seat.pointer_client().is_some_and(|client| client == *owner)
        })
    }
}
