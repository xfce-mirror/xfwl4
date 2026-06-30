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

use std::rc::Rc;

use anyhow::anyhow;
use gtk::cairo;
use smithay::utils::{Logical, Size, Transform};

use crate::util::{cairo_ext::CairoImageSurfaceExt, gdk_pixbuf_ext::GdkPixbufSurfaceExt, icon_theme::IconTheme};

#[derive(Debug, Clone)]
pub struct FreedesktopIconsIconTheme {
    icon_theme_name: Rc<String>,
}

impl FreedesktopIconsIconTheme {
    pub fn new<S: AsRef<str>>(icon_theme_name: S) -> Self {
        Self {
            icon_theme_name: Rc::new(icon_theme_name.as_ref().to_owned()),
        }
    }

    pub fn set_icon_theme_name(&mut self, icon_theme_name: &str) {
        self.icon_theme_name = Rc::new(icon_theme_name.to_owned());
    }
}

impl IconTheme for FreedesktopIconsIconTheme {
    fn contains_icon(&self, icon_name: &str, size: u32, scale: u32) -> bool {
        freedesktop_icons::lookup(icon_name)
            .with_theme(&self.icon_theme_name)
            .with_size(size as u16)
            .with_scale(scale as u16)
            .with_cache()
            .find()
            .is_some()
    }

    fn load_icon(&self, icon_name: &str, size: u32, scale: f64) -> anyhow::Result<cairo::ImageSurface> {
        let scalei = scale.ceil() as u16;

        let icon_path = freedesktop_icons::lookup(icon_name)
            .with_theme(&self.icon_theme_name)
            .with_size(size as u16)
            .with_scale(scalei)
            .with_cache()
            .find()
            .ok_or_else(|| anyhow!("Unable to find icon {icon_name} in icon theme"))
            .inspect_err(|err| tracing::debug!("{err}"))?;

        let render_size = size as i32 * scalei as i32;
        let pixbuf = gdk_pixbuf::Pixbuf::from_file_at_scale(&icon_path, render_size, render_size, true)
            .inspect_err(|err| tracing::debug!("Failed to load icon at {}:  {err}", icon_path.display()))?;

        let surface = pixbuf.to_surface(scale)?;
        let final_size = Size::<_, Logical>::new(size, size)
            .to_f64()
            .to_buffer(scale, Transform::Normal)
            .to_i32_floor::<u32>();
        surface.scale_aspect(final_size.w, final_size.h)
    }
}
