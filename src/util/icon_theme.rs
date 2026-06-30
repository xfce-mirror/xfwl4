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

/// Abstraction over an icon theme
pub trait IconTheme {
    /// Checks if the icon theme can resolve `icon_name` at the specified `size` and `scale`
    fn contains_icon(&self, icon_name: &str, size: u32, scale: u32) -> bool;

    /// Loads an icon named `icon_name` at the specified `size` and `scale`
    ///
    /// `scale` should correspond to the (possibly fractional) output scale on which the icon is to
    /// be displayed.  Since icon themes only include icons at integer scales, the scale will be
    /// rounded up to the next integer when doing the lookup, but then will be rendered at a size
    /// using the fractional scale.
    fn load_icon(&self, icon_name: &str, size: u32, scale: f64) -> anyhow::Result<gtk::cairo::ImageSurface>;
}
