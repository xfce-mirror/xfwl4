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

use std::collections::HashSet;

use smithay::{
    desktop::space::{RenderZindex, SpaceElement},
    output::Output,
    reexports::{calloop::LoopHandle, wayland_server::DisplayHandle},
    utils::{Logical, Point, Rectangle, Size},
};
use xfconf::ChannelExtManual;

use crate::{
    backend::Backend,
    core::{
        config::XFWM4_CHANNEL_NAME,
        shell::WindowElement,
        state::Xfwl4State,
        util::{CalloopXfconfSource, Direction, ScrollAccumulator, zip_all_first},
        workspaces::Workspace,
    },
    protocols::ext_workspace::{
        ExtWorkspaceHandler, ExtWorkspaceState, WorkspaceChangedInput, WorkspaceCreatedInput, delegate_ext_workspace,
    },
};

const PROP_WORKSPACE_COUNT: &str = "/general/workspace_count";
const PROP_WORKSPACE_NAMES: &str = "/general/workspace_names";
const PROP_WORKSPACE_NROWS: &str = "/general/workspace_nrows";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WindowStackingLayer {
    AlwaysOnBottom = RenderZindex::Bottom as u8,
    Normal = RenderZindex::Shell as u8,
    AlwaysOnTop = RenderZindex::Top as u8,
    Overlay = RenderZindex::Overlay as u8,
    System = 255,
}

pub struct WorkspaceManager<BackendData: Backend + 'static> {
    channel: xfconf::Channel,
    workspaces: Vec<Workspace>,
    active_space: u32,
    geometry: Size<u32, Logical>,

    scroll_accum: ScrollAccumulator,

    ext_workspace_state: ExtWorkspaceState<Xfwl4State<BackendData>>,
}

