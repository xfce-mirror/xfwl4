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

mod client_ext;
mod color_ops;
mod geometry_ext;
pub(crate) mod icon_theme;
mod image_copy_ext;
mod image_data;
mod iter;
mod laptop;
mod output_ext;
pub(crate) mod rc;
mod scroll;
mod xfconf_source;
mod xkb_ext;
pub(crate) mod xpm;

pub use client_ext::ClientExt;
pub use color_ops::Hlsa;
pub use geometry_ext::*;
pub use image_copy_ext::{OutputImageCopyExt, WindowImageCopyExt};
pub use image_data::{ImageData, shm_buffer_to_image_data};
pub use iter::zip_all_first;
pub use laptop::*;
pub use output_ext::OutputExt;
pub use scroll::ScrollAccumulator;
pub use xfconf_source::CalloopXfconfSource;
pub use xkb_ext::XkbStateGdkExt;

pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

pub(in crate::core) fn prettify_name(name: &str) -> Option<String> {
    if name.is_empty() {
        None
    } else {
        use std::{collections::HashSet, sync::LazyLock};

        static VALID_CHARS: LazyLock<HashSet<char>> = LazyLock::new(|| {
            "[]()0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
                .chars()
                .collect()
        });

        Some(
            name.chars()
                .map(|c| if VALID_CHARS.contains(&c) { c } else { ' ' })
                .collect::<String>()
                .trim()
                .to_owned(),
        )
    }
}
