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

use gtk::gdk;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hlsa {
    hue: f64,
    lightness: f64,
    saturation: f64,
    alpha: f64,
}

impl Hlsa {
    pub fn shade(&self, multiplier: f64) -> Self {
        Self {
            hue: self.hue,
            lightness: (self.lightness * multiplier).clamp(0., 1.),
            saturation: (self.saturation * multiplier).clamp(0., 1.),
            alpha: self.alpha,
        }
    }
}

impl From<gdk::RGBA> for Hlsa {
    fn from(value: gdk::RGBA) -> Self {
        let max = value.red().max(value.green()).max(value.blue());
        let min = value.red().min(value.green()).min(value.blue());

        let lightness = (max + min) / 2.;

        let saturation = if max != min {
            if lightness <= 0.5 {
                (max - min) / (max + min)
            } else {
                (max - min) / (2. - max - min)
            }
        } else {
            0.
        };

        let hue = if max != min {
            let delta = max - min;
            let h = 60.
                * if value.red() == max {
                    (value.green() - value.blue()) / delta
                } else if value.green() == max {
                    2. + (value.blue() - value.red()) / delta
                } else if value.blue() == max {
                    4. + (value.red() - value.green()) / delta
                } else {
                    0.
                };

            if h < 0. { h + 360. } else { h }
        } else {
            0.
        };

        Self {
            hue,
            lightness,
            saturation,
            alpha: value.alpha(),
        }
    }
}

impl From<Hlsa> for gdk::RGBA {
    fn from(value: Hlsa) -> Self {
        if value.saturation == 0. {
            Self::new(value.lightness, value.lightness, value.lightness, value.alpha)
        } else {
            let m2 = if value.lightness <= 0.5 {
                value.lightness * (1. + value.saturation)
            } else {
                value.lightness + value.saturation - value.lightness * value.saturation
            };
            let m1 = 2. * value.lightness - m2;

            let conv_color_from = |v: f64| {
                // Ensures 'v' is in 0..360 by wrapping, not clamping.
                let v = ((v % 360.) + 360.) % 360.;
                if v < 60. {
                    m1 + (m2 - m1) * v / 60.
                } else if v < 180. {
                    m2
                } else if v < 240. {
                    m1 + (m2 - m1) * (240. - v) / 60.
                } else {
                    m1
                }
            };

            Self::new(
                conv_color_from(value.hue + 120.),
                conv_color_from(value.hue),
                conv_color_from(value.hue - 120.),
                value.alpha,
            )
        }
    }
}
