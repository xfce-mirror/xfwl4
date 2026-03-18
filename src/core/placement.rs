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
// Based on xfm4/src/placement.c, which is:
//
// Copyright (C) 2002-2022 Olivier Fourdan

use smithay::{
    desktop::{WindowSurface, layer_map_for_output, space::SpaceElement},
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Size},
};

use crate::{
    backend::Backend,
    core::{
        config::PlacementMode,
        shell::WindowElement,
        state::Xfwl4State,
        workspaces::{Workspace, WorkspaceManager},
    },
};

struct Frame {
    content_size: Size<i32, Logical>,
    frame_left: i32,
    frame_right: i32,
    frame_top: i32,
    frame_bottom: i32,
}

impl Frame {
    fn new(window: &WindowElement, content_size: Size<i32, Logical>) -> Self {
        if let Some(decorations) = window.decoration_state().window_decorations() {
            Self {
                content_size,
                frame_left: decorations.left_decoration_width(),
                frame_right: decorations.right_decoration_width(),
                frame_top: decorations.top_decoration_height(),
                frame_bottom: decorations.bottom_decoration_height(),
            }
        } else {
            match window.0.underlying_surface() {
                WindowSurface::Wayland(_) => {
                    let content_geo = window.0.geometry();
                    let bbox = window.0.bbox();
                    Self {
                        content_size,
                        frame_left: -(content_geo.loc.x - bbox.loc.x),
                        frame_right: -((bbox.loc.x + bbox.size.w) - (content_geo.loc.x + content_geo.size.w)),
                        frame_top: -(content_geo.loc.y - bbox.loc.y),
                        frame_bottom: -((bbox.loc.y + bbox.size.h) - (content_geo.loc.y + content_geo.size.h)),
                    }
                }
                #[cfg(feature = "xwayland")]
                WindowSurface::X11(_) => {
                    // TODO: check _NET_FRAME_EXTENTS / _GTK_FRAME_EXTENTS for CSD X11 windows
                    Self {
                        content_size,
                        frame_left: 0,
                        frame_right: 0,
                        frame_top: 0,
                        frame_bottom: 0,
                    }
                }
            }
        }
    }

    /// Left decoration margin. xfwm4: `frameExtentLeft(c)`.
    fn extent_left(&self) -> i32 {
        self.frame_left
    }

    /// Right decoration margin. xfwm4: `frameExtentRight(c)`.
    fn extent_right(&self) -> i32 {
        self.frame_right
    }

    /// Top decoration margin. xfwm4: `frameExtentTop(c)`.
    fn extent_top(&self) -> i32 {
        self.frame_top
    }

    /// Bottom decoration margin. xfwm4: `frameExtentBottom(c)`.
    fn extent_bottom(&self) -> i32 {
        self.frame_bottom
    }

    /// Total width including decorations. xfwm4: `frameExtentWidth(c)`.
    fn extent_width(&self) -> i32 {
        self.frame_left + self.content_size.w + self.frame_right
    }

