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
    fn load_icon(&self, icon_name: &str, size: i32, scale: i32) -> anyhow::Result<gdk_pixbuf::Pixbuf>;
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
    fn load_icon(&self, icon_name: &str, size: i32, scale: i32) -> anyhow::Result<gdk_pixbuf::Pixbuf> {
        let icon_path = freedesktop_icons::lookup(icon_name)
            .with_theme(&self.icon_theme_name)
            .with_size(size as u16)
            .with_scale(scale as u16)
            .with_cache()
            .find()
            .ok_or_else(|| anyhow!("Unable to find icon {icon_name} in icon theme"))
            .inspect_err(|err| tracing::debug!("{err}"))?;

        let render_size = size * scale;
        Ok(gdk_pixbuf::Pixbuf::from_file_at_scale(&icon_path, render_size, render_size, true)
            .inspect_err(|err| tracing::debug!("Failed to load icon at {}:  {err}", icon_path.display()))?)
    }
}

impl IconTheme for gtk::IconTheme {
    fn load_icon(&self, icon_name: &str, size: i32, scale: i32) -> anyhow::Result<gdk_pixbuf::Pixbuf> {
        let icon_info = self
            .lookup_icon_for_scale(icon_name, size, scale, gtk::IconLookupFlags::FORCE_SIZE)
            .ok_or_else(|| anyhow!("Unable to find icon {icon_name} in icon theme"))?;
        Ok(icon_info.load_icon()?)
    }
}
