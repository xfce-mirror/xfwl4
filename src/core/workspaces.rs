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

use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
};

use smithay::{
    desktop::Space,
    output::Output,
    reexports::{
        calloop::LoopHandle,
        wayland_server::{DisplayHandle, protocol::wl_surface::WlSurface},
    },
    utils::{Logical, Point, Size},
};
use xfconf::ChannelExtManual;

use crate::{
    backend::Backend,
    core::{
        config::XFWM4_CHANNEL_NAME,
        shell::WindowElement,
        state::Xfwl4State,
        util::{CalloopXfconfSource, zip_all_first},
    },
    protocols::ext_workspace::{
        ExtWorkspaceHandler, ExtWorkspaceState, WorkspaceChangedInput, WorkspaceCreatedInput, delegate_ext_workspace,
    },
};

const PROP_WORKSPACE_COUNT: &str = "/general/workspace_count";
const PROP_WORKSPACE_NAMES: &str = "/general/workspace_names";
const PROP_WORKSPACE_NROWS: &str = "/general/workspace_nrows";

#[derive(Debug)]
struct MinimizedWindow {
    location: Point<i32, Logical>,
}

#[derive(Debug)]
pub struct Workspace {
    id: String,
    space: Space<WindowElement>,
    name: String,
    position: Point<u32, Logical>,
    is_active: bool,
    minimized_windows: HashMap<WindowElement, MinimizedWindow>,
    fullscreen_windows: HashMap<Output, WindowElement>,
}

impl Workspace {
    fn new<S: Into<String>>(name: S, position: Point<u32, Logical>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(), // TODO: make the IDs stable
            space: Default::default(),
            name: name.into(),
            position,
            is_active: false,
            minimized_windows: HashMap::new(),
            fullscreen_windows: HashMap::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn active(&self) -> bool {
        self.is_active
    }

    pub fn space(&self) -> &Space<WindowElement> {
        &self.space
    }

    pub fn space_mut(&mut self) -> &mut Space<WindowElement> {
        &mut self.space
    }

    pub fn find_element<P>(&self, predicate: P) -> Option<WindowElement>
    where
        P: Fn(&WindowElement) -> bool,
    {
        self.space
            .elements()
            .find(|e| predicate(e))
            .cloned()
            .or_else(|| self.minimized_windows.keys().find(|e| predicate(e)).cloned())
    }

    pub fn windows(&self) -> impl Iterator<Item = &WindowElement> {
        self.space.elements().chain(self.minimized_windows.keys())
    }

    pub fn raise_window(&mut self, window: &WindowElement, activate: bool) {
        if self.minimized_windows.contains_key(window) {
            self.set_window_unminimized(window, activate);
        }
        self.space.raise_element(window, activate);
    }

    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<WindowElement> {
        self.space
            .elements()
            .find(|window| window.wl_surface().map(|s| &*s == surface).unwrap_or(false))
            .cloned()
    }

    pub fn set_window_fullscreen(&mut self, window: &WindowElement, output: &Output) -> Option<WindowElement> {
        self.fullscreen_windows.insert(output.clone(), window.clone())
    }

    pub fn set_window_unfullscreen(&mut self, window: &WindowElement) -> Option<Output> {
        if let Some(output) = self
            .fullscreen_windows
            .iter()
            .find_map(|(output, a_window)| (window == a_window).then(|| output.clone()))
        {
            self.fullscreen_windows.remove(&output);
            Some(output)
        } else {
            None
        }
    }

    pub fn fullscreen_window_for_output(&self, output: &Output) -> Option<WindowElement> {
        self.fullscreen_windows.get(output).cloned()
    }

    pub fn set_window_minimized(&mut self, window: &WindowElement) -> bool {
        if let Some(location) = self.space.element_location(window) {
            self.space.unmap_elem(window);
            self.minimized_windows.insert(window.clone(), MinimizedWindow { location });
            window.update_minimized_state(true);
            true
        } else {
            false
        }
    }

    pub fn set_window_unminimized(&mut self, window: &WindowElement, activate: bool) -> bool {
        if let Some(data) = self.minimized_windows.remove(window) {
            self.space.map_element(window.clone(), data.location, activate);
            window.update_minimized_state(false);
            true
        } else {
            false
        }
    }
}

impl Deref for Workspace {
    type Target = Space<WindowElement>;

    fn deref(&self) -> &Self::Target {
        &self.space
    }
}

impl DerefMut for Workspace {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.space
    }
}

