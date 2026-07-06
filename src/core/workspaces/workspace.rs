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
        if self.is_active != is_active {
            self.is_active = is_active;
            self.reconcile_activation();
        }
    }

    pub fn active(&self) -> bool {
        self.is_active
    }

    pub(super) fn set_active_window(&mut self, window: Option<&WindowElement>) {
        self.active_window = window.cloned();
        self.reconcile_activation();
    }

    // A window is activated only while its workspace is active and it is the
    // designated active window; the per-window flags are otherwise a pure
    // projection of `active_window`, so every mutation funnels through here.
    fn reconcile_activation(&mut self) {
        let active = self.is_active.then(|| self.active_window.clone()).flatten();
        for w in self.space.elements() {
            w.set_activate(Some(w) == active.as_ref());
        }
    }

    pub fn active_window(&self) -> Option<&WindowElement> {
        self.active_window.as_ref()
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

    pub(super) fn map_window<P: Into<Point<i32, Logical>>>(
        &mut self,
        window: WindowElement,
        location: P,
        activate: bool,
        parent: Option<&WindowElement>,
    ) {
        if let Some(parent) = parent {
            self.space.map_element_above(window.clone(), location, parent, false);
        } else {
            self.space.map_element(window.clone(), location, false);
        }

        if activate {
            self.set_active_window(Some(&window));
        }
    }

    pub(super) fn raise_window(&mut self, window: &WindowElement, activate: bool) {
        if self.window_location(window).is_some() {
            self.space.raise_element(window, false);
            if activate {
                self.set_active_window(Some(window));
            }
        }
    }

    pub(super) fn raise_window_above(&mut self, window: &WindowElement, reference_window: &WindowElement, activate: bool) {
        if self.window_location(window).is_some() {
            self.space.raise_element_above(window, reference_window, false);
            if activate {
                self.set_active_window(Some(window));
            }
        }
    }

    pub(super) fn lower_window(&mut self, window: &WindowElement) {
        if self.window_location(window).is_some() {
            self.space.lower_element(window);
        }
    }

    pub fn lower_window_below(&mut self, window: &WindowElement, reference_window: &WindowElement) {
        if self.window_location(window).is_some() {
            // I only added Space::raise_element_above() to smithay, so we have to find the element
            // below `reference_window`, and raise it above it.  So:
            // - Windows are listed from bottom to top, so reverse it; now windows are listed top
            //   to bottom.
            // - Skip everything in the list until the current element is refrerence_window, and
            //   then keep the rest.
            // - Fetch & drop the first item in the iterator, which is `reference_window`.
            // - Fetch the next item in the list, which is our new reference for
            //   `raise_window_above()`.
            let mut iter_at_reference = self.visible_windows().rev().skip_while(|elem| *elem != reference_window);
            let reference = iter_at_reference.next();

            if reference.is_some() {
                let reference_above = iter_at_reference.next().cloned();
                drop(iter_at_reference);

                if let Some(reference_above) = reference_above {
                    self.space.raise_element_above(window, &reference_above, false);
                } else {
                    self.space.lower_element(window);
                }
            }
        }
    }

    pub(super) fn relocate_window<P: Into<Point<i32, Logical>>>(&mut self, window: &WindowElement, location: P, activate: bool) {
        if let Some(cur_location) = self.window_location(window) {
            let location = location.into();
            if activate {
                self.space.map_element(window.clone(), location, false);
                self.set_active_window(Some(window));
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
        let outputs = self.space.outputs_for_element(window);
        if !outputs.is_empty() {
            outputs
        } else {
            // Before the first commit, a window will have a 0x0 bbox, which will cause the ouputs
            // list to be empty.  Instead, fall back to the output under the window's location in
            // the workspace.
            self.space
                .element_location(window)
                .map(|location| self.space.output_under(location.to_f64()).cloned().collect())
                .unwrap_or_default()
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
            if self.active_window.as_ref().is_some_and(|active| active == window) {
                self.active_window = None;
            }
            true
        } else {
            false
        }
    }

    pub(super) fn set_window_unminimized(&mut self, window: &WindowElement, activate: bool) -> bool {
        if let Some(data) = self.minimized_windows.remove(window) {
            self.space.map_element(window.clone(), data.location, false);
            if activate {
                self.set_active_window(Some(window));
            }
            true
        } else {
            false
        }
    }

    pub(super) fn add_minimized_window<P: Into<Point<i32, Logical>>>(&mut self, window: WindowElement, location: P) {
        window.set_activate(false);
        self.minimized_windows.insert(window, MinimizedWindow { location: location.into() });
    }

    pub(super) fn remove_minimized_window(&mut self, window: &WindowElement) {
        self.minimized_windows.remove(window);
    }

    pub(super) fn minimized_window_location(&self, window: &WindowElement) -> Option<Point<i32, Logical>> {
        self.minimized_windows.get(window).map(|mw| mw.location)
    }

    pub fn minimized_window_geometry(&self, window: &WindowElement) -> Option<Rectangle<i32, Logical>> {
        self.minimized_windows
            .get(window)
            .map(|mw| Rectangle::new(mw.location, window.geometry().size))
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
