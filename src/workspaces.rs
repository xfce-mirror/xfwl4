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
    utils::{Logical, Point},
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
    CountChanged(usize),
    NamesChanged(Vec<String>),
    NumRowsChanged(usize),
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
    position: Point<usize, Logical>,
}

impl Workspace {
    fn new<S: Into<String>>(name: S, position: Point<usize, Logical>) -> Self {
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
    active_space: usize,
    nrows: usize,
}

impl WorkspaceManager {
    pub fn new() -> (Self, CalloopXfconfSource) {
        let mut manager = Self {
            channel: xfconf::Channel::new(XFWM4_CHANNEL_NAME),
            workspaces: Default::default(),
            active_space: 0,
            nrows: 1,
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
            .unwrap_or(1) as usize;
        let names = self.get_workspace_names_uncached();
        let nrows = self
            .channel
            .get_property::<i32>(PROP_WORKSPACE_NROWS)
            .filter(|nrows| *nrows > 0)
            .unwrap_or(1) as usize;

        self.nrows = nrows;

        self.workspaces = zip_all_first(0..count, names)
            .map(|(i, name)| {
                let name = name.unwrap_or_else(|| format!("Workspace {}", i + 1));
                let position = position_for_workspace_index(i, nrows, count);
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

    pub fn set_active_workspace(&mut self, num: usize) {
        if num < self.workspaces.len() {
            self.active_space = num;
        }
    }

    pub fn active_workspace(&self) -> &Workspace {
        self.workspaces
            .get(self.active_space)
            .expect("active_space should not be out of range")
    }

    pub fn active_workspace_mut(&mut self) -> &mut Workspace {
        self.workspaces
            .get_mut(self.active_space)
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

    pub fn on_workspace_count_changed<'a>(&'a mut self, new_count: usize) -> Vec<WorkspaceChange<'a>> {
        if new_count > self.workspaces.len() {
            let old_count = self.workspaces.len();
            let names = self.get_workspace_names_uncached();
            let nrows = self.nrows;

            let start = self.workspaces.len();
            let new_workspaces = zip_all_first(start..new_count, names.into_iter().skip(start)).map(|(i, name)| {
                let name = name.unwrap_or_else(|| format!("Workspace {}", i + 1));
                let position = position_for_workspace_index(i, nrows, new_count);
                Workspace::new(name, position)
            });

            self.workspaces.extend(new_workspaces);

            self.workspaces
                .iter_mut()
                .enumerate()
                .flat_map(|(i, workspace)| {
                    if i < old_count {
                        let new_position = position_for_workspace_index(i, self.nrows, new_count);
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
        } else if new_count < self.workspaces.len() {
            let mut changes = Vec::new();

            let removed = self.workspaces.split_off(new_count).into_iter().map(WorkspaceChange::Removed);
            // TODO: move windows from removed workspace to another one
            changes.extend(removed);

            for (i, workspace) in self.workspaces.iter_mut().enumerate() {
                let new_position = position_for_workspace_index(i, self.nrows, new_count);
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

    pub fn on_workspace_num_rows_changed<'a>(&'a mut self, new_nrows: usize) -> Vec<WorkspaceChange<'a>> {
        if new_nrows != self.nrows {
            self.nrows = new_nrows;

            let nworkspaces = self.workspaces.len();
            self.workspaces
                .iter_mut()
                .enumerate()
                .flat_map(|(i, workspace)| {
                    let new_position = position_for_workspace_index(i, new_nrows, nworkspaces);
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

fn position_for_workspace_index(index: usize, nrows: usize, nworkspaces: usize) -> Point<usize, Logical> {
    debug_assert!(nworkspaces > 0);
    debug_assert!(nrows > 0);

    let cols = nworkspaces.div_ceil(nrows);
    let y = index / cols;
    let x = index % cols;
    Point::new(x, y)
}
