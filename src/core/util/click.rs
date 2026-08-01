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

use smithay::utils::{Logical, Point};

use crate::core::config::UiSettings;

#[derive(Debug)]
struct LastClick {
    location: Point<f64, Logical>,
    time_msec: u32,
}

#[derive(Debug, Default)]
pub struct DoubleClickState(Option<LastClick>);

impl DoubleClickState {
    /// Returns true if a double-click was deteted.
    pub fn clicked(&mut self, ui_settings: &UiSettings, location: Point<f64, Logical>, time_msec: u32) -> bool {
        if let Some(mut last_click) = self.0.take() {
            let distance = {
                let dx = last_click.location.x - location.x;
                let dy = last_click.location.y - location.y;
                (dx * dx + dy * dy).sqrt()
            };
            let elapsed = Duration::from_millis(time_msec as u64).saturating_sub(Duration::from_millis(last_click.time_msec as u64));

            if distance <= ui_settings.double_click_distance() && elapsed <= ui_settings.double_click_time() {
                true
            } else {
                last_click.location = location;
                last_click.time_msec = time_msec;
                self.0 = Some(last_click);
                false
            }
        } else {
            self.0 = Some(LastClick { location, time_msec });
            false
        }
    }

    pub fn reset(&mut self) {
        self.0 = None;
    }
}
