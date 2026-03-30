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

//! Window snapping during move and resize operations.
//!
//! When a window is dragged near a screen border (or, in the future, near
//! another window's edge), the window position snaps to align exactly with
//! that border.  Each axis is handled independently: a window can snap
//! horizontally without affecting its vertical position, and vice versa.
//!
//! Because a window edge is a line segment that may span more than one
//! output (e.g. the left edge of a tall window straddling two
//! vertically-stacked monitors), we check each edge against every output
//! whose perpendicular range overlaps the frame.  This means a single edge
//! can produce snap candidates from multiple outputs; we pick the closest
//! one across all of them.

use smithay::utils::{Logical, Point, Rectangle, Size};

fn ranges_overlap(a_start: i32, a_end: i32, b_start: i32, b_end: i32) -> bool {
    a_start < b_end && b_start < a_end
}

/// Finds the best snap position for one axis.  Filters outputs to only those
/// that overlap the frame on the perpendicular axis, then checks both the near
/// and far edges of each overlapping output.  Returns the snapped frame origin
/// coordinate if the closest candidate is within snap_width, or None.
#[allow(clippy::too_many_arguments)]
fn snap_axis(
    frame_near: i32,
    frame_far: i32,
    frame_size: i32,
    frame_perp_near: i32,
    frame_perp_far: i32,
    output_geometries: &[Rectangle<i32, Logical>],
    axis_near: impl Fn(&Rectangle<i32, Logical>) -> i32,
    axis_far: impl Fn(&Rectangle<i32, Logical>) -> i32,
    perp_near: impl Fn(&Rectangle<i32, Logical>) -> i32,
    perp_far: impl Fn(&Rectangle<i32, Logical>) -> i32,
    snap_width: i32,
) -> Option<i32> {
    output_geometries
        .iter()
        .filter(|o| ranges_overlap(frame_perp_near, frame_perp_far, perp_near(o), perp_far(o)))
        .flat_map(|o| {
            let dist_near = (frame_near - axis_near(o)).abs();
            let dist_far = (frame_far - axis_far(o)).abs();
            [(dist_near, axis_near(o)), (dist_far, axis_far(o) - frame_size)]
        })
        .min_by_key(|(dist, _)| *dist)
        .and_then(|(dist, pos)| if dist <= snap_width { Some(pos) } else { None })
}

/// Finds the best adjacency snap position for one axis.  Unlike snap_axis
/// (which checks alignment: near<->near, far<->far), this checks adjacency:
/// near<->far and far<->near, so that windows butt up against each other.
#[allow(clippy::too_many_arguments)]
fn snap_axis_adjacent(
    frame_near: i32,
    frame_far: i32,
    frame_size: i32,
    frame_perp_near: i32,
    frame_perp_far: i32,
    rects: &[Rectangle<i32, Logical>],
    axis_near: impl Fn(&Rectangle<i32, Logical>) -> i32,
    axis_far: impl Fn(&Rectangle<i32, Logical>) -> i32,
    perp_near: impl Fn(&Rectangle<i32, Logical>) -> i32,
    perp_far: impl Fn(&Rectangle<i32, Logical>) -> i32,
    snap_width: i32,
) -> Option<i32> {
    rects
        .iter()
        .filter(|r| ranges_overlap(frame_perp_near, frame_perp_far, perp_near(r), perp_far(r)))
        .flat_map(|r| {
            let dist_near_to_far = (frame_near - axis_far(r)).abs();
            let dist_far_to_near = (frame_far - axis_near(r)).abs();
            [(dist_near_to_far, axis_far(r)), (dist_far_to_near, axis_near(r) - frame_size)]
        })
        .min_by_key(|(dist, _)| *dist)
        .and_then(|(dist, pos)| if dist <= snap_width { Some(pos) } else { None })
}

