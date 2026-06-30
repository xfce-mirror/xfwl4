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

use anyhow::anyhow;
use gtk::cairo;
use smithay::utils::{Buffer, Size};

pub trait GdkPixbufSurfaceExt {
    fn to_surface(&self, scale: f64) -> anyhow::Result<cairo::ImageSurface>;
}

impl GdkPixbufSurfaceExt for gdk_pixbuf::Pixbuf {
    /// Converts a [`gdk_pixbuf::Pixbuf`] to a [`cairo::ImageSurface`]
    ///
    /// If the GdkPixbuf does not have an alpha channel, the returned cairo surface will have one,
    /// with its alpha channel set to 100% throughout.
    fn to_surface(&self, scale: f64) -> anyhow::Result<cairo::ImageSurface> {
        let size: Size<_, Buffer> = (self.width(), self.height()).into();
        let pix_stride = self.rowstride();
        let n_channels = self.n_channels();

        let alpha_mult = |channel: u8, alpha: u8| {
            let t = channel as u32 * alpha as u32 + 0x80;
            (((t >> 8) + t) >> 8) as u8
        };

        if n_channels == 3 || n_channels == 4 {
            let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, size.w, size.h)?;
            surface.set_device_scale(scale, scale);
            let surf_stride = surface.stride();

            let pix_data = self.read_pixel_bytes();
            let mut surf_data = surface.data()?;

            for j in 0..size.h as usize {
                let pix_offset = j * pix_stride as usize;
                let surf_offset = j * surf_stride as usize;

                if n_channels == 3 {
                    for i in 0..size.w as usize {
                        let pix_pixel = &pix_data[pix_offset + i * 3..];
                        let surf_pixel = &mut surf_data[surf_offset + i * 4..];

                        // TODO: handle big-endian systems?
                        surf_pixel[0] = pix_pixel[2];
                        surf_pixel[1] = pix_pixel[1];
                        surf_pixel[2] = pix_pixel[0];
                        surf_pixel[3] = 0xff;
                    }
                } else {
                    for i in 0..size.w as usize {
                        let pix_pixel = &pix_data[pix_offset + i * 4..];
                        let surf_pixel = &mut surf_data[surf_offset + i * 4..];

                        // TODO: handle big-endian systems?
                        surf_pixel[0] = alpha_mult(pix_pixel[2], pix_pixel[3]);
                        surf_pixel[1] = alpha_mult(pix_pixel[1], pix_pixel[3]);
                        surf_pixel[2] = alpha_mult(pix_pixel[0], pix_pixel[3]);
                        surf_pixel[3] = pix_pixel[3];
                    }
                }
            }

            drop(surf_data);

            surface.mark_dirty();
            Ok(surface)
        } else {
            Err(anyhow!("GdkPixbuf with {n_channels} color channels not supported"))
        }
    }
}
