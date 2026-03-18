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

use smithay::utils::{Logical, Rectangle};

const CENTER_POSITIONING_FUDGE: i32 = 25;

pub trait RectangleExt {
    fn is_near_center(&self, rect: Rectangle<i32, Logical>) -> bool;
}

impl RectangleExt for Rectangle<i32, Logical> {
    fn is_near_center(&self, rect: Rectangle<i32, Logical>) -> bool {
        let dx = (rect.loc.x - ((self.size.w - rect.size.w) / 2)).abs();
        let dy = (rect.loc.y - ((self.size.h - rect.size.h) / 2)).abs();
        dx < CENTER_POSITIONING_FUDGE && dy < CENTER_POSITIONING_FUDGE
    }
}
