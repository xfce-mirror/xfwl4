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

use std::sync::LazyLock;

use smithay::{
    backend::renderer::element::{
        Element,
        utils::{CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement},
    },
    utils::{Physical, Point, Rectangle, Scale, Size},
};

use crate::core::util::ScrollAccumulator;

const MIN_ZOOM: f64 = 1.;
const MAX_ZOOM: f64 = 64.;
const ZOOM_STEPS: u32 = 15;
static ZOOM_MULTIPLIER: LazyLock<f64> = LazyLock::new(|| MAX_ZOOM.powf(1. / (ZOOM_STEPS as f64)));

pub type ZoomedRenderElement<E> = CropRenderElement<RelocateRenderElement<RescaleRenderElement<E>>>;

#[derive(Debug)]
pub struct ZoomState {
    level: f64,
    scroll_accum: ScrollAccumulator,
}

impl ZoomState {
    pub fn scrolled_for_zoom(&mut self, amount: f64) {
        let steps = self.scroll_accum.accumulate(amount);
        if steps != 0 {
            let is_zoom_in = steps < 0;
            for _ in 0..steps.abs() {
                if is_zoom_in {
                    self.zoom_step_in();
                } else {
                    self.zoom_step_out();
                }
            }
        }
    }

    pub fn reset_scroll_amount(&mut self) {
        self.scroll_accum.reset();
    }

    fn zoom_step_in(&mut self) {
        self.level = (self.level * *ZOOM_MULTIPLIER).clamp(MIN_ZOOM, MAX_ZOOM);
        if (self.level - MAX_ZOOM).abs() < 0.1 {
            // Rounding error gave us something very close to MAX_ZOOM, so just set it so we don't
            // end up having one more possible (but tiny) zoom step.
            self.level = MAX_ZOOM;
        }
    }

    fn zoom_step_out(&mut self) {
        self.level = (self.level / *ZOOM_MULTIPLIER).clamp(MIN_ZOOM, MAX_ZOOM);
        if (self.level - MIN_ZOOM).abs() < 0.1 {
            // Rounding error gave us something very close to MIN_ZOOM, so just set it so we don't
            // have funky slight-zoom.
            self.level = MIN_ZOOM;
        }
    }

    pub fn is_zoomed(&self) -> bool {
        self.level > 1.
    }

    fn compute_viewport(&self, pointer_location: Point<f64, Physical>, output_size: Size<i32, Physical>) -> Rectangle<f64, Physical> {
        let output_size = output_size.to_f64();
        let viewport_w = output_size.w / self.level;
        let viewport_h = output_size.h / self.level;

        let px = pointer_location.x.clamp(0.0, output_size.w);
        let py = pointer_location.y.clamp(0.0, output_size.h);
        let x = (px / output_size.w) * (output_size.w - viewport_w);
        let y = (py / output_size.h) * (output_size.h - viewport_h);

        Rectangle::new((x, y).into(), (viewport_w, viewport_h).into())
    }

    pub fn zoomed_render_elements<E>(
        &mut self,
        pointer_location: Point<f64, Physical>,
        output_size: Size<i32, Physical>,
        output_scale: f64,
        elements: Vec<E>,
    ) -> Vec<ZoomedRenderElement<E>>
    where
        E: Element,
    {
        let viewport = self.compute_viewport(pointer_location, output_size).to_i32_round::<i32>();
        let zoom_scale = Scale::from(self.level);
        let output_rect = Rectangle::from_size(output_size);
        let relocate_offset = Point::from((-viewport.loc.x, -viewport.loc.y));
        elements
            .into_iter()
            .map(|e| RescaleRenderElement::from_element(e, viewport.loc, zoom_scale))
            .map(|e| RelocateRenderElement::from_element(e, relocate_offset, Relocate::Relative))
            .filter_map(|e| CropRenderElement::from_element(e, output_scale, output_rect))
            .collect()
    }
}

impl Default for ZoomState {
    fn default() -> Self {
        Self {
            level: 1.,
            scroll_accum: ScrollAccumulator::default(),
        }
    }
}