    /// Total height including decorations. xfwm4: `frameExtentHeight(c)`.
    fn extent_height(&self) -> i32 {
        self.frame_top + self.content_size.h + self.frame_bottom
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub(in crate::core) fn place_window(&mut self, window: &WindowElement, content_size: Size<i32, Logical>, allow_activate: bool) {
        let is_new_window = self.core.workspace_manager.workspace_for_window_mut(window).is_none();
        let pointer_location = self.core.pointer.current_location();

        let frame = Frame::new(window, content_size);

        let output = self
            .core
            .workspace_manager
            .output_under(pointer_location)
            .next()
            .or_else(|| self.core.workspace_manager.outputs().next())
            .cloned();
        let output_geometry = output
            .and_then(|o| {
                let geo = self.core.workspace_manager.output_geometry(&o)?;
                let map = layer_map_for_output(&o);
                let zone = map.non_exclusive_zone();
                Some(Rectangle::new(geo.loc + zone.loc, zone.size))
            })
            .unwrap_or_else(|| Rectangle::from_size((800, 800).into()));

        let output_geometries = self
            .core
            .workspace_manager
            .outputs()
            .flat_map(|output| self.core.workspace_manager.output_geometry(output));

        let full_geometry = {
            let mut iter = output_geometries.into_iter();
            iter.next()
                .map(|first_geometry| iter.fold(first_geometry, |geometry_accum, geometry| geometry_accum.merge(geometry)))
                .unwrap_or_else(|| Rectangle::from_size((800, 800).into()))
        };

        let location = place_at_requested_location(window, &frame, output_geometry, full_geometry)
            .or_else(|| place_as_child_window(window, &frame, &self.core.workspace_manager))
            .or_else(|| place_at_existing_position(window, &frame))
            .unwrap_or_else(|| {
                let placement_ratio = self.core.config.placement_ratio();
                let placement_mode = self.core.config.placement_mode();

                if placement_ratio >= 100
                    || 100 * frame.extent_width() * frame.extent_height()
                        < placement_ratio * output_geometry.size.w * output_geometry.size.h
                {
                    if placement_mode == PlacementMode::Mouse {
                        place_under_pointer(&frame, output_geometry, pointer_location)
                    } else {
                        place_in_center(&frame, output_geometry)
                    }
                } else if frame.extent_width() >= output_geometry.size.w && frame.extent_height() >= output_geometry.size.h {
                    place_in_center(&frame, output_geometry)
                } else {
                    place_smartly(window, &frame, output_geometry, self.core.workspace_manager.active_workspace())
                }
            });

        // If the window is partiall off-screen in either dimension, try to move it left/up to get
        // it to fit, but don't push it farther left/up than the output area's bounds.
        let location: Point<_, Logical> = (
            if location.x + frame.extent_width() > output_geometry.loc.x + output_geometry.size.w {
                output_geometry.loc.x + output_geometry.size.w - frame.extent_width()
            } else {
                location.x
            }
            .max(output_geometry.loc.x),
            if location.y + frame.extent_height() > output_geometry.loc.y + output_geometry.size.h {
                output_geometry.loc.y + output_geometry.size.h - frame.extent_height()
            } else {
                location.y
            }
            .max(output_geometry.loc.y),
        )
            .into();

        if frame.extent_width() >= output_geometry.size.w && frame.extent_height() >= output_geometry.size.h && can_auto_maximize(window) {
            // If the window is larger than the output area's bounds in *both* dimensions, maximize
            // the window.
            if is_new_window {
                self.new_window(window.clone(), location, allow_activate, None);
            }
            self.set_window_maximized(window);
        } else if is_new_window {
            self.new_window(window.clone(), location, allow_activate, None);
        } else {
            self.core.workspace_manager.relocate_window(window, location, allow_activate);
            if allow_activate {
                self.focus_window(window, SERIAL_COUNTER.next_serial(), None);
            }
        }
    }
}

fn can_auto_maximize(window: &WindowElement) -> bool {
    match window.0.underlying_surface() {
        WindowSurface::Wayland(_) => true,
        #[cfg(feature = "xwayland")]
        WindowSurface::X11(surface) => {
            use smithay::xwayland::xwm::WmWindowType;
            surface.window_type().is_none_or(|ty| matches!(ty, WmWindowType::Normal))
        }
    }
}

/// Wayland doesn't generally allow clients to choose positions for their windows, but in the
/// legacy Xwayland case, X11 apps may rely on being able to do this in order to behave correctly.
/// So this function exists solely to allow X11 windows to set their own postition.
fn place_at_requested_location(
    window: &WindowElement,
    frame: &Frame,
    output_geometry: Rectangle<i32, Logical>,
    full_geometry: Rectangle<i32, Logical>,
) -> Option<Point<i32, Logical>> {
    match window.0.underlying_surface() {
        WindowSurface::Wayland(_) => None,
        #[cfg(feature = "xwayland")]
        WindowSurface::X11(surface) => {
            surface.size_hints().and_then(|hints| hints.position).map(|(_, x, y)| {
                use crate::core::util::RectangleExt;
                use smithay::xwayland::xwm::WmWindowType;

                // Some clients place dialogs at (0, 0).  Other clients aren't multihead aware
                // and try to put dialogs near the center of the full desktop.  Move them in both
                // cases to the center of the output.
                let is_dialog = surface.window_type().is_some_and(|ty| ty == WmWindowType::Dialog);
                if is_dialog && ((x == 0 && y == 0) || full_geometry.is_near_center(surface.geometry())) {
                    (
                        output_geometry.loc.x + (output_geometry.size.w / 2 - frame.extent_width() / 2),
                        output_geometry.loc.y + (output_geometry.size.h / 2 - frame.extent_height() / 2),
                    )
                        .into()
                } else {
                    (x, y).into()
                }
            })
        }
    }
}

/// Windows that are children of other windows should generally be placed in an "obvious" place
/// that's related to their parent, so we place them centered over their parent.
fn place_as_child_window<BackendData: Backend + 'static>(
    window: &WindowElement,
    frame: &Frame,
    workspace_manager: &WorkspaceManager<BackendData>,
) -> Option<Point<i32, Logical>> {
    window
        .parent()
        .and_then(|parent| workspace_manager.window_geometry(&parent))
        .map(|parent_geometry| {
            (
                parent_geometry.loc.x + parent_geometry.size.w / 2 - frame.extent_width() / 2,
                parent_geometry.loc.y + parent_geometry.size.h / 2 - frame.extent_height() / 2,
            )
                .into()
        })
}

