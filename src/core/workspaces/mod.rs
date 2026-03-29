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

use std::collections::VecDeque;

use smithay::{
    desktop::{WindowSurface, layer_map_for_output, space::SpaceElement},
    input::Seat,
    output::Output,
    reexports::{wayland_protocols::xdg::shell::server::xdg_toplevel, wayland_server::Resource},
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Serial},
};

use crate::{
    backend::Backend,
    core::{
        config::ActivateAction,
        focus::KeyboardFocusTarget,
        shell::{WindowElement, WindowFlags, WindowState, WorkspaceLocation, xdg::XdgSurfaceProps},
        state::Xfwl4State,
    },
};

mod manager;
mod workspace;

pub use manager::{WindowStackingLayer, WorkspaceManager};
pub use workspace::Workspace;

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub(in crate::core) fn set_active_workspace(&mut self, workspace_number: u32) {
        // Annoying, need to do this first before 'self' gets borrowed mutably below.
        let window_under_pointer = self
            .surface_under_for_workspace(self.core.pointer.current_location(), workspace_number)
            .and_then(|(target, _)| self.window_for_pointer_focus_target(&target));

        let changed = if let Some((prev_workspace, new_workspace)) = self.core.workspace_manager.set_active_workspace(workspace_number) {
            let new_active_window = if self.core.config.click_to_focus() {
                new_workspace.active_window().cloned()
            } else {
                window_under_pointer.clone()
            }
            .or_else(|| new_workspace.visible_windows().last().cloned());

            if let Some(prev_workspace) = prev_workspace
                && let Some(prev_active_window) = prev_workspace.active_window().cloned()
            {
                self.core.toplevel_changed(
                    &prev_active_window,
                    None,
                    None,
                    WindowState::empty(),
                    WindowState::ACTIVATED,
                    Vec::new(),
                    Vec::new(),
                    None,
                );
            }

            if let Some(active_window) = new_active_window {
                self.activate_window(&active_window, true, true, None);
            }

            self.core.pointer_window = window_under_pointer;
            true
        } else {
            false
        };

        if changed {
            self.core.cancel_focus_follows_mouse_timers();
        }
    }

    pub(in crate::core) fn toggle_active_workspace(&mut self, workspace_number: u32) {
        if workspace_number == self.core.workspace_manager.active_workspace_index() && self.core.config.toggle_workspaces() {
            let prev_ws_num = self.core.workspace_manager.previous_active_workspace_index();
            self.set_active_workspace(prev_ws_num);
        } else {
            self.set_active_workspace(workspace_number);
        }
    }

    pub(in crate::core) fn scrolled_for_workspace_switch(&mut self, amount: f64) {
        let steps = self.core.workspace_manager.scrolled_for_switch(amount);
        if steps != 0 {
            let index = self.core.workspace_manager.active_workspace_index();
            let nworkspaces = self.core.workspace_manager.workspaces().len() as i32;
            let new_index = ((index as i32) + steps).rem_euclid(nworkspaces);
            self.set_active_workspace(new_index as u32);
        }
    }

    pub(in crate::core) fn remove_workspace(&mut self, index: u32) {
        if let Some(new_ws_num) = self.core.workspace_manager.remove_workspace(index) {
            self.set_active_workspace(new_ws_num);
        }
    }

    pub(in crate::core) fn new_window<P: Into<Point<i32, Logical>>>(
        &mut self,
        window: WindowElement,
        location: P,
        allow_activate: bool,
        workspace_number: Option<u32>,
    ) {
        if !window.props().flags.contains(WindowFlags::NO_CYCLE) {
            self.core.cycle_list.add_new(window.clone());
        }

        let give_focus = allow_activate
            && self.core.config.focus_new()
            && workspace_number.is_none_or(|num| num == self.core.workspace_manager.active_workspace_index())
            && !self.core.cycling_windows;
        let parent = window.parent();

        self.core
            .workspace_manager
            .new_window(window.clone(), location, give_focus, workspace_number, parent.as_ref());

        if give_focus {
            self.focus_window(&window, SERIAL_COUNTER.next_serial(), None);
        }

        if self.core.cycling_windows {
            self.add_window_to_tabwin(&window);
        }
    }

    pub(in crate::core) fn focus_window(&mut self, window: &WindowElement, serial: Serial, seat: Option<Seat<Self>>) {
        self.core.cycle_list.focused(window);
        self.focus_target(window.clone(), serial, seat);
    }

    pub(in crate::core) fn focus_target<F: Into<KeyboardFocusTarget>>(&mut self, focus: F, serial: Serial, seat: Option<Seat<Self>>) {
        if let Some(keyboard) = seat.as_ref().unwrap_or(&self.core.seat).get_keyboard() {
            let focus = focus.into();

            if let KeyboardFocusTarget::Window(window) = &focus
                && let Some(window) = self.core.workspace_manager.active_workspace().find_window(|elem| elem.0 == *window)
                && let Some(urgent_state) = window.props().urgent.take()
            {
                self.core.handle.remove(urgent_state.token);

                if let Some(decorations) = window.decoration_state().window_decorations_mut() {
                    decorations.disable_titlebar_blink();
                }
            }

            keyboard.set_focus(self, Some(focus), serial);
        }
    }

    pub(in crate::core) fn unset_focus(&mut self, serial: Serial, seat: Option<Seat<Self>>) {
        if let Some(keyboard) = seat.as_ref().unwrap_or(&self.core.seat).get_keyboard() {
            keyboard.set_focus(self, None, serial);
        }
    }

    pub(in crate::core) fn remove_window(&mut self, window: &WindowElement) {
        self.core.cycle_list.remove(window);
        self.core.workspace_manager.remove_window(window);
        self.core.compositor_ui_state.tabwin_remove_window(window.window_id());

        if !self.core.cycling_windows
            && let Some(window) = { self.core.workspace_manager.active_workspace().visible_windows().last().cloned() }
        {
            self.activate_window(&window, true, false, None);
        }
    }

    pub(in crate::core) fn activate_window(&mut self, window: &WindowElement, raise: bool, force: bool, seat: Option<Seat<Self>>) {
        let active_workspace_index = self.core.workspace_manager.active_workspace_index();
        let cur_workspace_index = match window.props().workspace_loc {
            WorkspaceLocation::Single(num) => num,
            WorkspaceLocation::All => active_workspace_index,
        };

        if force || active_workspace_index == cur_workspace_index || self.core.config.activate_action() != ActivateAction::None {
            let cur_workspace_index = if active_workspace_index != cur_workspace_index {
                match self.core.config.activate_action() {
                    ActivateAction::Bring => {
                        self.core
                            .workspace_manager
                            .move_window_by_index(window, cur_workspace_index, active_workspace_index);
                        active_workspace_index
                    }
                    ActivateAction::Switch => {
                        self.set_active_workspace(cur_workspace_index);
                        cur_workspace_index
                    }
                    ActivateAction::None if force => {
                        self.set_active_workspace(cur_workspace_index);
                        cur_workspace_index
                    }
                    ActivateAction::None => cur_workspace_index,
                }
            } else {
                cur_workspace_index
            };

            if raise {
                self.raise_window(window, SERIAL_COUNTER.next_serial(), true);
            } else if let Some(workspace) = self.core.workspace_manager.workspaces_mut().get_mut(cur_workspace_index as usize) {
                workspace.set_active_window(Some(window));
            }

            if let Some(workspace) = self.core.workspace_manager.workspaces_mut().get_mut(cur_workspace_index as usize)
                && workspace.active()
            {
                let old_active_window = workspace.visible_windows().find(|window| window.active()).cloned();
                if old_active_window.as_ref().is_none_or(|old| old != window) {
                    let serial = SERIAL_COUNTER.next_serial();
                    self.focus_window(window, serial, seat);

                    if let Some(old_active_window) = &old_active_window {
                        self.core.toplevel_changed(
                            old_active_window,
                            None,
                            None,
                            WindowState::empty(),
                            WindowState::ACTIVATED,
                            Vec::new(),
                            Vec::new(),
                            None,
                        );
                    }

                    if old_active_window.as_ref() != Some(window) {
                        self.core.toplevel_changed(
                            window,
                            None,
                            None,
                            WindowState::ACTIVATED,
                            WindowState::empty(),
                            Vec::new(),
                            Vec::new(),
                            None,
                        );
                    }
                }
            }
        }
    }

    fn update_minimized_state(&self, window: &WindowElement, is_minimized: bool) {
        match window.0.underlying_surface() {
            WindowSurface::Wayland(surface) => {
                surface.with_pending_state(|state| {
                    if is_minimized {
                        state.states.set(xdg_toplevel::State::Suspended);
                    } else {
                        state.states.unset(xdg_toplevel::State::Suspended);
                    }
                });

                window
                    .0
                    .user_data()
                    .get_or_insert(XdgSurfaceProps::default)
                    .0
                    .lock()
                    .unwrap()
                    .is_minimized = is_minimized;
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x11_surface) => {
                if x11_surface.is_hidden() != is_minimized {
                    let _ = x11_surface.set_hidden(is_minimized);
                }
            }
        }
    }

    fn set_window_minimized_internal(&mut self, window: &WindowElement) {
        if self.core.workspace_manager.set_window_minimized(window) {
            if !self.core.config.cycle_minimized() {
                self.core.cycle_list.move_to_back(window);
            }
            self.update_minimized_state(window, true);
            window.set_activate(false);

            self.core.toplevel_changed(
                window,
                None,
                None,
                WindowState::MINIMIZED,
                WindowState::empty(),
                Vec::new(),
                Vec::new(),
                None,
            );
        }
    }

    pub(in crate::core) fn set_window_minimized(&mut self, window: &WindowElement) {
        // Here we do a breadth-first traversal, but upside down.
        let mut windows = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back(window.clone());

        while let Some(window) = queue.pop_front() {
            for child in window.children() {
                queue.push_back(child);
            }
            windows.push(window);
        }

        let was_active = windows.into_iter().rev().fold(false, |was_active_accum, window| {
            self.set_window_minimized_internal(&window);
            was_active_accum | window.active()
        });

        if was_active && let Some(window) = { self.core.workspace_manager.active_workspace().visible_windows().last().cloned() } {
            self.activate_window(&window, true, true, None);
        }
    }

    fn set_window_unminimized_internal(&mut self, window: &WindowElement, serial: Serial, activate: bool) {
        if self.core.workspace_manager.set_window_unminimized(window, activate) {
            self.set_window_shaded(window, false);
            self.update_minimized_state(window, false);

            if activate {
                self.focus_window(window, serial, None);
            }

            self.core.toplevel_changed(
                window,
                None,
                None,
                WindowState::empty(),
                WindowState::MINIMIZED,
                Vec::new(),
                Vec::new(),
                None,
            );
        }
    }

    pub(in crate::core) fn set_window_unminimized(&mut self, window: &WindowElement, serial: Serial, activate: bool) {
        let mut windows = vec![window.clone()];
        while let Some(parent) = window.parent() {
            windows.push(parent);
        }

        for w in windows.into_iter().rev() {
            self.set_window_unminimized_internal(&w, serial, activate && &w == window);
        }
    }

    pub(in crate::core) fn set_window_maximized(&mut self, window: &WindowElement) {
        let outputs_for_window = self.core.workspace_manager.outputs_for_window(window);
        if let Some((output, output_geom)) = outputs_for_window
            .first()
            .or_else(|| {
                // The window hasn't been mapped yet, use the primary output instead
                self.core.workspace_manager.outputs().next()
            })
            .and_then(|output| self.core.workspace_manager.output_geometry(output).map(|geom| (output, geom)))
        {
            let old_geom = self.core.workspace_manager.window_geometry(window);
            let mut props = window.props();
            props.pre_maximize_geom = old_geom;
            props.maximized_output = Some(output.downgrade());
            drop(props);

            let mut geometry = {
                let layer_map = layer_map_for_output(output);
                let zone = layer_map.non_exclusive_zone();
                Rectangle::new(output_geom.loc + zone.loc, zone.size)
            };

            if let Some(window_decorations) = window.decoration_state().window_decorations_mut() {
                window_decorations.update_maximized_state(true);
                geometry.size.w -= window_decorations.left_decoration_width() + window_decorations.right_decoration_width();
                geometry.size.h -= window_decorations.top_decoration_height() + window_decorations.bottom_decoration_height();
            }

            match window.0.underlying_surface() {
                WindowSurface::Wayland(surface) => {
                    surface.with_pending_state(|state| {
                        state.states.set(xdg_toplevel::State::Maximized);
                        state.size = Some(geometry.size);
                    });
                    self.core.workspace_manager.relocate_window(window, geometry.loc, false);

                    // The protocol demands us to always reply with a configure,
                    // regardless of we fulfilled the request or not
                    if surface.is_initial_configure_sent() {
                        surface.send_configure();
                    }
                }

                #[cfg(feature = "xwayland")]
                WindowSurface::X11(surface) => {
                    let _ = surface.set_maximized(true);
                    let _ = surface.configure(geometry);
                    self.core.workspace_manager.relocate_window(window, geometry.loc, false);
                }
            }

            self.core.toplevel_changed(
                window,
                None,
                None,
                WindowState::MAXIMIZED,
                WindowState::empty(),
                Vec::new(),
                Vec::new(),
                None,
            );
        }
    }

    pub(in crate::core) fn set_window_unmaximized(&mut self, window: &WindowElement, new_location: Option<Point<i32, Logical>>) {
        if let Some(window_decorations) = window.decoration_state().window_decorations_mut() {
            window_decorations.update_maximized_state(false);
        }

        let mut props = window.props();
        let old_geom = props.pre_maximize_geom.take();
        props.maximized_output = None;
        drop(props);

        let new_location = new_location.or_else(|| old_geom.map(|geom| geom.loc));

        match window.0.underlying_surface() {
            WindowSurface::Wayland(surface) => {
                surface.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Maximized);
                    state.size = None;
                });

                // The protocol demands us to always reply with a configure,
                // regardless of we fulfilled the request or not
                if surface.is_initial_configure_sent() {
                    surface.send_configure();
                }
            }

            #[cfg(feature = "xwayland")]
            WindowSurface::X11(surface) => {
                let _ = surface.set_maximized(false);
                if let Some(old_geom) = old_geom {
                    let _ = surface.configure(old_geom);
                }
            }
        }

        if let Some(new_location) = new_location {
            self.core.workspace_manager.relocate_window(window, new_location, false);
        }

        self.core.toplevel_changed(
            window,
            None,
            None,
            WindowState::empty(),
            WindowState::MAXIMIZED,
            Vec::new(),
            Vec::new(),
            None,
        );
    }

    pub(in crate::core) fn set_window_filled(&mut self, window: &WindowElement) {
        if window.maximized() {
            self.set_window_unmaximized(window, None);
        }

        let outputs_for_window = self.core.workspace_manager.outputs_for_window(window);
        if let Some(output) = outputs_for_window.first().or_else(|| {
            // The window hasn't been mapped yet, use the primary output instead
            self.core.workspace_manager.outputs().next()
        }) {
            let layer_map = layer_map_for_output(output);
            let mut geometry = layer_map.non_exclusive_zone();
            drop(layer_map);

            if let Some(window_decorations) = window.decoration_state().window_decorations_mut() {
                geometry.size.w -= window_decorations.left_decoration_width() + window_decorations.right_decoration_width();
                geometry.size.h -= window_decorations.top_decoration_height() + window_decorations.bottom_decoration_height();
            }

            match window.0.underlying_surface() {
                WindowSurface::Wayland(surface) => {
                    surface.with_pending_state(|state| {
                        state.size = Some(geometry.size);
                    });
                    self.core.workspace_manager.relocate_window(window, geometry.loc, false);

                    if surface.is_initial_configure_sent() {
                        surface.send_configure();
                    }
                }

                #[cfg(feature = "xwayland")]
                WindowSurface::X11(surface) => {
                    let _ = surface.configure(geometry);
                    self.core.workspace_manager.relocate_window(window, geometry.loc, false);
                }
            }
        }
    }

    pub(in crate::core) fn set_window_shaded(&self, window: &WindowElement, is_shaded: bool) {
        let mut props = window.props();
        let changed = if props.is_shaded != is_shaded {
            props.is_shaded = is_shaded;
            if let Some(decorations) = window.decoration_state().window_decorations_mut() {
                decorations.update_is_shaded_state(is_shaded);
            }

            true
        } else {
            false
        };
        drop(props);

        if changed {
            #[cfg(feature = "xwayland")]
            if let WindowSurface::X11(x11_surface) = window.0.underlying_surface()
                && let Some(xw) = &self.core.xwayland
            {
                let (add, remove) = if is_shaded {
                    (vec!["_NET_WM_STATE_SHADED"], vec![])
                } else {
                    (vec![], vec!["_NET_WM_STATE_SHADED"])
                };
                xw.x11.update_net_wm_state(x11_surface.window_id(), &add, &remove);
            }
        }
    }

    fn set_window_sticky_internal(&mut self, window: &WindowElement, is_sticky: bool) {
        let cur_is_sticky = window.props().workspace_loc == WorkspaceLocation::All;
        if cur_is_sticky != is_sticky {
            let new_ws_loc = if is_sticky {
                WorkspaceLocation::All
            } else {
                WorkspaceLocation::Single(self.core.workspace_manager.active_workspace_index())
            };

            self.core.workspace_manager.set_window_workspace_num(window, new_ws_loc);

            if let Some(window_decorations) = window.decoration_state().window_decorations_mut() {
                window_decorations.update_is_sticky_state(is_sticky);
            }

            let (added, removed) = if is_sticky {
                (WindowState::STICKY, WindowState::empty())
            } else {
                (WindowState::empty(), WindowState::STICKY)
            };

            self.core
                .toplevel_changed(window, None, None, added, removed, Vec::new(), Vec::new(), None);
        }
    }

    pub(in crate::core) fn set_window_sticky(&mut self, window: &WindowElement, is_sticky: bool) {
        let mut root = window.clone();
        while let Some(parent) = root.parent() {
            root = parent;
        }

        // Do a breadth-first traversal, (un)sticking each window as we go down the tree.
        let mut queue = VecDeque::new();
        queue.push_back(root);
        while let Some(child) = queue.pop_front() {
            self.set_window_sticky_internal(&child, is_sticky);
            for child in child.children() {
                queue.push_back(child);
            }
        }
    }

    pub(in crate::core) fn set_window_always_on_top(&mut self, window: &WindowElement) {
        self.core
            .workspace_manager
            .set_window_stacking_layer(window, WindowStackingLayer::AlwaysOnTop);
    }

    pub(in crate::core) fn set_window_always_on_bottom(&mut self, window: &WindowElement) {
        self.core
            .workspace_manager
            .set_window_stacking_layer(window, WindowStackingLayer::AlwaysOnBottom);
    }

    pub(in crate::core) fn set_window_normal_stacking(&mut self, window: &WindowElement) {
        self.core
            .workspace_manager
            .set_window_stacking_layer(window, WindowStackingLayer::Normal);
    }

    pub(in crate::core) fn set_window_fullscreen(&mut self, window: &WindowElement, output: Option<Output>) {
        let workspace = self.core.workspace_manager.active_workspace_mut();
        let output_and_geometry = output
            .or_else(|| workspace.outputs_for_window(window).into_iter().next())
            .or_else(|| self.core.workspace_manager.outputs().next().cloned())
            .and_then(|output| self.core.workspace_manager.output_geometry(&output).map(|geom| (output, geom)));

        if let Some((output, geometry)) = output_and_geometry {
            // NOTE: This is only one part of the solution. We can set the
            // location and configure size here, but the surface should be rendered fullscreen
            // independently from its buffer size

            let (fullscreened, old_fullscreen_windows) = match window.0.underlying_surface() {
                WindowSurface::Wayland(surface) => {
                    let (fullscreened, old_fullscreen_window) =
                        if let Ok(client) = self.core.display_handle.get_client(surface.wl_surface().id()) {
                            let wl_output = output.client_outputs(&client).last();

                            window.disable_decorations();
                            surface.with_pending_state(|state| {
                                state.states.set(xdg_toplevel::State::Fullscreen);
                                state.size = Some(geometry.size);
                                state.fullscreen_output = wl_output;
                            });
                            tracing::trace!("Fullscreening: {:?}", window);
                            (true, self.core.workspace_manager.set_window_fullscreen(window, &output))
                        } else {
                            (false, vec![])
                        };

                    // The protocol demands us to always reply with a configure,
                    // regardless of we fulfilled the request or not
                    if surface.is_initial_configure_sent() {
                        surface.send_configure();
                    }

                    (fullscreened, old_fullscreen_window)
                }

                #[cfg(feature = "xwayland")]
                WindowSurface::X11(surface) => {
                    window.disable_decorations();
                    let _ = surface.set_fullscreen(true);
                    let _ = surface.configure(geometry);
                    tracing::trace!("Fullscreening: {:?}", window);
                    (true, self.core.workspace_manager.set_window_fullscreen(window, &output))
                }
            };

            self.backend.reset_buffers(&output);

            for old_fullscreen_window in old_fullscreen_windows {
                self.set_window_unfullscreen(&old_fullscreen_window);
            }

            if fullscreened {
                self.core.toplevel_changed(
                    window,
                    None,
                    None,
                    WindowState::FULLSCREEN,
                    WindowState::empty(),
                    Vec::new(),
                    Vec::new(),
                    None,
                );
            }
        }
    }

    pub(in crate::core) fn set_window_unfullscreen(&mut self, window: &WindowElement) {
        match window.0.underlying_surface() {
            WindowSurface::Wayland(surface) => {
                surface.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Fullscreen);
                    state.size = None;
                    state.fullscreen_output = None;
                });

                // The protocol demands us to always reply with a configure,
                // regardless of we fulfilled the request or not
                if surface.is_initial_configure_sent() {
                    surface.send_configure();
                }
            }

            #[cfg(feature = "xwayland")]
            WindowSurface::X11(surface) => {
                let _ = surface.set_fullscreen(false);
                if let Some(workspace) = self.core.workspace_manager.workspace_for_window_mut(window) {
                    let _ = surface.configure(workspace.window_bbox(window));
                }
                if !surface.is_decorated() {
                    self.enable_decorations_for_window(window);
                } else {
                    window.disable_decorations();
                }
            }
        }

        for output in self.core.workspace_manager.set_window_unfullscreen(window) {
            self.backend.reset_buffers(&output);
        }

        self.core.toplevel_changed(
            window,
            None,
            None,
            WindowState::empty(),
            WindowState::FULLSCREEN,
            Vec::new(),
            Vec::new(),
            None,
        );
    }

    fn raise_window_internal(&mut self, window: &WindowElement, serial: Serial, activate: bool) {
        // FIXME: actually should probably just match the root's stacking.
        if !window.always_on_top() || !window.normal_stacking() {
            self.set_window_normal_stacking(window);
        }

        let active_ws_num = self.core.workspace_manager.active_workspace_index();
        let workspace_and_index = if !window.sticky() {
            self.core.workspace_manager.workspace_for_window_with_index_mut(window)
        } else {
            Some((active_ws_num, self.core.workspace_manager.active_workspace_mut()))
        };

        if let Some((ws_num, workspace)) = workspace_and_index {
            workspace.raise_window(window, activate);
            if ws_num == active_ws_num && activate {
                self.focus_window(window, serial, None);
            }
        }
    }

    pub(in crate::core) fn raise_window(&mut self, window: &WindowElement, serial: Serial, activate: bool) {
        let mut root = window.clone();
        while let Some(parent) = root.parent() {
            root = parent;
        }

        // Do a breadth-first traversal, raising each window as we go down the tree.
        let mut queue = VecDeque::new();
        queue.push_back(root);
        while let Some(child) = queue.pop_front() {
            self.raise_window_internal(&child, serial, activate && &child == window);
            for child in child.children() {
                queue.push_back(child);
            }
        }
    }

    fn lower_window_internal(&mut self, window: &WindowElement, below: Option<&WindowElement>) {
        if let Some(workspace) = if !window.sticky() {
            self.core.workspace_manager.workspace_for_window_mut(window)
        } else {
            Some(self.core.workspace_manager.active_workspace_mut())
        } {
            if let Some(below) = below {
                workspace.lower_window_below(window, below);
            } else {
                workspace.lower_window(window);
            }
        }
    }

    pub(in crate::core) fn lower_window(&mut self, window: &WindowElement, serial: Serial, below: Option<WindowElement>) {
        let mut root = window.clone();
        while let Some(parent) = root.parent() {
            root = parent;
        }

        // Do a breadth-first traversal, lowering each window as we go down the tree.
        let mut queue = VecDeque::new();
        let mut was_active = false;
        queue.push_back(root);
        while let Some(child) = queue.pop_front() {
            was_active |= child.active();
            self.lower_window_internal(&child, below.as_ref());
            for child in child.children() {
                queue.push_back(child);
            }
        }

        let active_ws_num = self.core.workspace_manager.active_workspace_index();
        let workspace_and_index = if !window.sticky() {
            self.core.workspace_manager.workspace_for_window_with_index_mut(window)
        } else {
            Some((active_ws_num, self.core.workspace_manager.active_workspace_mut()))
        };

        if let Some((ws_num, workspace)) = workspace_and_index
            && was_active
        {
            // Next activate and give focus to the now-top window in the stack.
            if let Some(new_focus) = workspace.visible_windows().last().cloned() {
                workspace.raise_window(&new_focus, true);
                if ws_num == active_ws_num {
                    self.focus_window(&new_focus, serial, None);
                }
            }
        }
    }

    pub(in crate::core) fn set_window_parent(&mut self, window: &WindowElement, parent: Option<WindowElement>) {
        let old_parent = window.parent();
        if window.set_parent(parent.clone()) {
            if let Some(old_parent) = old_parent {
                old_parent.remove_child(window);
            }

            if let Some(parent) = &parent {
                parent.add_child(window.clone());
            }
        }

        if let Some(parent) = parent {
            let workspace_loc = parent.props().workspace_loc;
            match workspace_loc {
                WorkspaceLocation::Single(num) => {
                    self.set_window_sticky(window, false);

                    if let Some(workspace) = self.core.workspace_manager.workspaces_mut().get_mut(num as usize)
                        && workspace.window_location(window).is_some()
                    {
                        let activate = workspace.active_window().is_some_and(|active| active == &parent);
                        workspace.raise_window_above(window, &parent, activate);
                    }
                }

                WorkspaceLocation::All => {
                    self.set_window_sticky(window, true);

                    for workspace in self.core.workspace_manager.workspaces_mut() {
                        if workspace.window_location(window).is_some() {
                            let activate = workspace.active_window().is_some_and(|active| active == &parent);
                            workspace.raise_window_above(window, &parent, activate);
                        }
                    }
                }
            }
        }
    }
}
