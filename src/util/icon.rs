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

use std::path::PathBuf;

use gdk_pixbuf::Pixbuf;
use gio::traits::{AppInfoExt, FileExt};
use glib::Cast;
use gtk::cairo;
use smithay::utils::{Buffer, Logical, Size, Transform};

use crate::util::{cairo_ext::CairoImageSurfaceExt, gdk_pixbuf_ext::GdkPixbufSurfaceExt, icon_theme::IconTheme};

pub const FALLBACK_ICON_NAME: &str = "xfwm4-default";

/// An icon stored in a file, which may be scalable and has a width and height
///
/// If `scalable` is `true`, the `size` field holds the image's "native" size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIcon {
    pub path: PathBuf,
    pub scalable: bool,
    pub size: Size<u32, Buffer>,
}

impl FileIcon {
    /// The dimension independent "icon size", in raw pixels.
    pub fn pixel_size(&self) -> u32 {
        self.size.w.max(self.size.h)
    }
}

/// Pixels stored as packed ARGB32 data, little-endian order, `size.w * 4` rowstride, with
/// premultiplied alpha
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argb32Pixels {
    pub bytes: Vec<u8>,
    pub size: Size<u32, Buffer>,
    pub scale: u32,
}

impl Argb32Pixels {
    /// The dimension-independent "icon size", in logical pixels
    pub fn icon_size(&self) -> u32 {
        let logical_size = self.size.to_logical(self.scale, Transform::Normal);
        logical_size.w.max(logical_size.h)
    }

    /// The dimension independent "icon size", in raw pixels.
    pub fn pixel_size(&self) -> u32 {
        self.size.w.max(self.size.h)
    }
}

/// An icon specified in a .desktop file, which is either a themed icon or a file
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopIcon {
    Named(String),
    File(FileIcon),
}

/// All icons specified or advertised by an application
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconSource {
    window_named: Option<String>,
    window_rasters: Vec<Argb32Pixels>,
    app_icon: Option<DesktopIcon>,
}

/// A single icon, chosen as the best match according to some criteria
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Icon {
    Named(String),
    File(PathBuf),
    Pixels(Argb32Pixels),
}

impl DesktopIcon {
    fn from_app_id(app_id: &str) -> Option<DesktopIcon> {
        let desktop_name = if app_id.ends_with(".desktop") {
            app_id
        } else {
            &format!("{app_id}.desktop")
        };
        let app_info = gio::DesktopAppInfo::new(desktop_name)?;

        let gicon = app_info.icon()?;
        if let Some(themed) = gicon.downcast_ref::<gio::ThemedIcon>() {
            themed.names().into_iter().next().map(|name| DesktopIcon::Named(name.to_string()))
        } else if let Some(file) = gicon.downcast_ref::<gio::FileIcon>()
            && let Some(path) = file.file().path()
        {
            let (format, width, height) = Pixbuf::file_info(&path)?;
            let width = (width > 0 || format.is_scalable()).then_some(width.max(0) as u32)?;
            let height = (height > 0 || format.is_scalable()).then_some(height.max(0) as u32)?;
            Some(DesktopIcon::File(FileIcon {
                path,
                scalable: format.is_scalable(),
                size: (width, height).into(),
            }))
        } else {
            None
        }
    }
}

impl IconSource {
    fn is_empty(&self) -> bool {
        self.window_named.is_none() && self.window_rasters.is_empty() && self.app_icon.is_none()
    }

    fn is_fallback(&self) -> bool {
        self.window_named.as_ref().is_some_and(|name| name == FALLBACK_ICON_NAME)
            && self.window_rasters.is_empty()
            && self.app_icon.is_none()
    }

    fn reset_to_fallback(&mut self) {
        self.window_named = Some(FALLBACK_ICON_NAME.to_owned());
        self.window_rasters.clear();
        self.app_icon = None;
    }

    fn clear_fallback(&mut self) {
        self.window_named = None;
    }

    /// Updates the icon source's app_id; returns whether the icon changed
    pub fn update_app_id(&mut self, app_id: Option<String>) -> bool {
        let app_icon = app_id.and_then(|app_id| DesktopIcon::from_app_id(&app_id));

        if app_icon.is_some() && self.is_fallback() {
            self.clear_fallback();
            self.app_icon = app_icon;
            true
        } else if self.app_icon != app_icon {
            self.app_icon = app_icon;
            if self.is_empty() {
                self.reset_to_fallback();
            }
            true
        } else {
            false
        }
    }

    /// Updates the icon source's icon name; returns whether the icon changed
    pub fn update_name(&mut self, window_named: Option<String>) -> bool {
        if window_named.is_some() && self.is_fallback() {
            self.clear_fallback();
            self.window_named = window_named;
            true
        } else if self.window_named != window_named {
            self.window_named = window_named;
            if self.is_empty() {
                self.reset_to_fallback();
            }
            true
        } else {
            false
        }
    }

    /// Updates the icon source's raster images; returns whether the icon changed
    pub fn update_rasters(&mut self, mut window_rasters: Vec<Argb32Pixels>) -> bool {
        window_rasters.sort_by_key(|raster| raster.pixel_size());

        if !window_rasters.is_empty() && self.is_fallback() {
            self.clear_fallback();
            self.window_rasters = window_rasters;
            true
        } else if self.window_rasters != window_rasters {
            self.window_rasters = window_rasters;
            if self.is_empty() {
                self.reset_to_fallback();
            }
            true
        } else {
            false
        }
    }

