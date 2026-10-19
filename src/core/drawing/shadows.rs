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

use std::{cell::RefCell, f64::consts::PI, sync::OnceLock};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer, Texture,
            element::{Id, Kind, texture::TextureRenderElement},
            gles::{
                GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName, UniformType, UniformValue, element::TextureShaderElement,
            },
        },
    },
    utils::{Physical, Point, Rectangle, Scale, Size, Transform},
};

use crate::core::{config::Xfwl4Config, drawing::ssd::place_piece};

const SHADOW_RADIUS: f64 = 12.;
const SHADOW_OFFSET_X: i32 = -3 * (SHADOW_RADIUS as i32) / 2;
const SHADOW_OFFSET_Y: i32 = -3 * (SHADOW_RADIUS as i32) / 2;
const SHADOW_CACHE_CAPACITY: usize = 4;

// Evaluates the shadow's alpha as the product of the horizontal and vertical windowed gaussian
// kernel masses inside the virtual window rect, looked up as differences of the cumulative kernel texture
// (C(b) - C(a) = mass in [a, b)).  Like xfwm4's make_shadow, whose mirrored corner and edge
// writes pin the falloff tails to the image's edges, the virtual rect is centered in the quad
// with size geo_size - gaussian size: the frame inset by the theme's shadow deltas, with the
// quad always the virtual rect expanded by the full kernel extent, so the falloff is never
// clipped.  Exact for any frame size.
const SHADOW_SHADER_SOURCE: &str = r#"
    //_DEFINES
    precision mediump float;
    varying vec2 v_coords;
    uniform sampler2D tex;      // 1D cumulative gaussian kernel mass, texel i: C(t = i - center)
    uniform float alpha;
    uniform vec2 tex_size;      // CDF texture size (gaussian size + 1, 1)
    uniform vec2 geo_size;      // destination shadow size in physical pixels
    uniform float opacity;      // quantized shadow opacity level, 0..25

    float seg(float t, float center) {
        return texture2D(tex, vec2((t + center + 0.5) / tex_size.x, 0.5)).r;
    }

    void main() {
        float center = (tex_size.x - 1.0) * 0.5;
        vec2 pixel = v_coords * geo_size;
        float fx = max(0.0, seg(geo_size.x - center - pixel.x, center) - seg(center - pixel.x, center));
        float fy = max(0.0, seg(geo_size.y - center - pixel.y, center) - seg(center - pixel.y, center));
        gl_FragColor = vec4(0.0, 0.0, 0.0, floor(floor(fx * fy * 255.0) * opacity / 25.0) / 255.0) * alpha;
    }
"#;

pub struct ShadowParams {
    pub offset: Point<i32, Physical>,
    pub size: Size<i32, Physical>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowKey {
    pub opacity: i32,
    pub delta_loc: Point<i32, Physical>,
    pub delta_width: i32,
    pub delta_height: i32,
}

impl ShadowKey {
    /// Key for a decorated (SSD) window's shadow.
    pub fn from_config(config: &Xfwl4Config) -> Self {
        ShadowKey {
            opacity: config.shadow_opacity(),
            delta_loc: (-config.shadow_delta_x(), -config.shadow_delta_y()).into(),
            delta_width: -config.shadow_delta_width(),
            delta_height: -config.shadow_delta_height(),
        }
    }

    /// Key for an undecorated window's shadow (popups, docks).
    ///
    /// Ignores the shadow_delta settings from the theme, as they're often tailored to the
    /// particular shape of the decoration pieces, which isn't relevant for these types of windows.
    pub fn for_undecorated(config: &Xfwl4Config) -> Self {
        ShadowKey {
            opacity: config.shadow_opacity(),
            delta_loc: (0, 0).into(),
            delta_width: 0,
            delta_height: 0,
        }
    }

    pub fn opacity_f64(&self) -> f64 {
        (self.opacity as f64 / 100.).clamp(0., 1.)
    }