impl<BackendData: Backend + 'static> WorkspaceManager<BackendData> {
    pub fn new(dh: &DisplayHandle, loop_handle: &LoopHandle<'static, Xfwl4State<BackendData>>) -> Self {
        let mut manager = Self {
            channel: xfconf::Channel::new(XFWM4_CHANNEL_NAME),
            workspaces: Default::default(),
            active_space: 0,
            geometry: (1, 1).into(),
            scroll_accum: ScrollAccumulator::default(),
            ext_workspace_state: ExtWorkspaceState::new(dh),
        };

        let source = CalloopXfconfSource::new(
            manager.channel.clone(),
            [PROP_WORKSPACE_COUNT, PROP_WORKSPACE_NAMES, PROP_WORKSPACE_NROWS],
        );
        loop_handle
            .insert_source(source, |(property_name, value), _, state| match property_name.as_str() {
                PROP_WORKSPACE_COUNT => {
                    if let Ok(new_count) = value.get::<i32>()
                        && new_count > 0
                    {
                        state.core.workspace_manager.on_workspace_count_changed(new_count as u32)
                    }
                }

                PROP_WORKSPACE_NAMES => {
                    if let Ok(new_names) = value.get::<xfconf::Array<String>>().map(|v| v.into_inner()) {
                        state.core.workspace_manager.on_workspace_names_changed(new_names)
                    }
                }

                PROP_WORKSPACE_NROWS => {
                    if let Ok(new_num_rows) = value.get::<i32>()
                        && new_num_rows > 0
                    {
                        state.core.workspace_manager.on_workspace_num_rows_changed(new_num_rows as u32)
                    }
                }
                _ => (),
            })
            .unwrap();

        manager.init_workspaces();
        manager.active_workspace_mut().set_active(true);

        manager
    }

    fn init_workspaces(&mut self) {
        let count = self
            .channel
            .get_property::<i32>(PROP_WORKSPACE_COUNT)
            .filter(|count| *count > 0)
            .unwrap_or(1) as u32;
        let names = self.get_workspace_names_uncached();
        let nrows = self
            .channel
            .get_property::<i32>(PROP_WORKSPACE_NROWS)
            .filter(|nrows| *nrows > 0)
            .unwrap_or(1) as u32;

        self.update_geometry(nrows, count);

        self.workspaces = zip_all_first(0..count, names)
            .map(|(i, name)| {
                let name = name.unwrap_or_else(|| format!("Workspace {}", i + 1));
                let position = position_for_workspace_index(i, self.geometry, count);
                Workspace::new(name, position)
            })
            .collect::<Vec<_>>();

        for (i, workspace) in self.workspaces.iter().enumerate() {
            self.ext_workspace_state.workspace_created(WorkspaceCreatedInput {
                id: workspace.id(),
                name: workspace.name(),
                coordinates: workspace.position(),
                is_active: self.active_space as usize == i,
            });
        }
    }

    pub fn map_output<P: Into<Point<i32, Logical>>>(&mut self, output: &Output, position: P) {
        let position = position.into();
        for workspace in self.workspaces.iter_mut() {
            workspace.map_output(output, position);
        }

        self.ext_workspace_state.output_enter(output);
    }

    pub fn unmap_output(&mut self, output: &Output) {
        for workspace in self.workspaces.iter_mut() {
            workspace.unmap_output(output);
        }

        self.ext_workspace_state.output_leave(output);
    }

    pub fn outputs(&self) -> impl Iterator<Item = &Output> {
        self.active_workspace().outputs()
    }

    pub fn output_geometry(&self, output: &Output) -> Option<Rectangle<i32, Logical>> {
        self.active_workspace().output_geometry(output)
    }

    pub fn output_under<P: Into<Point<f64, Logical>>>(&self, point: P) -> impl Iterator<Item = &Output> {
        self.active_workspace().output_under(point)
    }

    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }

    pub fn workspaces_mut(&mut self) -> &mut [Workspace] {
        &mut self.workspaces
    }

    pub fn set_active_workspace(&mut self, num: u32) {
        if (num as usize) < self.workspaces.len() && self.active_space != num {
            tracing::debug!("Switching active workspace from {} to {num}", self.active_space);

            if let Some(old_active_space) = self.workspaces.get_mut(self.active_space as usize) {
                old_active_space.set_active(false);
                self.ext_workspace_state.workspace_changed(
                    old_active_space.id(),
                    WorkspaceChangedInput {
                        name: None,
                        coordinates: None,
                        is_active: Some(false),
                    },
                );
            }

            self.active_space = num;

            if let Some(new_active_space) = self.workspaces.get_mut(self.active_space as usize) {
                new_active_space.set_active(true);
                self.ext_workspace_state.workspace_changed(
                    new_active_space.id(),
                    WorkspaceChangedInput {
                        name: None,
                        coordinates: None,
                        is_active: Some(true),
                    },
                );
            }
        }
    }

    pub fn scrolled_for_switch(&mut self, amount: f64) {
        let steps = self.scroll_accum.accumulate(amount);
        if steps != 0 {
            let is_next = steps > 0;
            for _ in 0..steps.abs() {
                if is_next {
                    self.activate_next();
                } else {
                    self.activate_previous();
                }
            }
        }
    }

    pub fn reset_scroll_amount(&mut self) {
        self.scroll_accum.reset();
    }

    /// Returns the workspace in the specified direction, or None if wrapping causes it to be the
    /// same as 'from_workspace'.
    fn position_for_direction(&self, from_workspace: &Workspace, direction: Direction) -> Option<Point<u32, Logical>> {
        let cur_pos = from_workspace.position();
        let cols = self.geometry.w;
        let rows = self.geometry.h;
        let n = self.workspaces.len() as u32;

        if n <= 1 {
            None
        } else {
            let (new_col, new_row) = match direction {
                Direction::Left => {
                    let col = if cur_pos.x > 0 { cur_pos.x - 1 } else { cols - 1 };
                    if workspace_index_for_position(col, cur_pos.y, self.geometry, n).is_some() {
                        (col, cur_pos.y)
                    } else {
                        let last_col = (n - 1) % cols;
                        (last_col, cur_pos.y)
                    }
                }
                Direction::Right => {
                    let col = (cur_pos.x + 1) % cols;
                    if workspace_index_for_position(col, cur_pos.y, self.geometry, n).is_some() {
                        (col, cur_pos.y)
                    } else {
                        (0, cur_pos.y)
                    }
                }
                Direction::Up => {
                    let mut row = if cur_pos.y > 0 { cur_pos.y - 1 } else { rows - 1 };
                    while workspace_index_for_position(cur_pos.x, row, self.geometry, n).is_none() {
                        row = if row > 0 { row - 1 } else { rows - 1 };
                    }
                    (cur_pos.x, row)
                }
                Direction::Down => {
                    let mut row = (cur_pos.y + 1) % rows;
                    while workspace_index_for_position(cur_pos.x, row, self.geometry, n).is_none() {
                        row = (row + 1) % rows;
                    }
                    (cur_pos.x, row)
                }
            };

            if new_col == cur_pos.x && new_row == cur_pos.y {
                None
            } else {
                Some((new_col, new_row).into())
            }
        }
    }

    fn activate_position(&mut self, col: u32, row: u32) {
        if let Some(new_idx) = workspace_index_for_position(col, row, self.geometry, self.workspaces.len() as u32) {
            self.set_active_workspace(new_idx);
        }
    }

    fn activate_direction(&mut self, direction: Direction) {
        if let Some(new_pos) = self.position_for_direction(self.active_workspace(), direction) {
            self.activate_position(new_pos.x, new_pos.y);
        }
    }

    pub fn activate_up(&mut self) {
        self.activate_direction(Direction::Up);
    }

    pub fn activate_down(&mut self) {
        self.activate_direction(Direction::Down);
    }

    pub fn activate_left(&mut self) {
        self.activate_direction(Direction::Left);
    }

    pub fn activate_right(&mut self) {
        self.activate_direction(Direction::Right);
    }

    pub fn activate_previous(&mut self) {
        if self.active_space > 0 {
            self.set_active_workspace(self.active_space - 1);
        } else {
            self.set_active_workspace(self.workspaces.len() as u32 - 1);
        }
    }

    pub fn activate_next(&mut self) {
        if self.active_space < self.workspaces.len() as u32 - 1 {
            self.set_active_workspace(self.active_space + 1);
        } else {
            self.set_active_workspace(0);
        }
    }

    pub fn active_workspace_index(&self) -> u32 {
        self.active_space
    }

    pub fn active_workspace(&self) -> &Workspace {
        self.workspaces
            .get(self.active_space as usize)
            .expect("active_space should not be out of range")
    }

    pub fn active_workspace_mut(&mut self) -> &mut Workspace {
        self.workspaces
            .get_mut(self.active_space as usize)
            .expect("active_space should not be out of range")
    }

    fn update_geometry(&mut self, nrows: u32, nworkspaces: u32) {
        self.geometry = (nworkspaces.div_ceil(nrows), nrows).into();
    }

    pub fn add_workspace(&mut self) {
        let count = self.workspaces.len();
        self.insert_workspace(count as u32);
    }

    pub fn insert_workspace(&mut self, index: u32) {
        let count = self.workspaces.len() as u32;

        if index == count {
            // Let the xfconf callbacks handle everything.
            self.set_xfconf_workspace_count(count + 1);
        } else {
            // If we're inserting at or below the current workspace, increment active_space so we
            // don't switch workspaces because of this.
            if index <= self.active_space {
                // This is one of the *only* times it's ok to set this directly and not go through
                // the setter.
                self.active_space += 1;
            }
            self.update_geometry(self.geometry.h, count + 1);

            let new_name = format!("Workspace {}", index + 1);

            // Insert the new workspace ourselves, because the xfconf handler will just append to
            // the end.
            let new_position = position_for_workspace_index(index, self.geometry, count + 1);
            let new_workspace = Workspace::new(&new_name, new_position);
            self.workspaces.insert(index as usize, new_workspace);
            self.set_xfconf_workspace_count(count + 1);

            // Add a new workspace name so the other workspaces don't change names.
            let mut names = self
                .workspaces
                .iter()
                .map(|workspace| workspace.name().to_owned())
                .collect::<Vec<_>>();
            names.insert(index as usize, new_name);
            self.set_xfconf_workspace_names(names);

            let workspace = self.workspaces.get(index as usize).unwrap();
            self.ext_workspace_state.workspace_created(WorkspaceCreatedInput {
                id: workspace.id(),
                name: workspace.name(),
                coordinates: workspace.position(),
                is_active: false,
            });

            // Now update all the other workspace coordinates.
            for (i, workspace) in self.workspaces.iter_mut().enumerate().map(|(i, workspace)| (i as u32, workspace)) {
                update_workspace_position(workspace, i, count + 1, self.geometry, &mut self.ext_workspace_state);
            }

            // Map any sticky windows into the new workspace.
            let (visible_sticky_windows, minimized_sticky_windows) = {
                let active_workspace = self.workspaces.get(self.active_space as usize).unwrap();
                let visible = active_workspace
                    .visible_windows()
                    .filter(|window| window.sticky())
                    .map(|window| (window.clone(), active_workspace.window_location(window).unwrap_or_default()))
                    .collect::<Vec<_>>();
                let minimized = active_workspace
                    .minimized_windows()
                    .filter(|window| window.sticky())
                    .map(|window| {
                        (
                            window.clone(),
                            active_workspace.minimized_window_location(window).unwrap_or_default(),
                        )
                    })
                    .collect::<Vec<_>>();
                (visible, minimized)
            };

            let workspace = self.workspaces.get_mut(index as usize).unwrap();
            for (window, location) in visible_sticky_windows {
                workspace.map_window(window, location, false);
            }
            for (window, location) in minimized_sticky_windows {
                workspace.add_minimized_window(window, location);
            }
        }
    }

    pub fn remove_workspace(&mut self, index: u32) {
        let count = self.workspaces.len() as u32;

        if count == 1 {
            // Never remove the last workspace.
        } else if index == count - 1 {
            // Let the xfconf callbacks handle everything.
            self.set_xfconf_workspace_count(count - 1);
        } else if index < count {
            let removed_workspace = self.workspaces.remove(index as usize);

            let target_workspace_index = index.saturating_sub(1);
            let target_workspace = self.workspaces.get_mut(target_workspace_index as usize).unwrap();

            // Move non-sticky windows to the target workspace.
            for window in removed_workspace.visible_windows().cloned() {
                if !window.sticky() {
                    let location = removed_workspace.window_location(&window).unwrap_or_default();
                    target_workspace.map_window(window, location, false);
                }
            }
            for window in removed_workspace.minimized_windows().cloned() {
                if !window.sticky() {
                    let location = removed_workspace.minimized_window_location(&window).unwrap_or_default();
                    target_workspace.add_minimized_window(window, location);
                }
            }

            self.set_xfconf_workspace_count(count - 1);
            // Update the workspace names list so other existing workspaces don't change names.
            let names = self
                .workspaces
                .iter()
                .map(|workspace| workspace.name().to_owned())
                .collect::<Vec<_>>();
            self.set_xfconf_workspace_names(names);

            self.ext_workspace_state.workspace_destroyed(removed_workspace.id());

            // Now update all the workspace coordinates.
            for (i, workspace) in self.workspaces.iter_mut().enumerate().map(|(i, workspace)| (i as u32, workspace)) {
                update_workspace_position(workspace, i, count - 1, self.geometry, &mut self.ext_workspace_state);
            }

            if self.active_space == index {
                // We removed the active workspace, so switch to the workspace where we moved all
                // the windows to.
                self.set_active_workspace(target_workspace_index);
            } else if self.active_space > index {
                //  We removed a workspace "before" the active one, so to keep ourselves on the
                //  active workspace, we have to decrement the active_space.  This is one of the
                //  *only* times it's ok to set this directly and not go through the setter.
                self.active_space -= 1;
            }
        }
    }

    pub fn refresh_spaces(&mut self) {
        profiling::scope!("refresh_spaces");
        for workspace in &mut self.workspaces {
            workspace.refresh();
        }
    }

    pub fn find_window<P>(&self, predicate: P) -> Option<WindowElement>
    where
        P: Fn(&WindowElement) -> bool + Clone,
    {
        self.workspaces
            .iter()
            .find_map(|workspace| workspace.find_window(predicate.clone()))
    }

    pub fn find_window_and_workspace_mut<P>(&mut self, predicate: P) -> Option<(&mut Workspace, WindowElement)>
    where
        P: Fn(&WindowElement) -> bool + Clone,
    {
        self.workspaces
            .iter_mut()
            .find_map(|workspace| workspace.find_window(predicate.clone()).map(|window| (workspace, window)))
    }

    pub fn outputs_for_window(&self, window: &WindowElement) -> Vec<Output> {
        self.workspaces
            .iter()
            .find_map(|workspace| {
                let outputs = workspace.outputs_for_window(window);
                (!outputs.is_empty()).then_some(outputs)
            })
            .unwrap_or_else(Vec::new)
    }

    fn workspace_for_window_with_index(&self, window: &WindowElement) -> Option<(u32, &Workspace)> {
        if self
            .active_workspace()
            .window_location(window)
            .or_else(|| self.active_workspace().minimized_window_location(window))
            .is_some()
        {
            Some((self.active_space, self.active_workspace()))
        } else {
            self.workspaces().iter().enumerate().find_map(|(i, workspace)| {
                workspace
                    .window_location(window)
                    .or_else(|| workspace.minimized_window_location(window))
                    .map(|_| (i as u32, workspace))
            })
        }
    }

    pub fn workspace_for_window(&mut self, window: &WindowElement) -> Option<&Workspace> {
        if self
            .active_workspace()
            .window_location(window)
            .or_else(|| self.active_workspace().minimized_window_location(window))
            .is_some()
        {
            Some(self.active_workspace())
        } else {
            self.workspaces().iter().find(|workspace| {
                workspace
                    .window_location(window)
                    .or_else(|| workspace.minimized_window_location(window))
                    .is_some()
            })
        }
    }

    pub fn workspace_for_window_mut(&mut self, window: &WindowElement) -> Option<&mut Workspace> {
        if self
            .active_workspace()
            .window_location(window)
            .or_else(|| self.active_workspace().minimized_window_location(window))
            .is_some()
        {
            Some(self.active_workspace_mut())
        } else {
            self.workspaces_mut().iter_mut().find(|workspace| {
                workspace
                    .window_location(window)
                    .or_else(|| workspace.minimized_window_location(window))
                    .is_some()
            })
        }
    }

    pub fn workspace_for_window_with_index_mut(&mut self, window: &WindowElement) -> Option<(u32, &mut Workspace)> {
        if self
            .active_workspace()
            .window_location(window)
            .or_else(|| self.active_workspace().minimized_window_location(window))
            .is_some()
        {
            Some((self.active_space, self.active_workspace_mut()))
        } else {
            self.workspaces_mut().iter_mut().enumerate().find_map(|(i, workspace)| {
                workspace
                    .window_location(window)
                    .or_else(|| workspace.minimized_window_location(window))
                    .map(|_| (i as u32, workspace))
            })
        }
    }

    fn move_window_by_index(&mut self, window: &WindowElement, old_index: u32, new_index: u32) -> bool {
        let count = self.workspaces.len() as u32;
        if old_index < count && new_index < count && old_index != new_index {
            if !window.sticky() {
                let workspace = self.workspaces.get_mut(old_index as usize).unwrap();
                let location = workspace.window_location(window).unwrap_or_default();
                workspace.unmap_window(window);

                let workspace = self.workspaces.get_mut(new_index as usize).unwrap();
                workspace.map_window(window.clone(), location, true);
            }

            true
        } else {
            false
        }
    }

    fn move_window_by_direction(&mut self, window: &WindowElement, direction: Direction) -> Option<u32> {
        let geometry = self.geometry;
        let count = self.workspaces.len() as u32;

        if let Some((old_index, workspace)) = self.workspace_for_window_with_index(window)
            && let Some(new_pos) = self.position_for_direction(workspace, direction)
            && let Some(new_index) = workspace_index_for_position(new_pos.x, new_pos.y, geometry, count)
        {
            self.move_window_by_index(window, old_index, new_index).then_some(new_index)
        } else {
            None
        }
    }

    pub fn move_window_up(&mut self, window: &WindowElement) -> Option<u32> {
        self.move_window_by_direction(window, Direction::Up)
    }

    pub fn move_window_down(&mut self, window: &WindowElement) -> Option<u32> {
        self.move_window_by_direction(window, Direction::Down)
    }

    pub fn move_window_left(&mut self, window: &WindowElement) -> Option<u32> {
        self.move_window_by_direction(window, Direction::Left)
    }

    pub fn move_window_right(&mut self, window: &WindowElement) -> Option<u32> {
        self.move_window_by_direction(window, Direction::Right)
    }

    pub fn move_window_to(&mut self, window: &WindowElement, new_index: u32) -> bool {
        if let Some((old_index, _)) = self.workspace_for_window_with_index(window) {
            self.move_window_by_index(window, old_index, new_index)
        } else {
            false
        }
    }

    pub fn move_window_next(&mut self, window: &WindowElement) -> Option<u32> {
        if let Some((old_index, _)) = self.workspace_for_window_with_index(window) {
            let new_index = if old_index == self.workspaces.len() as u32 - 1 {
                0
            } else {
                old_index + 1
            };
            self.move_window_by_index(window, old_index, new_index).then_some(new_index)
        } else {
            None
        }
    }

    pub fn move_window_previous(&mut self, window: &WindowElement) -> Option<u32> {
        if let Some((old_index, _)) = self.workspace_for_window_with_index(window) {
            let new_index = if old_index == 0 {
                self.workspaces.len() as u32 - 1
            } else {
                old_index - 1
            };
            self.move_window_by_index(window, old_index, new_index).then_some(new_index)
        } else {
            None
        }
    }

    pub fn new_window<P: Into<Point<i32, Logical>>>(
        &mut self,
        window: WindowElement,
        location: P,
        activate: bool,
        workspace_number: Option<u32>,
    ) {
        let workspace = if let Some(workspace) = workspace_number.and_then(|num| self.workspaces.get_mut(num as usize)) {
            workspace
        } else {
            self.workspaces.get_mut(self.active_space as usize).unwrap()
        };

        workspace.map_window(window, location, activate);
    }

    pub fn remove_window(&mut self, window: &WindowElement) {
        for workspace in self.workspaces_mut() {
            workspace.unmap_window(window);
            workspace.remove_minimized_window(window);
        }
    }

    pub fn relocate_window<P: Into<Point<i32, Logical>>>(&mut self, window: &WindowElement, location: P, activate: bool) {
        let location = location.into();
        for workspace in self.workspaces_mut() {
            workspace.relocate_window(window, location, activate);
        }
    }

    pub(super) fn set_window_stacking_layer(&mut self, window: &WindowElement, layer: WindowStackingLayer) {
        if window.z_index() != layer as u8 {
            window.0.override_z_index(layer as u8);

            if !window.sticky()
                && let Some(workspace) = self.workspace_for_window_mut(window)
            {
                workspace.raise_window(window, false);
            } else {
                for workspace in self.workspaces.iter_mut() {
                    workspace.raise_window(window, false);
                }
            }
        }
    }

    pub(super) fn set_window_minimized(&mut self, window: &WindowElement) -> bool {
        self.workspaces.iter_mut().fold(false, |did_minimize, workspace| {
            workspace.set_window_minimized(window) || did_minimize
        })
    }

    pub(super) fn set_window_unminimized(&mut self, window: &WindowElement, activate: bool) -> bool {
        self.workspaces.iter_mut().fold(false, |did_unminimize, workspace| {
            workspace.set_window_unminimized(window, activate) || did_unminimize
        })
    }

    pub(super) fn set_window_fullscreen(&mut self, window: &WindowElement, output: &Output) -> Vec<WindowElement> {
        if !window.sticky() {
            if let Some(workspace) = self.workspace_for_window_mut(window) {
                workspace.set_window_fullscreen(window, output).into_iter().collect()
            } else {
                vec![]
            }
        } else {
            self.workspaces
                .iter_mut()
                .flat_map(|workspace| workspace.set_window_fullscreen(window, output))
                .collect::<HashSet<_>>()
                .into_iter()
                .collect()
        }
    }

    pub(super) fn set_window_unfullscreen(&mut self, window: &WindowElement) -> Vec<Output> {
        if !window.sticky() {
            if let Some(workspace) = self.workspace_for_window_mut(window) {
                workspace.set_window_unfullscreen(window).into_iter().collect()
            } else {
                vec![]
            }
        } else {
            self.workspaces
                .iter_mut()
                .flat_map(|workspace| workspace.set_window_unfullscreen(window))
                .collect::<HashSet<_>>()
                .into_iter()
                .collect()
        }
    }

    pub(super) fn set_window_sticky(&mut self, window: &WindowElement, is_sticky: bool) {
        let is_minimized = window.minimized();

        if is_sticky {
            if let Some((ws_num, workspace)) = self.workspace_for_window_with_index(window)
                && let Some(location) = if !is_minimized {
                    workspace.window_location(window)
                } else {
                    workspace.minimized_window_location(window)
                }
            {
                for (i, workspace) in self.workspaces_mut().iter_mut().enumerate() {
                    if ws_num as usize != i {
                        if !is_minimized {
                            workspace.map_window(window.clone(), location, true);
                        } else {
                            workspace.add_minimized_window(window.clone(), location);
                        }
                    }
                }
            }
        } else {
            let active_ws_num = self.active_workspace_index() as usize;
            for (i, workspace) in self.workspaces_mut().iter_mut().enumerate() {
                if active_ws_num != i {
                    if !is_minimized {
                        workspace.unmap_window(window);
                    } else {
                        workspace.remove_minimized_window(window);
                    }
                }
            }
        }
    }

    pub fn window_geometry(&self, window: &WindowElement) -> Option<Rectangle<i32, Logical>> {
        self.workspaces.iter().find_map(|workspace| workspace.window_geometry(window))
    }

    fn get_workspace_names_uncached(&self) -> Vec<String> {
        self.channel
            .get_property::<Vec<String>>(PROP_WORKSPACE_NAMES)
            .unwrap_or_else(Vec::new)
    }

    fn set_xfconf_workspace_count(&self, num: u32) {
        self.channel.set_property(PROP_WORKSPACE_COUNT, num as i32);
    }

    fn set_xfconf_workspace_names(&self, names: Vec<String>) {
        self.channel.set_property(PROP_WORKSPACE_NAMES, names);
    }

    fn on_workspace_count_changed(&mut self, new_count: u32) {
        assert!(self.workspaces.len() <= i32::MAX as usize);
        let old_count = self.workspaces.len() as u32;
        self.update_geometry(self.geometry.h, new_count);

        if new_count > old_count {
            let names = self.get_workspace_names_uncached();

            let start = old_count;
            let new_workspaces = zip_all_first(start..new_count, names.into_iter().skip(start as usize)).map(|(i, name)| {
                let name = name.unwrap_or_else(|| format!("Workspace {}", i + 1));
                let position = position_for_workspace_index(i, self.geometry, new_count);
                Workspace::new(name, position)
            });

            self.workspaces.extend(new_workspaces);

            for (i, workspace) in self.workspaces.iter_mut().enumerate().map(|(i, workspace)| (i as u32, workspace)) {
                if i < old_count {
                    update_workspace_position(workspace, i, new_count, self.geometry, &mut self.ext_workspace_state);
                } else {
                    self.ext_workspace_state.workspace_created(WorkspaceCreatedInput {
                        id: workspace.id(),
                        name: workspace.name(),
                        coordinates: workspace.position(),
                        is_active: false,
                    });
                }
            }
        } else if new_count < old_count {
            let removed = self.workspaces.split_off(new_count as usize);
            let target_workspace = self.workspaces.last_mut().unwrap();

            for removed_workspace in removed.into_iter().rev() {
                // Move non-sticky windows to the target workspace.
                for window in removed_workspace.visible_windows().cloned() {
                    if !window.sticky() {
                        let location = removed_workspace.window_location(&window).unwrap_or_default();
                        target_workspace.map_window(window, location, false);
                    }
                }
                for window in removed_workspace.minimized_windows().cloned() {
                    if !window.sticky() {
                        let location = removed_workspace.minimized_window_location(&window).unwrap_or_default();
                        target_workspace.add_minimized_window(window, location);
                    }
                }

                self.ext_workspace_state.workspace_destroyed(removed_workspace.id());
            }

            for (i, workspace) in self.workspaces.iter_mut().enumerate().map(|(i, workspace)| (i as u32, workspace)) {
                update_workspace_position(workspace, i, new_count, self.geometry, &mut self.ext_workspace_state);
            }

            if self.active_space >= new_count {
                self.set_active_workspace(new_count - 1);
            }
        }
    }

    fn on_workspace_names_changed(&mut self, new_names: Vec<String>) {
        for (i, (workspace, new_name)) in zip_all_first(self.workspaces.iter_mut(), new_names).enumerate() {
            let new_name = new_name.unwrap_or_else(|| format!("Workspace {}", i + 1));
            if new_name != workspace.name() {
                workspace.set_name(new_name);
                self.ext_workspace_state.workspace_changed(
                    workspace.id(),
                    WorkspaceChangedInput {
                        name: Some(workspace.name()),
                        ..Default::default()
                    },
                );
            }
        }
    }

    fn on_workspace_num_rows_changed(&mut self, new_nrows: u32) {
        if new_nrows != self.geometry.h {
            let nworkspaces = self.workspaces.len() as u32;
            self.update_geometry(new_nrows, nworkspaces);

            for (i, workspace) in self.workspaces.iter_mut().enumerate().map(|(i, workspace)| (i as u32, workspace)) {
                update_workspace_position(workspace, i, nworkspaces, self.geometry, &mut self.ext_workspace_state);
            }
        }
    }
}