    pub fn depends_on_theme(&self) -> bool {
        self.window_named.is_some() || matches!(self.app_icon, Some(DesktopIcon::Named(_)))
    }

    pub fn choose_best<IT: IconTheme>(&self, icon_theme: &IT, size: u32, scale: u32) -> Icon {
        if let Some(raster) = self
            .window_rasters
            .iter()
            .find(|raster| raster.icon_size() == size && raster.scale == scale)
        {
            // Exact match in rasters.
            Icon::Pixels(raster.clone())
        } else if let Some(named) = self.window_named.as_ref()
            && icon_theme.contains_icon(named, size, scale)
        {
            // Themed icon is available.
            Icon::Named(named.clone())
        } else if let Some(raster) = self
            .window_rasters
            .iter()
            .find(|raster| raster.scale == scale && raster.icon_size() >= size)
        {
            // Next-largest logical size in rasters (same scale).
            Icon::Pixels(raster.clone())
        } else if let Some(raster) = self.window_rasters.iter().find(|raster| raster.pixel_size() >= size * scale) {
            // Next-largest physical size in rasters (any scale).
            Icon::Pixels(raster.clone())
        } else if let Some(raster) = self
            .window_rasters
            .last()
            .filter(|raster| raster.pixel_size() as f64 * 1.75 >= (size * scale) as f64)
        {
            // Now we have a trickier decision to make.  If we have any rasters at all, they're all
            // smaller than the requested size/scale, and upscaling will probably not look good.
            // But the app icon might be a completely different icon.  But let's say that if we
            // only have to upscale the raster by a *little* bit, it's good enough.
            Icon::Pixels(raster.clone())
        } else if let Some(DesktopIcon::File(file)) = self.app_icon.as_ref()
            && (file.scalable || file.pixel_size() >= size * scale)
        {
            // App icon is scalable or is at least as large as requested.
            Icon::File(file.path.clone())
        } else if let Some(DesktopIcon::Named(named)) = self.app_icon.as_ref()
            && icon_theme.contains_icon(named, size, scale)
        {
            // App icon is in theme.
            Icon::Named(named.clone())
        } else if let Some(DesktopIcon::File(file)) = self.app_icon.as_ref() {
            if let Some(raster) = self.window_rasters.last()
                && raster.pixel_size() as f64 >= file.pixel_size() as f64 * 0.75
            {
                // We have an app icon, but the raster icon (even if possibly slightly smaller) is
                // still more desirable.
                Icon::Pixels(raster.clone())
            } else {
                // We don't have a raster, or it's significantly smaller than the file.
                Icon::File(file.path.clone())
            }
        } else if let Some(raster) = self.window_rasters.last() {
            // Even a tiny raster image is probably better than the fallback.
            Icon::Pixels(raster.clone())
        } else {
            // Fallback.
            Icon::Named(FALLBACK_ICON_NAME.to_owned())
        }
    }
}

impl Default for IconSource {
    fn default() -> Self {
        Self {
            window_named: Some(FALLBACK_ICON_NAME.to_owned()),
            window_rasters: Vec::new(),
            app_icon: None,
        }
    }
}

impl Icon {
    pub fn fallback() -> Self {
        Self::Named(FALLBACK_ICON_NAME.to_owned())
    }

    /// Loads the specified icon into a [`cairo::ImageSurface`]
    ///
    /// `width` and `height` are interpreted as scaled/logical pixels.
    pub fn load<IT: IconTheme>(&self, width: u32, height: u32, scale: f64, icon_theme: &IT) -> anyhow::Result<cairo::ImageSurface> {
        match self {
            Self::Named(icon_name) => icon_theme.load_icon(icon_name, width.min(height), scale),
            Self::File(path) => Pixbuf::from_file(path)?.to_surface(scale),
            Self::Pixels(pixels) => pixels.to_surface(),
        }
        .or_else(|err| {
            tracing::info!("Unable to load icon (using fallback): {err}");
            icon_theme.load_icon(FALLBACK_ICON_NAME, width.min(height), scale)
        })
        .and_then(|surface| {
            surface.set_device_scale(scale, scale);
            let phys_size = Size::<_, Logical>::new(width, height)
                .to_f64()
                .to_buffer(scale, Transform::Normal)
                .to_i32_floor::<u32>();
            surface.scale_aspect(phys_size.w, phys_size.h)
        })
    }
}

impl Argb32Pixels {
    pub fn to_surface(&self) -> anyhow::Result<cairo::ImageSurface> {
        let surface = cairo::ImageSurface::create_for_data(
            self.bytes.clone(),
            cairo::Format::ARgb32,
            self.size.w as i32,
            self.size.h as i32,
            self.size.w as i32 * 4,
        )?;
        surface.set_device_scale(self.scale as f64, self.scale as f64);
        Ok(surface)
    }

    pub fn into_surface(self) -> anyhow::Result<cairo::ImageSurface> {
        let surface = cairo::ImageSurface::create_for_data(
            self.bytes,
            cairo::Format::ARgb32,
            self.size.w as i32,
            self.size.h as i32,
            self.size.w as i32 * 4,
        )?;
        surface.set_device_scale(self.scale as f64, self.scale as f64);
        Ok(surface)
    }
}
