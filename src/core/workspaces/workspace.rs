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

use std::collections::HashMap;

use smithay::{
    backend::renderer::{ImportAll, ImportMem, Renderer, RendererSuper, Texture, element::AsRenderElements},
    desktop::{Space, space::SpaceElement},
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle, Scale},
};

use crate::{
    backend::{AsGlesRenderer, FromGlesError},
    core::shell::WindowElement,
};

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
    active_window: Option<WindowElement>,
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
            active_window: None,
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

        if is_active {
            for window in self.visible_windows() {
                window.0.set_activate(self.active_window.as_ref() == Some(window));
            }
        }
    }

    pub fn active(&self) -> bool {
        self.is_active
    }

    pub(super) fn outputs(&self) -> impl Iterator<Item = &Output> {
        self.space.outputs()
    }

    pub(super) fn output_geometry(&self, output: &Output) -> Option<Rectangle<i32, Logical>> {
        self.space.output_geometry(output)
    }

    pub(super) fn output_under<P: Into<Point<f64, Logical>>>(&self, point: P) -> impl Iterator<Item = &Output> {
        self.space.output_under(point)
    }

    pub(super) fn map_output<P: Into<Point<i32, Logical>>>(&mut self, output: &Output, position: P) {
        self.space.map_output(output, position);
    }

    pub(super) fn unmap_output(&mut self, output: &Output) {
        self.space.unmap_output(output);
    }

    pub fn find_window<P>(&self, predicate: P) -> Option<WindowElement>
    where
        P: Fn(&WindowElement) -> bool,
    {
        self.space
            .elements()
            .find(|e| predicate(e))
            .cloned()
            .or_else(|| self.minimized_windows.keys().find(|e| predicate(e)).cloned())
    }

    pub fn window_under<P: Into<Point<f64, Logical>>>(&self, point: P) -> Option<(&WindowElement, Point<i32, Logical>)> {
        self.space.element_under(point)
    }

    pub(super) fn map_window<P: Into<Point<i32, Logical>>>(&mut self, window: WindowElement, location: P, activate: bool) {
        if activate {
            self.active_window = Some(window.clone());
        }
        self.space.map_element(window, location, activate);
    }

    pub fn raise_window(&mut self, window: &WindowElement, activate: bool) {
        if self.minimized_windows.contains_key(window) {
            self.set_window_unminimized(window, activate);
        }

        if self.window_location(window).is_some() {
            if activate {
                self.active_window = Some(window.clone());
            }
            self.space.raise_element(window, activate);
        }
    }

    pub(super) fn relocate_window<P: Into<Point<i32, Logical>>>(&mut self, window: &WindowElement, location: P, activate: bool) {
        if let Some(cur_location) = self.window_location(window) {
            if activate {
                self.active_window = Some(window.clone());
            }

            let location = location.into();
            if activate {
                self.space.map_element(window.clone(), location, activate);
            } else if location != cur_location {
                self.space.relocate_element(window, location);
            }
        }
    }

    pub(super) fn unmap_window(&mut self, window: &WindowElement) {
        self.set_window_unfullscreen(window);
        self.space.unmap_elem(window);
        if self.active_window.as_ref() == Some(window) {
            self.active_window = None;
        }
    }

    pub fn window_location(&self, window: &WindowElement) -> Option<Point<i32, Logical>> {
        self.space.element_location(window)
    }

    pub fn window_bbox(&self, window: &WindowElement) -> Option<Rectangle<i32, Logical>> {
        self.space.element_bbox(window)
    }

    pub fn window_geometry(&self, window: &WindowElement) -> Option<Rectangle<i32, Logical>> {
        self.space.element_geometry(window)
    }

    pub fn outputs_for_window(&self, window: &WindowElement) -> Vec<Output> {
        self.space.outputs_for_element(window)
    }

    pub fn activate_window(&mut self, window: &WindowElement) {
        if self.window_location(window).is_some() {
            for elem in self.visible_windows() {
                elem.set_activate(elem == window);
            }

            self.active_window = Some(window.clone());
        }
    }

    pub fn all_windows(&self) -> impl Iterator<Item = &WindowElement> {
        self.space.elements().chain(self.minimized_windows.keys())
    }

    pub fn visible_windows(&self) -> impl DoubleEndedIterator<Item = &WindowElement> {
        self.space.elements()
    }

    pub fn minimized_windows(&self) -> impl Iterator<Item = &WindowElement> {
        self.minimized_windows.keys()
    }

    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<WindowElement> {
        self.space
            .elements()
            .find(|window| window.wl_surface().map(|s| &*s == surface).unwrap_or(false))
            .cloned()
    }

    pub(super) fn set_window_fullscreen(&mut self, window: &WindowElement, output: &Output) -> Option<WindowElement> {
        self.fullscreen_windows.insert(output.clone(), window.clone())
    }

    pub(super) fn set_window_unfullscreen(&mut self, window: &WindowElement) -> Option<Output> {
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

    pub(super) fn set_window_minimized(&mut self, window: &WindowElement) -> bool {
        if let Some(location) = self.space.element_location(window) {
            self.space.unmap_elem(window);
            self.minimized_windows.insert(window.clone(), MinimizedWindow { location });
            window.update_minimized_state(true);
            true
        } else {
            false
        }
    }

    pub(super) fn set_window_unminimized(&mut self, window: &WindowElement, activate: bool) -> bool {
        if let Some(data) = self.minimized_windows.remove(window) {
            self.space.map_element(window.clone(), data.location, activate);
            window.update_minimized_state(false);
            true
        } else {
            false
        }
    }

    pub(super) fn add_minimized_window<P: Into<Point<i32, Logical>>>(&mut self, window: WindowElement, location: P) {
        self.minimized_windows.insert(window, MinimizedWindow { location: location.into() });
    }

    pub(super) fn remove_minimized_window(&mut self, window: &WindowElement) {
        self.minimized_windows.remove(window);
    }

    pub(super) fn minimized_window_location(&self, window: &WindowElement) -> Option<Point<i32, Logical>> {
        self.minimized_windows.get(window).map(|mw| mw.location)
    }

    pub fn render_elements_for_region<R, S>(
        &self,
        renderer: &mut R,
        region: &Rectangle<i32, Logical>,
        scale: S,
        alpha: f32,
    ) -> Vec<<WindowElement as AsRenderElements<R>>::RenderElement>
    where
        R: Renderer + ImportAll + ImportMem + AsGlesRenderer,
        R::TextureId: Texture + Clone + 'static,
        <R as RendererSuper>::Error: FromGlesError,
        S: Into<Scale<f64>>,
    {
        self.space.render_elements_for_region(renderer, region, scale, alpha)
    }

    pub fn refresh(&mut self) {
        self.space.refresh();
    }
}
