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

use smithay::{
    desktop::{WindowSurface, layer_map_for_output, space::RenderZindex},
    input::Seat,
    output::Output,
    reexports::{wayland_protocols::xdg::shell::server::xdg_toplevel, wayland_server::Resource},
    utils::{Rectangle, SERIAL_COUNTER, Serial},
};

use crate::{
    backend::Backend,
    core::{
        focus::KeyboardFocusTarget,
        shell::{WindowElement, WindowProps, WindowState},
        state::Xfwl4State,
    },
};

mod manager;
mod workspace;

pub use manager::{WindowStackingLayer, WorkspaceManager};
pub use workspace::Workspace;

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub(in crate::core) fn activate_window(&mut self, window: &WindowElement, seat: Option<Seat<Self>>) {
        if let Some(workspace) = if !window.sticky() {
            self.core.workspace_manager.workspace_for_window_mut(window)
        } else {
            Some(self.core.workspace_manager.active_workspace_mut())
        } {
            let old_active_window = workspace.visible_windows().find(|window| window.active()).cloned();

            workspace.raise_window(window, true);

            if workspace.active() {
                let seat = seat.as_ref().unwrap_or(&self.core.seat);
                if let Some(keyboard) = seat.get_keyboard() {
                    let focus = KeyboardFocusTarget::Window(window.0.clone());
                    keyboard.set_focus(self, Some(focus), SERIAL_COUNTER.next_serial());
                }

                if let Some(old_active_window) = &old_active_window
                    && old_active_window != window
                {
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

    pub(in crate::core) fn set_window_minimized(&mut self, window: &WindowElement) {
        if self.core.workspace_manager.set_window_minimized(window) {
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

    pub(in crate::core) fn set_window_unminimized(&mut self, window: &WindowElement, activate: bool) {
        if self.core.workspace_manager.set_window_unminimized(window, activate) {
            self.set_window_shaded(window, false);
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

    pub(in crate::core) fn set_window_maximized(&mut self, window: &WindowElement, is_maximized: bool) {
        if is_maximized {
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
                let mut inner = window.0.user_data().get_or_insert(WindowProps::default).0.lock().unwrap();
                inner.pre_maximize_geom = old_geom;

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
        } else {
            if let Some(window_decorations) = window.decoration_state().window_decorations_mut() {
                window_decorations.update_maximized_state(false);
            }

            match window.0.underlying_surface() {
                WindowSurface::Wayland(surface) => {
                    surface.with_pending_state(|state| {
                        state.states.unset(xdg_toplevel::State::Maximized);
                        state.size = None;
                    });

                    let mut props = window.0.user_data().get_or_insert(WindowProps::default).0.lock().unwrap();
                    let old_loc = props.pre_maximize_geom.take().map(|geom| geom.loc).unwrap_or_default();
                    self.core.workspace_manager.relocate_window(window, old_loc, false);

                    // The protocol demands us to always reply with a configure,
                    // regardless of we fulfilled the request or not
                    if surface.is_initial_configure_sent() {
                        surface.send_configure();
                    }
                }

                #[cfg(feature = "xwayland")]
                WindowSurface::X11(surface) => {
                    let mut props = window.0.user_data().get_or_insert(WindowProps::default).0.lock().unwrap();
                    if let Some(old_geom) = props.pre_maximize_geom.take() {
                        drop(props);
                        let _ = surface.set_maximized(false);
                        let _ = surface.configure(old_geom);
                        self.core.workspace_manager.relocate_window(window, old_geom.loc, false);
                    }
                }
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
    }

    pub(in crate::core) fn set_window_filled(&mut self, window: &WindowElement) {
        if window.maximized() {
            self.set_window_maximized(window, false);
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
        let mut inner = window.0.user_data().get_or_insert(WindowProps::default).0.lock().unwrap();
        let changed = if inner.is_shaded != is_shaded {
            inner.is_shaded = is_shaded;
            true
        } else {
            false
        };

        if changed {
            #[cfg(feature = "xwayland")]
            if let WindowSurface::X11(x11_surface) = window.0.underlying_surface()
                && let Some((x11_conn, _)) = &self.core.x11conn
            {
                use crate::core::util::x11::{get_atom, update_net_wm_state};

                if let Some(net_wm_state_shaded) = get_atom(x11_conn, b"_NET_WM_STATE_SHADED") {
                    let (add, remove) = if is_shaded {
                        (vec![net_wm_state_shaded], vec![])
                    } else {
                        (vec![], vec![net_wm_state_shaded])
                    };
                    update_net_wm_state(x11_conn, x11_surface.window_id(), &add, &remove);
                }
            }
        }
    }

    pub(in crate::core) fn set_window_sticky(&mut self, window: &WindowElement, is_sticky: bool) {
        let mut inner = window.0.user_data().get_or_insert(WindowProps::default).0.lock().unwrap();
        if inner.is_sticky != is_sticky {
            inner.is_sticky = is_sticky;
            drop(inner);

            self.core.workspace_manager.set_window_sticky(window, is_sticky);

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

    pub(in crate::core) fn raise_window(&mut self, window: &WindowElement, serial: Serial) {
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
            workspace.raise_window(window, true);

            if ws_num == active_ws_num
                && let Some(keyboard) = self.core.seat.get_keyboard()
            {
                keyboard.set_focus(self, Some(window.clone().into()), serial);
            }
        }
    }

    pub(in crate::core) fn lower_window(&mut self, window: &WindowElement, serial: Serial) {
        let active_ws_num = self.core.workspace_manager.active_workspace_index();
        let workspace_and_index = if !window.sticky() {
            self.core.workspace_manager.workspace_for_window_with_index_mut(window)
        } else {
            Some((active_ws_num, self.core.workspace_manager.active_workspace_mut()))
        };

        if let Some((ws_num, workspace)) = workspace_and_index {
            let was_active = window.active();

            // This is annoying; smithay's Space doesn't give us direct access to order
            // windows, so we have to go through some acrobatics: override the z-index to the
            // bottom layer, "raise" the window (which removes it, re-maps it, and sorts by the
            // elements z-index), and then override the z-index back to the default.
            window.0.override_z_index(RenderZindex::Bottom as u8);
            workspace.raise_window(window, false);
            window.0.override_z_index(RenderZindex::Shell as u8);

            if ws_num == active_ws_num && was_active {
                // Next activate and give focus to the now-top window in the stack.
                if let Some(new_focus) = workspace.visible_windows().last().cloned() {
                    workspace.raise_window(&new_focus, true);
                    if let Some(keyboard) = self.core.seat.get_keyboard() {
                        keyboard.set_focus(self, Some(new_focus.into()), serial);
                    }
                }
            }
        }
    }
}
