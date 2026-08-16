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

use std::time::Duration;

use smithay::{
    output::Output,
    utils::{Logical, Rectangle},
};

pub trait OutputExt {
    fn geometry(&self) -> Option<Rectangle<i32, Logical>>;
    fn frame_duration(&self) -> Option<Duration>;
}

impl OutputExt for Output {
    fn geometry(&self) -> Option<Rectangle<i32, Logical>> {
        self.current_mode().map(|mode| {
            let size = self
                .current_transform()
                .transform_size(mode.size)
                .to_f64()
                .to_logical(self.current_scale().fractional_scale())
                .to_i32_round();
            Rectangle::new(self.current_location(), size)
        })
    }

    fn frame_duration(&self) -> Option<Duration> {
        self.current_mode()
            .filter(|mode| mode.refresh > 0)
            .map(|mode| Duration::from_secs_f64(1_000f64 / mode.refresh as f64))
    }
}
