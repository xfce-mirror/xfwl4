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
use gtk::{gdk_pixbuf, traits::IconThemeExt};

pub trait IconTheme {
    fn load_icon(&self, icon_name: &str, size: i32, scale: f64) -> anyhow::Result<gdk_pixbuf::Pixbuf>;
}

#[derive(Debug, Clone)]
pub struct FreedesktopIconsIconTheme {
    icon_theme_name: Rc<String>,
}

impl FreedesktopIconsIconTheme {
    pub fn new() -> Self {
        Self {
            icon_theme_name: Rc::new("hicolor".to_owned()),
        }
    }

    pub fn set_icon_theme_name(&mut self, icon_theme_name: &str) {
        self.icon_theme_name = Rc::new(icon_theme_name.to_owned());
    }
}

impl Default for FreedesktopIconsIconTheme {
    fn default() -> Self {
        Self::new()
    }
}

impl IconTheme for FreedesktopIconsIconTheme {
    fn load_icon(&self, icon_name: &str, size: i32, scale: f64) -> anyhow::Result<gdk_pixbuf::Pixbuf> {
        let scalei = scale.ceil() as u16;

        let icon_path = freedesktop_icons::lookup(icon_name)
            .with_theme(&self.icon_theme_name)
            .with_size(size as u16)
            .with_scale(scalei)
            .with_cache()
            .find()
            .ok_or_else(|| anyhow!("Unable to find icon {icon_name} in icon theme"))
            .inspect_err(|err| tracing::debug!("{err}"))?;

        let render_size = size * scalei as i32;
        let pixbuf = gdk_pixbuf::Pixbuf::from_file_at_scale(&icon_path, render_size, render_size, true)
            .inspect_err(|err| tracing::debug!("Failed to load icon at {}:  {err}", icon_path.display()))?;

        maybe_rescale_icon(pixbuf, (size as f64 * scale).floor() as i32)
    }
}

impl IconTheme for gtk::IconTheme {
    fn load_icon(&self, icon_name: &str, size: i32, scale: f64) -> anyhow::Result<gdk_pixbuf::Pixbuf> {
        let icon_info = self
            .lookup_icon_for_scale(icon_name, size, scale.ceil() as i32, gtk::IconLookupFlags::FORCE_SIZE)
            .ok_or_else(|| anyhow!("Unable to find icon {icon_name} in icon theme"))?;
        let pixbuf = icon_info.load_icon()?;
        maybe_rescale_icon(pixbuf, (size as f64 * scale).floor() as i32)
    }
}

fn maybe_rescale_icon(pixbuf: gdk_pixbuf::Pixbuf, final_size: i32) -> anyhow::Result<gdk_pixbuf::Pixbuf> {
    if pixbuf.width() > final_size || pixbuf.height() > final_size {
        let xratio = pixbuf.width() as f64 / final_size as f64;
        let yratio = pixbuf.height() as f64 / final_size as f64;
        let (final_width, final_height) = if pixbuf.width() > pixbuf.height() {
            (final_size, (pixbuf.height() as f64 / xratio).round() as i32)
        } else {
            ((pixbuf.width() as f64 / yratio).round() as i32, final_size)
        };
        pixbuf
            .scale_simple(final_width, final_height, gdk_pixbuf::InterpType::Bilinear)
            .ok_or_else(|| anyhow!("Failed to scale pixbuf to requested size"))
    } else {
        Ok(pixbuf)
    }
}
