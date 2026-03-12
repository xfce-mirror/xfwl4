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
// Based on portions of xfwm4/src/compositor.c, which is:
//
// Copyright (C) 2005-2021 Olivier Fourdan

use std::f64::consts::PI;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem,
            gles::{GlesRenderer, GlesTexture},
        },
    },
    utils::{Logical, Point, Size, Transform},
};

const SHADOW_RADIUS: f64 = 12.;
const SHADOW_OFFSET_X: i32 = -3 * (SHADOW_RADIUS as i32) / 2;
const SHADOW_OFFSET_Y: i32 = -3 * (SHADOW_RADIUS as i32) / 2;

pub struct ShadowParams {
    pub offset: Point<i32, Logical>,
    pub size: Size<i32, Logical>,
    pub surface_size: Size<i32, Logical>,
}

impl ShadowParams {
    pub fn new(delta_loc: Point<i32, Logical>, delta_width: i32, delta_height: i32, surface_size: Size<i32, Logical>) -> Self {
        let gaussian_size = ((SHADOW_RADIUS * 3.).ceil() as i32 + 1) & !1;
        ShadowParams {
            offset: (SHADOW_OFFSET_X + delta_loc.x, SHADOW_OFFSET_Y + delta_loc.y).into(),
            size: (
                surface_size.w + gaussian_size - delta_width - delta_loc.x,
                surface_size.h + gaussian_size - delta_height - delta_loc.y,
            )
                .into(),
            surface_size,
        }
    }
}

struct GaussianConv {
    size: i32,
    data: Vec<f64>,
}

fn gaussian(r: f64, x: f64, y: f64) -> f64 {
    (1. / (2. * PI * r).sqrt()) * (-(x * x + y * y) / (2. * r * r)).exp()
}

fn make_gaussian_map(r: f64) -> GaussianConv {
    let size = ((r * 3.).ceil() as i32 + 1) & !1;
    let center = size / 2;
    let mut data: Vec<f64> = (0..size)
        .flat_map(|y| (0..size).map(move |x| gaussian(r, (x - center) as f64, (y - center) as f64)))
        .collect();
    let total: f64 = data.iter().sum();
    for v in data.iter_mut() {
        *v /= total;
    }
    GaussianConv { size, data }
}

fn sum_gaussian(map: &GaussianConv, opacity: f64, x: i32, y: i32, width: i32, height: i32) -> u8 {
    let g_size = map.size as usize;
    let center = map.size / 2;
    let fx_start = (center - x).max(0) as usize;
    let fx_end = (width + center - x).min(map.size) as usize;
    let fy_start = (center - y).max(0) as usize;
    let fy_end = (height + center - y).min(map.size) as usize;

    let v: f64 = map
        .data
        .chunks(g_size)
        .skip(fy_start)
        .take(fy_end - fy_start)
        .filter_map(|row| row.get(fx_start..fx_end))
        .flatten()
        .sum();

    (v.min(1.) * opacity * 255.) as u8
}

struct ShadowLookup {
    shadow_corner: Vec<u8>,
    shadow_top: Vec<u8>,
    stride: usize,
}

impl ShadowLookup {
    fn corner(&self, opacity: i32, y: i32, x: i32) -> u8 {
        let idx = (opacity as usize) * self.stride * self.stride + (y as usize) * self.stride + x as usize;
        self.shadow_corner.get(idx).copied().unwrap_or_else(|| {
            tracing::error!(
                "BUG: shadow corner out of bounds: opacity={opacity}, y={y}, x={x}, stride={}",
                self.stride,
            );
            0
        })
    }

    fn set_corner(&mut self, opacity: i32, y: i32, x: i32, val: u8) {
        let idx = (opacity as usize) * self.stride * self.stride + (y as usize) * self.stride + x as usize;
        if let Some(slot) = self.shadow_corner.get_mut(idx) {
            *slot = val;
        } else {
            tracing::error!(
                "BUG: shadow corner set out of bounds: opacity={opacity}, y={y}, x={x}, stride={}",
                self.stride,
            );
        }
    }

    fn top(&self, opacity: i32, x: i32) -> u8 {
        let idx = (opacity as usize) * self.stride + x as usize;
        self.shadow_top.get(idx).copied().unwrap_or_else(|| {
            tracing::error!("BUG: shadow top out of bounds: opacity={opacity}, x={x}, stride={}", self.stride,);
            0
        })
    }

    fn set_top(&mut self, opacity: i32, x: i32, val: u8) {
        let idx = (opacity as usize) * self.stride + x as usize;
        if let Some(slot) = self.shadow_top.get_mut(idx) {
            *slot = val;
        } else {
            tracing::error!(
                "BUG: shadow top set out of bounds: opacity={opacity}, x={x}, stride={}",
                self.stride,
            );
        }
    }
}