pub struct WorkspaceManager<BackendData: Backend + 'static> {
    channel: xfconf::Channel,
    workspaces: Vec<Workspace>,
    active_space: u32,
    geometry: Size<u32, Logical>,

    ext_workspace_state: ExtWorkspaceState<Xfwl4State<BackendData>>,
}

impl<BackendData: Backend + 'static> WorkspaceManager<BackendData> {
    pub fn new(dh: &DisplayHandle, loop_handle: &LoopHandle<'static, Xfwl4State<BackendData>>) -> Self {
        let mut manager = Self {
            channel: xfconf::Channel::new(XFWM4_CHANNEL_NAME),
            workspaces: Default::default(),
            active_space: 0,
            geometry: (1, 1).into(),
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
                        state.workspace_manager.on_workspace_count_changed(new_count as u32)
                    }
                }

                PROP_WORKSPACE_NAMES => {
                    if let Ok(new_names) = value.get::<xfconf::Array<String>>().map(|v| v.into_inner()) {
                        state.workspace_manager.on_workspace_names_changed(new_names)
                    }
                }

                PROP_WORKSPACE_NROWS => {
                    if let Ok(new_num_rows) = value.get::<i32>()
                        && new_num_rows > 0
                    {
                        state.workspace_manager.on_workspace_num_rows_changed(new_num_rows as u32)
                    }
                }
                _ => (),
            })
            .unwrap();

        manager.init_workspaces();
        manager.active_workspace_mut().is_active = true;

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
                id: &workspace.id,
                name: &workspace.name,
                coordinates: workspace.position,
                is_active: self.active_space as usize == i,
            });
        }
    }

    fn get_workspace_names_uncached(&self) -> Vec<String> {
        self.channel
            .get_property::<Vec<String>>(PROP_WORKSPACE_NAMES)
            .unwrap_or_else(Vec::new)
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
                old_active_space.is_active = false;
                self.ext_workspace_state.workspace_changed(
                    &old_active_space.id,
                    WorkspaceChangedInput {
                        name: None,
                        coordinates: None,
                        is_active: Some(false),
                    },
                );
            }

            self.active_space = num;

            if let Some(new_active_space) = self.workspaces.get_mut(self.active_space as usize) {
                new_active_space.is_active = true;
                self.ext_workspace_state.workspace_changed(
                    &new_active_space.id,
                    WorkspaceChangedInput {
                        name: None,
                        coordinates: None,
                        is_active: Some(true),
                    },
                );
            }
        }
    }

    fn activate_position(&mut self, col: u32, row: u32) {
        if let Some(new_idx) = workspace_index_for_position(col, row, self.geometry, self.workspaces.len() as u32) {
            self.set_active_workspace(new_idx);
        }
    }

    pub fn activate_up(&mut self) {
        let cur_pos = &self.active_workspace().position;
        if cur_pos.y > 0 {
            self.activate_position(cur_pos.x, cur_pos.y - 1);
        } else {
            self.activate_position(cur_pos.x, self.geometry.h - 1);
        }
    }

    pub fn activate_down(&mut self) {
        let cur_pos = &self.active_workspace().position;
        if cur_pos.y < self.geometry.h - 1 {
            self.activate_position(cur_pos.x, cur_pos.y + 1);
        } else {
            self.activate_position(cur_pos.x, 0);
        }
    }

    pub fn activate_left(&mut self) {
        let cur_pos = &self.active_workspace().position;
        if cur_pos.x > 0 {
            self.activate_position(cur_pos.x - 1, cur_pos.y);
        } else {
            self.activate_position(self.geometry.w - 1, cur_pos.y);
        }
    }

    pub fn activate_right(&mut self) {
        let cur_pos = &self.active_workspace().position;
        if cur_pos.x < self.geometry.w - 1 {
            self.activate_position(cur_pos.x + 1, cur_pos.y);
        } else {
            self.activate_position(0, cur_pos.y);
        }
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

    pub fn refresh_spaces(&mut self) {
        for workspace in &mut self.workspaces {
            workspace.space.refresh();
        }
    }

    pub fn find_element<P>(&self, predicate: P) -> Option<WindowElement>
    where
        P: Fn(&WindowElement) -> bool + Clone,
    {
        self.workspaces
            .iter()
            .find_map(|workspace| workspace.find_element(predicate.clone()))
    }

    pub fn outputs_for_element(&self, element: &WindowElement) -> Vec<Output> {
        self.workspaces
            .iter()
            .find_map(|workspace| {
                let outputs = workspace.outputs_for_element(element);
                (!outputs.is_empty()).then_some(outputs)
            })
            .unwrap_or_else(Vec::new)
    }

    pub fn workspace_for_window_mut(&mut self, window: &WindowElement) -> Option<&mut Workspace> {
        if self.active_workspace().element_location(window).is_some() {
            Some(self.active_workspace_mut())
        } else {
            self.workspaces_mut()
                .iter_mut()
                .find(|workspace| workspace.element_location(window).is_some())
        }
    }

    pub fn set_window_minimized(&mut self, window: &WindowElement) -> bool {
        self.workspaces.iter_mut().fold(false, |did_minimize, workspace| {
            workspace.set_window_minimized(window) || did_minimize
        })
    }

    pub fn set_window_unminimized(&mut self, window: &WindowElement, activate: bool) -> bool {
        self.workspaces.iter_mut().fold(false, |did_unminimize, workspace| {
            workspace.set_window_unminimized(window, activate) || did_unminimize
        })
    }

    fn update_geometry(&mut self, nrows: u32, nworkspaces: u32) {
        self.geometry = (nworkspaces.div_ceil(nrows), nrows).into();
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
                    let new_position = position_for_workspace_index(i, self.geometry, new_count);
                    if new_position != workspace.position {
                        workspace.position = new_position;
                        self.ext_workspace_state.workspace_changed(
                            &workspace.id,
                            WorkspaceChangedInput {
                                coordinates: Some(workspace.position),
                                ..Default::default()
                            },
                        );
                    }
                } else {
                    self.ext_workspace_state.workspace_created(WorkspaceCreatedInput {
                        id: &workspace.id,
                        name: &workspace.name,
                        coordinates: workspace.position,
                        is_active: false,
                    });
                }
            }
        } else if new_count < old_count {
            let removed = self.workspaces.split_off(new_count as usize);
            let target_workspace = self.workspaces.last_mut().unwrap();

            for mut workspace in removed.into_iter().rev() {
                let elems = workspace.elements().cloned().collect::<Vec<_>>();

                for elem in elems {
                    // Remove element from old workspace and remap on the last of the remaining
                    // workspaces.
                    let location = workspace.element_location(&elem).unwrap_or_else(|| (0, 0).into());
                    workspace.unmap_elem(&elem);
                    target_workspace.map_element(elem, location, false)
                }

                self.ext_workspace_state.workspace_destroyed(&workspace.id);
            }

            for (i, workspace) in self.workspaces.iter_mut().enumerate().map(|(i, workspace)| (i as u32, workspace)) {
                let new_position = position_for_workspace_index(i, self.geometry, new_count);
                if new_position != workspace.position {
                    workspace.position = new_position;
                    self.ext_workspace_state.workspace_changed(
                        &workspace.id,
                        WorkspaceChangedInput {
                            name: None,
                            coordinates: Some(workspace.position),
                            is_active: None,
                        },
                    );
                }
            }

            if self.active_space >= new_count {
                self.set_active_workspace(new_count - 1);
            }
        }
    }

    fn on_workspace_names_changed(&mut self, new_names: Vec<String>) {
        for (i, (workspace, new_name)) in zip_all_first(self.workspaces.iter_mut(), new_names).enumerate() {
            let new_name = new_name.unwrap_or_else(|| format!("Workspace {}", i + 1));
            if new_name != workspace.name {
                workspace.name = new_name;
                self.ext_workspace_state.workspace_changed(
                    &workspace.id,
                    WorkspaceChangedInput {
                        name: Some(&workspace.name),
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
                let new_position = position_for_workspace_index(i, self.geometry, nworkspaces);
                if new_position != workspace.position {
                    workspace.position = new_position;
                    self.ext_workspace_state.workspace_changed(
                        &workspace.id,
                        WorkspaceChangedInput {
                            coordinates: Some(workspace.position),
                            ..Default::default()
                        },
                    );
                }
            }
        }
    }
}

impl<BackendData: Backend + 'static> ExtWorkspaceHandler for Xfwl4State<BackendData> {
    fn ext_workspace_state(&mut self) -> &mut ExtWorkspaceState<Self> {
        &mut self.workspace_manager.ext_workspace_state
    }

    fn on_workspace_activate(&mut self, workspace_id: &str) {
        if let Some(workspace_num) = self
            .workspace_manager
            .workspaces
            .iter()
            .position(|workspace| workspace.id == workspace_id)
        {
            self.workspace_manager.set_active_workspace(workspace_num as u32);
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
