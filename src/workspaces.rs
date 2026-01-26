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

use std::ops::{Deref, DerefMut};

use smithay::{
    desktop::Space,
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Size},
};
use xfconf::ChannelExtManual;

use crate::{
    config::XFWM4_CHANNEL_NAME,
    shell::WindowElement,
    util::{CalloopXfconfSource, zip_all_first},
};

pub const PROP_WORKSPACE_COUNT: &str = "/general/workspace_count";
pub const PROP_WORKSPACE_NAMES: &str = "/general/workspace_names";
pub const PROP_WORKSPACE_NROWS: &str = "/general/workspace_nrows";

pub enum WorkspaceManagerEvent {
    CountChanged(u32),
    NamesChanged(Vec<String>),
    NumRowsChanged(u32),
}

pub enum WorkspaceChange<'a> {
    Added(&'a Workspace),
    Removed(Workspace),
    Name(&'a Workspace),
    Position(&'a Workspace),
}

#[derive(Debug, PartialEq)]
pub struct Workspace {
    space: Space<WindowElement>,
    name: String,
    position: Point<u32, Logical>,
}

impl Workspace {
    fn new<S: Into<String>>(name: S, position: Point<u32, Logical>) -> Self {
        Self {
            space: Default::default(),
            name: name.into(),
            position,
        }
    }

    pub fn space(&self) -> &Space<WindowElement> {
        &self.space
    }

    pub fn space_mut(&mut self) -> &mut Space<WindowElement> {
        &mut self.space
    }

    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<WindowElement> {
        self.space
            .elements()
            .find(|window| window.wl_surface().map(|s| &*s == surface).unwrap_or(false))
            .cloned()
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

#[derive(Debug)]
pub struct WorkspaceManager {
    channel: xfconf::Channel,
    workspaces: Vec<Workspace>,
    active_space: u32,
    geometry: Size<u32, Logical>,
}

impl WorkspaceManager {
    pub fn new() -> (Self, CalloopXfconfSource) {
        let mut manager = Self {
            channel: xfconf::Channel::new(XFWM4_CHANNEL_NAME),
            workspaces: Default::default(),
            active_space: 0,
            geometry: (1, 1).into(),
        };

        let notifier = CalloopXfconfSource::new(
            manager.channel.clone(),
            [PROP_WORKSPACE_COUNT, PROP_WORKSPACE_NAMES, PROP_WORKSPACE_NROWS],
        );

        manager.init_workspaces();

        (manager, notifier)
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
    }

    fn get_workspace_names_uncached(&self) -> Vec<String> {
        self.channel
            .get_property::<Vec<String>>(PROP_WORKSPACE_NAMES)
            .unwrap_or_else(Vec::new)
    }

    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }

    pub fn workspaces_mut(&mut self) -> &mut [Workspace] {
        &mut self.workspaces
    }

    pub fn set_active_workspace(&mut self, num: u32) {
        if (num as usize) < self.workspaces.len() {
            tracing::debug!("Switching active workspace to {num}");
            self.active_space = num;
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
        P: Fn(&WindowElement) -> bool,
    {
        self.workspaces
            .iter()
            .find_map(|workspace| workspace.elements().find(|e| predicate(e)).cloned())
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

    fn update_geometry(&mut self, nrows: u32, nworkspaces: u32) {
        self.geometry = (nworkspaces.div_ceil(nrows), nrows).into();
    }

    pub fn on_workspace_count_changed<'a>(&'a mut self, new_count: u32) -> Vec<WorkspaceChange<'a>> {
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

            self.workspaces
                .iter_mut()
                .enumerate()
                .map(|(i, workspace)| (i as u32, workspace))
                .flat_map(|(i, workspace)| {
                    if i < old_count {
                        let new_position = position_for_workspace_index(i, self.geometry, new_count);
                        if new_position != workspace.position {
                            workspace.position = new_position;
                            Some(WorkspaceChange::Position(&*workspace))
                        } else {
                            None
                        }
                    } else {
                        Some(WorkspaceChange::Added(&*workspace))
                    }
                })
                .collect()
        } else if new_count < old_count {
            let mut changes = Vec::new();

            let removed = self
                .workspaces
                .split_off(new_count as usize)
                .into_iter()
                .map(WorkspaceChange::Removed);
            // TODO: move windows from removed workspace to another one
            changes.extend(removed);

            for (i, workspace) in self.workspaces.iter_mut().enumerate().map(|(i, workspace)| (i as u32, workspace)) {
                let new_position = position_for_workspace_index(i, self.geometry, new_count);
                if new_position != workspace.position {
                    workspace.position = new_position;
                    changes.push(WorkspaceChange::Position(&*workspace));
                }
            }

            changes
        } else {
            Vec::new()
        }
    }

    pub fn on_workspace_names_changed<'a>(&'a mut self, new_names: Vec<String>) -> Vec<WorkspaceChange<'a>> {
        zip_all_first(self.workspaces.iter_mut(), new_names)
            .enumerate()
            .flat_map(|(i, (workspace, name))| {
                let name = name.unwrap_or_else(|| format!("Workspace {}", i + 1));
                if name != workspace.name {
                    workspace.name = name;
                    Some(WorkspaceChange::Name(&*workspace))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn on_workspace_num_rows_changed<'a>(&'a mut self, new_nrows: u32) -> Vec<WorkspaceChange<'a>> {
        if new_nrows != self.geometry.h {
            let nworkspaces = self.workspaces.len() as u32;
            self.update_geometry(new_nrows, nworkspaces);

            self.workspaces
                .iter_mut()
                .enumerate()
                .map(|(i, workspace)| (i as u32, workspace))
                .flat_map(|(i, workspace)| {
                    let new_position = position_for_workspace_index(i, self.geometry, nworkspaces);
                    if new_position != workspace.position {
                        workspace.position = new_position;
                        Some(WorkspaceChange::Position(&*workspace))
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        }
    }
}

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