impl<BackendData: Backend + 'static> ExtWorkspaceHandler for Xfwl4State<BackendData> {
    fn ext_workspace_state(&mut self) -> &mut ExtWorkspaceState<Self> {
        &mut self.core.workspace_manager.ext_workspace_state
    }

    fn on_workspace_activate(&mut self, workspace_id: &str) {
        if let Some(workspace_num) = self
            .core
            .workspace_manager
            .workspaces
            .iter()
            .position(|workspace| workspace.id() == workspace_id)
        {
            self.core.workspace_manager.set_active_workspace(workspace_num as u32);
        }
    }

    fn on_workspace_deactivate(&mut self, _workspace_id: &str) {
        // We don't support deactivating a workspace without activating another, so we just do
        // nothing here.
    }
}

delegate_ext_workspace!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);

#[inline]
fn position_for_workspace_index(index: u32, geometry: Size<u32, Logical>, nworkspaces: u32) -> Point<u32, Logical> {
    debug_assert!(nworkspaces > 0);
    debug_assert!(geometry.w > 0);
    debug_assert!(geometry.h > 0);

    let y = index / geometry.w;
    let x = index % geometry.w;
    Point::new(x, y)
}

#[inline]
fn workspace_index_for_position(col: u32, row: u32, geometry: Size<u32, Logical>, nworkspaces: u32) -> Option<u32> {
    debug_assert!(nworkspaces > 0);
    debug_assert!(geometry.w > 0);
    debug_assert!(geometry.h > 0);

    if row < geometry.h && col < geometry.w {
        let index = row * geometry.w + col;
        (index < nworkspaces).then_some(index)
    } else {
        None
    }
}

fn update_workspace_position<BackendData: Backend + 'static>(
    workspace: &mut Workspace,
    index: u32,
    nworkspaces: u32,
    geometry: Size<u32, Logical>,
    ext_workspace_state: &mut ExtWorkspaceState<Xfwl4State<BackendData>>,
) {
    let new_position = position_for_workspace_index(index, geometry, nworkspaces);
    if new_position != workspace.position() {
        workspace.set_position(new_position);
        ext_workspace_state.workspace_changed(
            workspace.id(),
            WorkspaceChangedInput {
                coordinates: Some(new_position),
                ..Default::default()
            },
        );
    }
}
