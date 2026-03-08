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

const NOTCH_AMOUNT: f64 = 15.;

#[derive(Debug)]
pub struct ScrollAccumulator(f64);

impl ScrollAccumulator {
    pub fn accumulate(&mut self, amount: f64) -> i32 {
        self.0 += amount;

        let steps = self.0 / NOTCH_AMOUNT;
        if steps.abs() >= 1. {
            self.0 = steps.fract();
            steps.trunc() as i32
        } else {
            0
        }
    }

    pub fn reset(&mut self) {
        self.0 = 0.;
    }
}

impl Default for ScrollAccumulator {
    fn default() -> Self {
        Self(0.)
    }
}