/// This is an (unlikely) catch-all for edge cases around X11 dock/dialog/etc. type windows where
/// they've been mapped both without PPosition/UPosition set, and without a parent.  We still want
/// them to be placed where they intended to be placed (via the coordinates passed to the
/// XCreateWindow() call), so we handle that here.
fn place_at_existing_position(window: &WindowElement, frame: &Frame) -> Option<Point<i32, Logical>> {
    match window.0.underlying_surface() {
        WindowSurface::Wayland(_) => None,
        #[cfg(feature = "xwayland")]
        WindowSurface::X11(surface) => {
            use smithay::xwayland::xwm::WmWindowType;

            let type_matches = surface.window_type().is_some_and(|ty| {
                matches!(
                    ty,
                    WmWindowType::Desktop | WmWindowType::Dock | WmWindowType::Splash | WmWindowType::Utility | WmWindowType::Dialog
                )
            });
            let orphaned_transient = surface.is_transient_for().is_some() && !window.has_parent();

            (type_matches || orphaned_transient).then(|| {
                let location = surface.geometry().loc;
                (location.x - frame.frame_left, location.y - frame.frame_top).into()
            })
        }
    }
}

/// Places the window centered under the pointer.
fn place_under_pointer(
    frame: &Frame,
    output_geometry: Rectangle<i32, Logical>,
    pointer_location: Point<f64, Logical>,
) -> Point<i32, Logical> {
    let frame_width = frame.extent_width() as f64;
    let frame_height = frame.extent_height() as f64;
    let output_geometry = output_geometry.to_f64();

    Point::<f64, Logical>::new(
        (pointer_location.x - frame_width / 2.)
            .min(output_geometry.loc.x + output_geometry.size.w - frame_width)
            .max(output_geometry.loc.x),
        (pointer_location.y - frame_height / 2.)
            .min(output_geometry.loc.y + output_geometry.size.h - frame_height)
            .max(output_geometry.loc.y),
    )
    .to_i32_round()
}

/// Places in the center of the monitor.
fn place_in_center(frame: &Frame, output_geometry: Rectangle<i32, Logical>) -> Point<i32, Logical> {
    (
        (output_geometry.loc.x + (output_geometry.size.w - frame.extent_width()) / 2).max(output_geometry.loc.x),
        (output_geometry.loc.y + (output_geometry.size.h - frame.extent_height()) / 2).max(output_geometry.loc.y),
    )
        .into()
}