fn presum_gaussian(map: &GaussianConv) -> ShadowLookup {
    let gaussian_size = map.size;
    let center = gaussian_size / 2;
    let stride = (gaussian_size + 1) as usize;

    let mut lookup = ShadowLookup {
        shadow_corner: vec![0u8; stride * stride * 26],
        shadow_top: vec![0u8; stride * 26],
        stride,
    };

    for x in 0..=gaussian_size {
        let base_top = sum_gaussian(map, 1., x - center, center, gaussian_size * 2, gaussian_size * 2);
        lookup.set_top(25, x, base_top);
        for opacity in 0..25 {
            lookup.set_top(opacity, x, (base_top as i32 * opacity / 25) as u8);
        }

        for y in 0..=x {
            let base_corner = sum_gaussian(map, 1., x - center, y - center, gaussian_size * 2, gaussian_size * 2);
            lookup.set_corner(25, y, x, base_corner);
            lookup.set_corner(25, x, y, base_corner);
            for opacity in 0..25 {
                let val = (base_corner as i32 * opacity / 25) as u8;
                lookup.set_corner(opacity, y, x, val);
                lookup.set_corner(opacity, x, y, val);
            }
        }
    }

    lookup
}

struct ShadowImage {
    data: Vec<u8>,
    width: i32,
}

impl ShadowImage {
    fn new(width: i32, height: i32, fill: u8) -> Self {
        ShadowImage {
            data: vec![fill; (width * height) as usize],
            width,
        }
    }

    fn set(&mut self, x: i32, y: i32, val: u8) {
        let idx = (y * self.width + x) as usize;
        if let Some(slot) = self.data.get_mut(idx) {
            *slot = val;
        } else {
            tracing::error!("shadow image set out of bounds: x={x}, y={y}, width={}", self.width,);
        }
    }
}

pub fn make_shadow(opacity: f64, params: &ShadowParams) -> Option<Vec<u8>> {
    let map = make_gaussian_map(SHADOW_RADIUS);
    let lookup = presum_gaussian(&map);
    let gaussian_size = map.size;
    let center = gaussian_size / 2;

    let swidth = params.size.w;
    let sheight = params.size.h;
    let size = params.surface_size;
    let opacity_int = (opacity * 25.) as i32;

    if swidth < 1 || sheight < 1 {
        None
    } else {
        let d = if gaussian_size > 0 {
            lookup.top(opacity_int, gaussian_size)
        } else {
            sum_gaussian(&map, opacity, center, center, size.w, size.h)
        };
        let mut img = ShadowImage::new(swidth, sheight, d);

        let ylimit = gaussian_size.min((sheight + 1) / 2);
        let xlimit = gaussian_size.min((swidth + 1) / 2);

        // corners
        for y in 0..ylimit {
            for x in 0..xlimit {
                let d = if xlimit == gaussian_size && ylimit == gaussian_size {
                    lookup.corner(opacity_int, y, x)
                } else {
                    sum_gaussian(&map, opacity, x - center, y - center, size.w, size.h)
                };

                img.set(x, y, d);
                img.set(x, sheight - y - 1, d);
                img.set(swidth - x - 1, sheight - y - 1, d);
                img.set(swidth - x - 1, y, d);
            }
        }

        // top/bottom edges
        let x_diff = swidth - (gaussian_size * 2);
        if x_diff > 0 && ylimit > 0 {
            for y in 0..ylimit {
                let d = if ylimit == gaussian_size {
                    lookup.top(opacity_int, y)
                } else {
                    sum_gaussian(&map, opacity, center, y - center, size.w, size.h)
                };
                for i in 0..x_diff {
                    img.set(gaussian_size + i, y, d);
                    img.set(gaussian_size + i, sheight - y - 1, d);
                }
            }
        }

        // sides
        for x in 0..xlimit {
            let d = if xlimit == gaussian_size {
                lookup.top(opacity_int, x)
            } else {
                sum_gaussian(&map, opacity, x - center, center, size.w, size.h)
            };
            for y in gaussian_size..(sheight - gaussian_size) {
                img.set(x, y, d);
                img.set(swidth - x - 1, y, d);
            }
        }

        Some(img.data)
    }
}

pub fn make_shadow_texture(renderer: &mut GlesRenderer, opacity: f64, params: &ShadowParams) -> anyhow::Result<Option<GlesTexture>> {
    if let Some(data) = make_shadow(opacity, params) {
        let rgba: Vec<u8> = data.iter().flat_map(|&alpha| [0, 0, 0, alpha]).collect();

        let size = params.size.to_buffer(1, Transform::Normal);
        let texture = renderer
            .import_memory(&rgba, Fourcc::Abgr8888, size, false)
            .map_err(|err| anyhow::anyhow!("{err}"))?;

        Ok(Some(texture))
    } else {
        Ok(None)
    }
}
