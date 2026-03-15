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
    desktop::{Space, space::SpaceElement},
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point},
};

use crate::core::shell::WindowElement;

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
    pub(super) fn new<S: Into<String>>(name: S, position: Point<u32, Logical>) -> Self {
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

    pub fn id(&self) -> &str {
        &self.id
    }

    pub(super) fn set_name<S: AsRef<str>>(&mut self, name: S) {
        self.name = name.as_ref().to_owned();
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub(super) fn set_position(&mut self, position: Point<u32, Logical>) {
        self.position = position;
    }

    pub fn position(&self) -> Point<u32, Logical> {
        self.position
    }

    pub(super) fn set_active(&mut self, is_active: bool) {
        self.is_active = is_active;
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

    pub fn activate_window(&mut self, window: &WindowElement) {
        if self.element_location(window).is_some() {
            for elem in self.elements() {
                elem.set_activate(elem == window);
            }
        }
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
