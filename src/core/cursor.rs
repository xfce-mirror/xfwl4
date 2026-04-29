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

use std::{io::Read, path::PathBuf, time::Duration};

use anyhow::anyhow;
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::CursorIcon,
    reexports::calloop::{
        LoopHandle,
        channel::{Channel, channel},
    },
    utils::{Logical, Point, Rectangle, Size, Transform},
};
use xcursor::{
    CursorTheme as XCursorTheme,
    parser::{Image, parse_xcursor},
};
use xfconf::ChannelExtManual;

use crate::{
    backend::Backend,
    core::{state::Xfwl4State, util::CalloopXfconfSource},
};

const XSETTINGS_CHANNEL_NAME: &str = "xsettings";
const PROP_CURSOR_THEME_NAME: &str = "/Gtk/CursorThemeName";
const PROP_CURSOR_THEME_SIZE: &str = "/Gtk/CursorThemeSize";

static FALLBACK_CURSOR_DATA: &[u8] = include_bytes!("../../resources/cursor.rgba");

pub struct CursorTheme {
    xtheme: XCursorTheme,
    name: String,
    size: u32,
    buffer_cache: Vec<(Image, MemoryRenderBuffer)>,
}

pub struct Cursor {
    icons: Vec<Image>,
    size: u32,
}

pub struct CursorFrame {
    pub buffer: MemoryRenderBuffer,
    pub hotspot: Point<i32, Logical>,
    pub src: Rectangle<f64, Logical>,
    pub size: Size<i32, Logical>,
}

pub struct CursorThemeChanged;

impl CursorTheme {
    pub fn new<BackendData: Backend>(loop_handle: LoopHandle<'static, Xfwl4State<BackendData>>) -> (Self, Channel<CursorThemeChanged>) {
        let (tx, rx) = channel();
        let channel = xfconf::Channel::new(XSETTINGS_CHANNEL_NAME);

        let source = CalloopXfconfSource::new(channel.clone(), [PROP_CURSOR_THEME_NAME, PROP_CURSOR_THEME_SIZE]);
        loop_handle
            .insert_source(source, move |(property_name, value), _, state| {
                let changed = match property_name.as_str() {
                    PROP_CURSOR_THEME_NAME => {
                        if let Ok(name) = value.get::<String>()
                            && name != state.core.cursor_theme.name
                        {
                            state.core.cursor_theme.xtheme = XCursorTheme::load(&name);
                            state.core.cursor_theme.name = name;
                            true
                        } else {
                            false
                        }
                    }

                    PROP_CURSOR_THEME_SIZE => {
                        if let Some(size) = value.get::<i32>().ok().filter(|size| *size > 0)
                            && size as u32 != state.core.cursor_theme.size
                        {
                            state.core.cursor_theme.size = size as u32;
                            true
                        } else {
                            false
                        }
                    }

                    _ => false,
                };

                if changed {
                    state.core.cursor_theme.buffer_cache.clear();
                    let _ = tx.send(CursorThemeChanged);
                }
            })
            .expect("failed to register cursor theme source with event loop");

        let name = channel
            .get_property::<String>(PROP_CURSOR_THEME_NAME)
            .unwrap_or_else(|| "default".to_owned());
        let size = channel
            .get_property::<i32>(PROP_CURSOR_THEME_SIZE)
            .filter(|size| *size > 0)
            .unwrap_or(24) as u32;
        let xtheme = XCursorTheme::load(&name);

        let theme = Self {
            xtheme,
            name,
            size,
            buffer_cache: Vec::new(),
        };

        (theme, rx)
    }

    pub fn get_frame(&mut self, cursor: &Cursor, output_scale: f64, time: Duration) -> CursorFrame {
        let image = cursor.get_image(output_scale, time);
        let theme_size = cursor.size as i32;
        let hotspot = (
            (image.xhot as f64 * theme_size as f64 / image.width as f64).round() as i32,
            (image.yhot as f64 * theme_size as f64 / image.height as f64).round() as i32,
        )
            .into();
        let src = Rectangle::from_size(Size::from((image.width as f64, image.height as f64)));
        let size = Size::from((theme_size, theme_size));
        let buffer = self.buffer_for(image);
        CursorFrame {
            buffer,
            hotspot,
            src,
            size,
        }
    }

    fn buffer_for(&mut self, image: Image) -> MemoryRenderBuffer {
        match self
            .buffer_cache
            .iter()
            .find_map(|(cached, buffer)| (cached == &image).then(|| buffer.clone()))
        {
            Some(buffer) => buffer,
            None => {
                let buffer = MemoryRenderBuffer::from_slice(
                    &image.pixels_rgba,
                    Fourcc::Argb8888,
                    (image.width as i32, image.height as i32),
                    1,
                    Transform::Normal,
                    None,
                );
                self.buffer_cache.push((image, buffer.clone()));
                buffer
            }
        }
    }

    fn cursor_path(&self, cursor_icon: CursorIcon) -> Option<PathBuf> {
        std::iter::once(&cursor_icon.name())
            .chain(cursor_icon.alt_names())
            .find_map(|name| self.xtheme.load_icon(name))
    }

    pub fn load_cursor(&self, cursor_icon: CursorIcon) -> anyhow::Result<Cursor> {
        let icon_path = self
            .cursor_path(cursor_icon)
            .ok_or_else(|| anyhow!("No cursor available for name {cursor_icon}"))?;
        let mut cursor_file = std::fs::File::open(icon_path)?;
        let mut cursor_data = Vec::new();
        cursor_file.read_to_end(&mut cursor_data)?;
        let icons = parse_xcursor(&cursor_data).ok_or_else(|| anyhow!("Failed to parse cursor named {cursor_icon}"))?;

        Ok(Cursor { icons, size: self.size })
    }

    pub fn fallback_cursor(&self) -> Cursor {
        self.load_cursor(CursorIcon::Default).unwrap_or_else(|_| Cursor {
            icons: Cursor::fallback().icons,
            size: self.size,
        })
    }

    pub fn theme_name(&self) -> &str {
        &self.name
    }

    pub fn cursor_size(&self) -> u32 {
        self.size
    }
}

impl Cursor {
    pub fn fallback() -> Cursor {
        Cursor {
            icons: vec![Image {
                size: 32,
                width: 64,
                height: 64,
                xhot: 1,
                yhot: 1,
                delay: 1,
                pixels_rgba: Vec::from(FALLBACK_CURSOR_DATA),
                pixels_argb: vec![], //unused
            }],
            size: 64,
        }
    }

    pub fn get_image(&self, output_scale: f64, time: Duration) -> Image {
        let target_pixel_size = (self.size as f64 * output_scale).round().max(1.0) as u32;
        frame(time.as_millis() as u32, target_pixel_size, &self.icons)
    }
}

fn nearest_images(size: u32, images: &[Image]) -> impl Iterator<Item = &Image> {
    // Follow the nominal size of the cursor to choose the nearest
    let nearest_image = images.iter().min_by_key(|image| (size as i32 - image.size as i32).abs()).unwrap();

    images
        .iter()
        .filter(move |image| image.width == nearest_image.width && image.height == nearest_image.height)
}

fn frame(mut millis: u32, size: u32, images: &[Image]) -> Image {
    let total = nearest_images(size, images).fold(0, |acc, image| acc + image.delay);
    if total == 0 {
        return nearest_images(size, images).next().unwrap().clone();
    }
    millis %= total;

    for img in nearest_images(size, images) {
        if millis < img.delay {
            return img.clone();
        }
        millis -= img.delay;
    }

    unreachable!()
}