/// Snaps a proposed window position to nearby output (monitor) borders.
/// All coordinates are in frame space (decorations included).  The x and y
/// axes are computed independently: for each axis, we find the output edge
/// closest to either of the frame's two edges on that axis, and snap if
/// it's within snap_width.
pub(in crate::core) fn snap_move_to_border(
    proposed: Point<i32, Logical>,
    frame_size: Size<i32, Logical>,
    output_geometries: &[Rectangle<i32, Logical>],
    snap_width: i32,
) -> Point<i32, Logical> {
    let frame_left = proposed.x;
    let frame_right = proposed.x + frame_size.w;
    let frame_top = proposed.y;
    let frame_bottom = proposed.y + frame_size.h;

    let snap_x = snap_axis(
        frame_left,
        frame_right,
        frame_size.w,
        frame_top,
        frame_bottom,
        output_geometries,
        |o| o.loc.x,
        |o| o.loc.x + o.size.w,
        |o| o.loc.y,
        |o| o.loc.y + o.size.h,
        snap_width,
    );

    let snap_y = snap_axis(
        frame_top,
        frame_bottom,
        frame_size.h,
        frame_left,
        frame_right,
        output_geometries,
        |o| o.loc.y,
        |o| o.loc.y + o.size.h,
        |o| o.loc.x,
        |o| o.loc.x + o.size.w,
        snap_width,
    );

    (snap_x.unwrap_or(proposed.x), snap_y.unwrap_or(proposed.y)).into()
}

/// Snaps a single edge position during a resize operation.  Only the edge
/// being dragged is checked, against both the near and far edges of other
/// windows (so the resized edge can snap to either side of a neighbor).
pub(in crate::core) fn snap_resize_edge_to_windows(
    edge_pos: i32,
    perp_near: i32,
    perp_far: i32,
    other_windows: &[Rectangle<i32, Logical>],
    snap_width: i32,
    horizontal: bool,
) -> i32 {
    other_windows
        .iter()
        .filter(|r| {
            if horizontal {
                ranges_overlap(perp_near, perp_far, r.loc.y, r.loc.y + r.size.h)
            } else {
                ranges_overlap(perp_near, perp_far, r.loc.x, r.loc.x + r.size.w)
            }
        })
        .flat_map(|r| {
            let (near, far) = if horizontal {
                (r.loc.x, r.loc.x + r.size.w)
            } else {
                (r.loc.y, r.loc.y + r.size.h)
            };
            [((edge_pos - near).abs(), near), ((edge_pos - far).abs(), far)]
        })
        .min_by_key(|(dist, _)| *dist)
        .map_or(edge_pos, |(dist, pos)| if dist <= snap_width { pos } else { edge_pos })
}

/// Snaps a proposed window position to nearby window edges (adjacency
/// snapping).  Tests whether the moving window's edges are close to
/// butting up against another window's opposite edge (left<->right,
/// top<->bottom).  Like border snapping, uses perpendicular overlap to
/// avoid snapping to windows that are far away on the other axis.
pub(in crate::core) fn snap_move_to_windows(
    proposed: Point<i32, Logical>,
    frame_size: Size<i32, Logical>,
    other_windows: &[Rectangle<i32, Logical>],
    snap_width: i32,
) -> Point<i32, Logical> {
    let frame_left = proposed.x;
    let frame_right = proposed.x + frame_size.w;
    let frame_top = proposed.y;
    let frame_bottom = proposed.y + frame_size.h;

    let snap_x = snap_axis_adjacent(
        frame_left,
        frame_right,
        frame_size.w,
        frame_top,
        frame_bottom,
        other_windows,
        |r| r.loc.x,
        |r| r.loc.x + r.size.w,
        |r| r.loc.y,
        |r| r.loc.y + r.size.h,
        snap_width,
    );

    let snap_y = snap_axis_adjacent(
        frame_top,
        frame_bottom,
        frame_size.h,
        frame_left,
        frame_right,
        other_windows,
        |r| r.loc.y,
        |r| r.loc.y + r.size.h,
        |r| r.loc.x,
        |r| r.loc.x + r.size.w,
        snap_width,
    );

    (snap_x.unwrap_or(proposed.x), snap_y.unwrap_or(proposed.y)).into()
}
