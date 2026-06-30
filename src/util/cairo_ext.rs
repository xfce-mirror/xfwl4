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

use std::{cell::RefCell, rc::Rc};

use gtk::cairo;
use smithay::utils::{Buffer, Size};

use crate::util::icon::Argb32Pixels;

// From gdk_pixbuf_saturate_and_pixelate().
const DARK_FACTOR: f64 = 0.7;

pub trait CairoImageSurfaceExt {
    /// Scales the surface to physical `width` and `height`, maintaining the aspect ratio.
    fn scale_aspect(&self, width: u32, height: u32) -> anyhow::Result<cairo::ImageSurface>;

    /// Desaturates and pixelates the surface in place
    ///
    /// Only [`cairo::Format::ARgb32`] surfaces are supported; does nothing for other formats.
    ///
    /// This is the same algorithm `gdk_pixbuf_saturate_and_pixelate()` applies.
    fn saturate_and_pixelate_in_place(&mut self, saturation_amount: f64);

    /// Exports the surface's data as [`Argb32Pixels`]
    fn to_argb32_pixels(&self) -> anyhow::Result<Argb32Pixels>;
}

impl CairoImageSurfaceExt for cairo::ImageSurface {
    fn scale_aspect(&self, width: u32, height: u32) -> anyhow::Result<cairo::ImageSurface> {
        let size: Size<_, Buffer> = (width, height).into();
        let self_size: Size<_, Buffer> = (self.width(), self.height()).into();

        let aspect = self_size.w as f64 / self_size.h as f64;
        let final_aspect = size.w as f64 / size.h as f64;

        let scaled_size: Size<i32, Buffer> = if aspect > final_aspect {
            Size::<_, Buffer>::new(size.w as f64, size.w as f64 / aspect)
        } else {
            Size::<_, Buffer>::new(size.h as f64 * aspect, size.h as f64)
        }
        .to_i32_floor::<i32>();

        if (self_size.w == scaled_size.w && self_size.h <= self_size.w) || (self_size.h == scaled_size.h && self_size.w <= self_size.h) {
            Ok(self.clone())
        } else {
            let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, scaled_size.w, scaled_size.h)?;
            let (scale_x, scale_y) = self.device_scale();
            surface.set_device_scale(scale_x, scale_y);

            let cr = cairo::Context::new(&surface)?;
            cr.set_operator(cairo::Operator::Source);
            cr.set_source_surface(self, 0., 0.)?;
            cr.scale(scaled_size.w as f64 / self_size.w as f64, scaled_size.h as f64 / self_size.h as f64);
            cr.paint()?;
            drop(cr);

            Ok(surface)
        }
    }

    fn saturate_and_pixelate_in_place(&mut self, saturation_amount: f64) {
        if self.format() == cairo::Format::ARgb32 {
            let width = self.width() as usize;
            let stride = self.stride() as usize;

            if let Ok(mut data) = self.data() {
                for (y, row) in data.chunks_mut(stride).enumerate() {
                    for (x, px) in row.chunks_exact_mut(4).take(width).enumerate() {
                        // Cairo ARGB32 is little-endian (bytes B, G, R, A) with R/G/B
                        // premultiplied.
                        let b = px[0] as f64;
                        let g = px[1] as f64;
                        let r = px[2] as f64;
                        let a = px[3] as f64;

                        let intensity = r * 0.30 + g * 0.59 + b * 0.11;

                        let (nb, ng, nr) = if (x + y) % 2 == 0 {
                            // The 127 grey constant is scaled by alpha so the result stays
                            // premultiplied (transparent pixels remain transparent).
                            let v = intensity / 2. + 127. * a / 255.;
                            (v, v, v)
                        } else {
                            let saturate = |c: f64| ((1. - saturation_amount) * intensity + saturation_amount * c) * DARK_FACTOR;
                            (saturate(b), saturate(g), saturate(r))
                        };

                        // Clamp to [0, a] to preserve the premultiplied invariant.
                        px[0] = nb.clamp(0.0, a) as u8;
                        px[1] = ng.clamp(0.0, a) as u8;
                        px[2] = nr.clamp(0.0, a) as u8;
                    }
                }
            }

            self.mark_dirty();
        }
    }

    fn to_argb32_pixels(&self) -> anyhow::Result<Argb32Pixels> {
        let width = self.width() as u32;
        let height = self.height() as u32;
        let stride = self.stride() as usize;

        let bytes = Rc::new(RefCell::new(Vec::new()));

        self.with_data(|src| {
            let bytes_inner = if stride == width as usize * 4 {
                src.to_vec()
            } else {
                let mut bytes = Vec::with_capacity(width as usize * 4 * height as usize);
                for line in src.chunks_exact(stride) {
                    bytes.extend_from_slice(&line[..(width as usize * 4)]);
                }
                bytes
            };

            bytes.replace(bytes_inner);
        })?;

        Ok(Argb32Pixels {
            bytes: Rc::unwrap_or_clone(bytes).into_inner(),
            size: (width, height).into(),
            scale: 1,
        })
    }
}