/// Smart placement tries to place windows with the minumum amount of overlap with other windows.
/// It's a bit of a slow process, as it goes through the list of existing windows over and over and
/// over, trying to find the best (lowest) set of overlaps.
fn place_smartly(
    window: &WindowElement,
    frame: &Frame,
    output_geometry: Rectangle<i32, Logical>,
    workspace: &Workspace,
) -> Point<i32, Logical> {
    let frame_left = frame.extent_left();
    let frame_top = frame.extent_top();
    let frame_size = Size::<_, Logical>::from((frame.extent_width(), frame.extent_height()));

    let max = Point::<_, Logical>::new(
        output_geometry.loc.x + output_geometry.size.w - frame.content_size.w - frame.extent_right(),
        output_geometry.loc.y + output_geometry.size.h - frame.content_size.h - frame.extent_bottom(),
    );
    let min = Point::<_, Logical>::new(output_geometry.loc.x + frame_left, output_geometry.loc.y + frame_top);

    let mut best_overlaps = f32::MAX;
    let mut best = min;

    let workspace_windows = workspace
        .visible_windows()
        .filter(|other| *other != window)
        .filter(|other| match other.0.underlying_surface() {
            WindowSurface::Wayland(_) => true,
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(surface) => {
                use smithay::xwayland::xwm::WmWindowType;
                surface.window_type().is_none_or(|ty| ty != WmWindowType::Desktop)
            }
        })
        .collect::<Vec<_>>();
    tracing::debug!("Analyzing {} windows", workspace_windows.len());

    let mut test = Point::<_, Logical>::new(0, min.y);
    'outer: loop {
        let mut next_test = Point::<_, Logical>::new(0, i32::MAX);
        let mut first_test_x = true;

        tracing::debug!("Testing y position {}", test.y);

        test.x = min.x;
        loop {
            let mut count_overlaps = 0f32;
            next_test.x = i32::MAX;
            let mut c2_next_test = Point::<_, Logical>::new(0, 0);

            tracing::debug!("Testing x position {}", test.x);

            for other in &workspace_windows {
                if let Some(other_geom) = workspace.window_geometry(other)
                    && output_geometry.intersection(other_geom).is_some()
                {
                    let other_loc = other_geom.loc;
                    let frame_other = Frame::new(other, SpaceElement::geometry(&other.0).size);

                    count_overlaps += overlap(
                        test.x - frame_left,
                        test.y - frame_top,
                        test.x - frame_left + frame_size.w,
                        test.y - frame_top + frame_size.h,
                        other_loc.x,
                        other_loc.y,
                        other_loc.x + frame_other.extent_width(),
                        other_loc.y + frame_other.extent_height(),
                    ) as f32;

                    // Find next x-bounds for the step, clamping to the coordinate of the right
                    // side of the window.
                    let other_x = if test.x > other_loc.x {
                        other_loc.x + frame_other.extent_width()
                    } else {
                        other_loc.x
                    };
                    c2_next_test.x = other_x.min(max.x);

                    if c2_next_test.x < next_test.x && c2_next_test.x > test.x {
                        next_test.x = c2_next_test.x;
                    }

                    if first_test_x {
                        // Find the next y-bounds for the step, clamping to the coordiate of the
                        // bottom side of the window.
                        let other_y = if test.y > other_loc.y {
                            other_geom.loc.y + frame_other.extent_height()
                        } else {
                            other_geom.loc.y
                        };
                        c2_next_test.y = other_y.min(max.y);

                        if c2_next_test.y < next_test.y && c2_next_test.y > test.y {
                            next_test.y = c2_next_test.y;
                        }
                    }
                }
            }

            first_test_x = false;

            if count_overlaps < best_overlaps {
                // Great, we found a position with fewer overlaps than what was previously the best
                // position found.
                best = test;
                best_overlaps = count_overlaps;

                if count_overlaps == 0. {
                    // Holy grail: zero overlaps.  No need to continue.
                    tracing::debug!("Found position without overlap");
                    break 'outer best;
                }
            }

            if next_test.x != i32::MAX {
                // Never go past the right edge of the monitor.
                test.x = next_test.x.max(next_test.x + frame_left).min(max.x);
            } else {
                test.x += 1;
            }

            if test.x > max.x {
                // Our x test position is past the right edge of the monitor, so continue on to the
                // next y test.
                break;
            }
        }

        if next_test.y != i32::MAX {
            // Never go past the bottom edge of the monitor.
            test.y = next_test.y.max(next_test.y + frame_top).min(max.y);
        } else {
            test.y += 1;
        }

        if test.y > max.y {
            // Our y test position is past the bottom edge of the monitor, so return whatever best
            // result we've gotten so far.
            break best;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn overlap(x0: i32, y0: i32, x1: i32, y1: i32, tx0: i32, ty0: i32, tx1: i32, ty1: i32) -> u64 {
    segment_overlap(x0, x1, tx0, tx1) * segment_overlap(y0, y1, ty0, ty1)
}

fn segment_overlap(x0: i32, x1: i32, tx0: i32, tx1: i32) -> u64 {
    let x0 = if tx0 > x0 { tx0 } else { x0 };
    let x1 = if tx1 < x1 { tx1 } else { x1 };

    (x1 - x0).max(0) as u64
}