    pub fn params_for(&self, surface_size: Size<i32, Physical>) -> ShadowParams {
        let gaussian = gaussian_map().size;
        ShadowParams {
            offset: (SHADOW_OFFSET_X + self.delta_loc.x, SHADOW_OFFSET_Y + self.delta_loc.y).into(),
            size: (
                surface_size.w + gaussian - self.delta_width - self.delta_loc.x,
                surface_size.h + gaussian - self.delta_height - self.delta_loc.y,
            )
                .into(),
        }
    }
}

/// The shadow for a given `ShadowKey`: a 1D cumulative-kernel texture, sampled by the shadow shader
/// to evaluate the exact separable gaussian blur at any surface size.  Mapping or resizing a window
/// never re-renders it.
#[derive(Debug, Clone)]
pub struct ShadowTextureSet {
    key: ShadowKey,
    offset: Point<i32, Physical>,
    texture: GlesTexture,
    shader: GlesTexProgram,
}

impl ShadowTextureSet {
    pub(super) fn render(renderer: &mut GlesRenderer, key: ShadowKey) -> Option<Self> {
        let map = gaussian_map();
        let g = map.size;

        // Texel i holds the cumulative kernel mass left of t = i - g/2, so the kernel mass inside
        // any span is a difference of two samples.
        let n = (g + 1) as usize;
        let mut cdf = vec![0f64; n];
        for y in 0..g as usize {
            for x in 0..g as usize {
                cdf[x + 1] += map.data[y * g as usize + x];
            }
        }
        for i in 1..n {
            cdf[i] += cdf[i - 1];
        }

        let mut data = Vec::with_capacity(n * 4);
        for mass in &cdf {
            let v = (mass * 255.) as u8;
            data.extend_from_slice(&[v, v, v, 255]);
        }

        let texture = renderer.import_memory(&data, Fourcc::Abgr8888, Size::new(n as i32, 1), false);
        let shader = renderer.compile_custom_texture_shader(
            SHADOW_SHADER_SOURCE,
            &[
                UniformName::new("tex_size", UniformType::_2f),
                UniformName::new("geo_size", UniformType::_2f),
                UniformName::new("opacity", UniformType::_1f),
            ],
        );

        match (texture, shader) {
            (Ok(texture), Ok(shader)) => Some(Self {
                key,
                offset: (SHADOW_OFFSET_X + key.delta_loc.x, SHADOW_OFFSET_Y + key.delta_loc.y).into(),
                texture,
                shader,
            }),
            (Err(err), _) | (_, Err(err)) => {
                tracing::warn!("Failed to set up shadow rendering: {err}");
                None
            }
        }
    }

    pub fn render_element(
        &self,
        renderer: &GlesRenderer,
        location: Point<f64, Physical>,
        scale: Scale<f64>,
        alpha: f32,
        surface_size: Size<i32, Physical>,
    ) -> TextureShaderElement {
        let params = self.key.params_for(surface_size);
        let origin = location + self.offset.to_f64();
        let (element_location, element_size) = place_piece(Rectangle::new((0, 0).into(), params.size), origin, scale);

        let tex_size = self.texture.size().to_f64();
        let src = Rectangle::new(Point::default(), self.texture.size())
            .to_logical(1, Transform::Normal, &self.texture.size())
            .to_f64();
        let element = TextureRenderElement::from_static_texture(
            Id::new(),
            renderer.context_id(),
            element_location,
            self.texture.clone(),
            1,
            Transform::Normal,
            Some(alpha),
            Some(src),
            Some(element_size),
            None,
            Kind::Unspecified,
        );

        // The shadow shader works in physical pixels, so geo_size is the destination size in
        // physical pixels — not the logical render size, which differs at fractional scale.
        let geo_size = params.size.to_f64();
        // Match the lookup tables' opacity quantization (26 levels, 0..25).
        let opacity_int = (self.key.opacity_f64() * 25.) as i32;
        let uniforms = [
            Uniform::new("tex_size", UniformValue::_2f(tex_size.w as f32, tex_size.h as f32)),
            Uniform::new("geo_size", UniformValue::_2f(geo_size.w as f32, geo_size.h as f32)),
            Uniform::new("opacity", UniformValue::_1f(opacity_int as f32)),
        ]
        .to_vec();

        TextureShaderElement::new(element, self.shader.clone(), uniforms)
    }
}

#[derive(Debug, Clone)]
pub struct ShadowCache(pub RefCell<Vec<ShadowTextureSet>>);

impl ShadowCache {
    pub fn new() -> Self {
        ShadowCache(RefCell::new(Vec::new()))
    }

    pub fn get(&self, key: ShadowKey) -> Option<ShadowTextureSet> {
        self.0.borrow().iter().find(|inner| inner.key == key).cloned()
    }

    pub fn set(&self, set: ShadowTextureSet) {
        let mut cache = self.0.borrow_mut();
        cache.retain(|inner| inner.key != set.key);
        if cache.len() >= SHADOW_CACHE_CAPACITY {
            cache.remove(0);
        }
        cache.push(set);
    }

    pub fn clear(&self) {
        self.0.borrow_mut().clear();
    }

    pub fn render_element(
        &self,
        key: ShadowKey,
        renderer: &mut GlesRenderer,
        location: Point<f64, Physical>,
        scale: Scale<f64>,
        alpha: f32,
        surface_size: Size<i32, Physical>,
    ) -> Option<TextureShaderElement> {
        if let Some(set) = self.get(key) {
            Some(set.render_element(renderer, location, scale, alpha, surface_size))
        } else if let Some(set) = ShadowTextureSet::render(renderer, key) {
            let element = set.render_element(renderer, location, scale, alpha, surface_size);
            self.set(set);
            Some(element)
        } else {
            None
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

fn gaussian_map() -> &'static GaussianConv {
    static MAP: OnceLock<GaussianConv> = OnceLock::new();
    MAP.get_or_init(|| make_gaussian_map(SHADOW_RADIUS))
}
