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

//! Screen edge resistance for pointer-driven workspace switching.
//!
//! Tracks when the pointer hits a screen edge and accumulates a motion
//! counter.  The counter increments each time the pointer is at the exact
//! edge pixel, resets after a 250ms gap, and triggers when it exceeds the
//! configured threshold.  A perpendicular drift check (15px) ensures the
//! user is intentionally pushing against the edge, not just sliding along
//! it.
//!
//! This module is purely computational with no side effects.  Callers
//! decide what to do when resistance is overcome (switch workspaces,
//! move a window, etc.).

use smithay::utils::{Logical, Point, Rectangle};

const EDGE_TIMEOUT_MSEC: u32 = 250;
const MAX_DRIFT: f64 = 15.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::core) enum ScreenEdge {
    Left,
    Right,
    Top,
    Bottom,
}

pub(in crate::core) struct EdgeResistanceState {
    active_edge: Option<ScreenEdge>,
    hit_count: i32,
    last_hit_time: u32,
    initial_perpendicular: f64,
}

impl EdgeResistanceState {
    pub(in crate::core) fn new() -> Self {
        Self {
            active_edge: None,
            hit_count: 0,
            last_hit_time: 0,
            initial_perpendicular: 0.0,
        }
    }

    fn reset(&mut self) {
        self.active_edge = None;
        self.hit_count = 0;
    }

    /// Called on each pointer motion event.  Returns Some(edge) when the
    /// resistance threshold has been overcome.
    ///
    /// unclamped is the pointer position before clamping to outputs;
    /// clamped is the position after clamping.  The difference tells us
    /// whether the pointer was trying to move past a screen edge.
    pub(in crate::core) fn update(
        &mut self,
        unclamped: Point<f64, Logical>,
        _clamped: Point<f64, Logical>,
        bbox: &Rectangle<i32, Logical>,
        time_msec: u32,
        threshold: i32,
    ) -> Option<ScreenEdge> {
        if let Some((edge, perpendicular)) = detect_edge(unclamped, bbox) {
            if self.active_edge != Some(edge) || time_msec.wrapping_sub(self.last_hit_time) > EDGE_TIMEOUT_MSEC {
                self.active_edge = Some(edge);
                self.hit_count = 1;
                self.initial_perpendicular = perpendicular;
                self.last_hit_time = time_msec;
                None
            } else if (perpendicular - self.initial_perpendicular).abs() > MAX_DRIFT {
                self.reset();
                None
            } else {
                self.hit_count += 1;
                self.last_hit_time = time_msec;
                if self.hit_count > threshold {
                    self.reset();
                    Some(edge)
                } else {
                    None
                }
            }
        } else {
            self.reset();
            None
        }
    }
}

fn detect_edge(unclamped: Point<f64, Logical>, bbox: &Rectangle<i32, Logical>) -> Option<(ScreenEdge, f64)> {
    let left = bbox.loc.x as f64;
    let right = (bbox.loc.x + bbox.size.w) as f64;
    let top = bbox.loc.y as f64;
    let bottom = (bbox.loc.y + bbox.size.h) as f64;

    if unclamped.x < left {
        Some((ScreenEdge::Left, unclamped.y))
    } else if unclamped.x >= right {
        Some((ScreenEdge::Right, unclamped.y))
    } else if unclamped.y < top {
        Some((ScreenEdge::Top, unclamped.x))
    } else if unclamped.y >= bottom {
        Some((ScreenEdge::Bottom, unclamped.x))
    } else {
        None
    }
}
