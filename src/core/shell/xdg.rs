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
// Portions of this file are based on "anvil", an example compositor
// based on the smithay crate, and are licensed under the MIT license
// with the following terms:
//
// Copyright (C) Victor Berger <victor.berger@m4x.org>
// Copyright (C) Drakulix (Victoria Brekenfeld)
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use std::{
    cell::RefCell,
    hash::{DefaultHasher, Hash, Hasher},
    rc::Rc,
};

use calloop::{
    RegistrationToken,
    timer::{TimeoutAction, Timer},
};
use gtk::gio::{self, traits::AppInfoExt};
use smithay::{
    desktop::{
        PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, Window, WindowSurfaceType, find_popup_root_surface,
        get_popup_toplevel_coords, layer_map_for_output,
        space::{RenderZindex, SpaceElement},
        utils::bbox_from_surface_tree,
    },
    input::{
        Seat,
        pointer::{CursorIcon, Focus, MotionEvent},
    },
    output::Output,
    reexports::{
        wayland_protocols::xdg::{decoration as xdg_decoration, shell::server::xdg_toplevel},
        wayland_server::{
            Client, Resource,
            protocol::{wl_output, wl_seat, wl_surface::WlSurface},
        },
    },
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Serial, Size},
    wayland::{
        compositor::{self, with_states},
        seat::WaylandFocus,
        shell::xdg::{
            Configure, PopupSurface, PositionerState, ShellClient, SurfaceCachedState, ToplevelCachedState, ToplevelSurface,
            XdgShellHandler, XdgShellState, XdgToplevelSurfaceData,
            dialog::{ToplevelDialogHint, XdgDialogHandler},
        },
        xdg_toplevel_icon::ToplevelIconCachedState,
    },
};
use tracing::warn;

use crate::{
    backend::Backend,
    core::{
        focus::KeyboardFocusTarget,
        handlers::xfwl4_compositor_ui::ActionLocation,
        placement::{FillMode, StackResult},
        shell::{GrabTrigger, WINDOW_PING_TIMEOUT, WindowFlags, ssd::DecorationInput},
        state::{Xfwl4Core, Xfwl4State},
        util::{prettify_name, shm_buffer_to_image_data},
        workspaces::WindowStackingLayer,
    },
    protocols::foreign_toplevel_management::{ToplevelChangedInput, xfce_foreign_toplevel_management::IconSize},
    ui::window_menu::WINDOW_MENU_TOPLEVEL_TITLE,
};

use super::{ResizeEdge, ResizeState, SurfaceData, WindowElement};

#[derive(Default)]
struct PingTimeoutToken(Rc<RefCell<Option<RegistrationToken>>>);

impl<BackendData: Backend> XdgShellHandler for Xfwl4State<BackendData> {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.core.shell_state.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Do not send a configure here, the initial configure
        // of a xdg_surface has to be sent during the commit if
        // the surface is not already configured

        // Set the initial toplevel bounds so the client knows what size to use
        let pointer_location = self.core.pointer.current_location();
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
                Some(self.output_window_area(&o, geo))
            })
            .unwrap_or_else(|| Rectangle::from_size((800, 800).into()));
        surface.with_pending_state(|state| {
            state.bounds = Some(output_geometry.size);
        });

        let window = WindowElement::new(
            Window::new_wayland_window(surface.clone()),
            self.core.next_window_id(),
            &self.core.config,
        );
        self.update_window_capabilities(&window);
        self.apply_pending_decoration_state(&window);

        self.core.shell_state.pending_windows.insert(surface.wl_surface().clone(), window);

        compositor::add_post_commit_hook(surface.wl_surface(), |state: &mut Self, _, surface| {
            state.handle_toplevel_commit(surface);
        });
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        // Do not send a configure here, the initial configure
        // of a xdg_surface has to be sent during the commit if
        // the surface is not already configured

        self.unconstrain_popup(&surface);

        if let Err(err) = self.core.shell_state.popup_manager.track_popup(PopupKind::from(surface)) {
            warn!("Failed to track popup: {}", err);
        }
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|state| {
            let geometry = positioner.get_geometry();
            state.geometry = geometry;
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn parent_changed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            let parent = compositor::with_states(surface.wl_surface(), |states| {
                states.data_map.get::<XdgToplevelSurfaceData>().and_then(|data| {
                    data.lock()
                        .unwrap()
                        .parent
                        .as_ref()
                        .and_then(|wl_surface| self.window_for_surface(wl_surface))
                })
            });
            self.set_window_parent(&window, parent);
        }
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        if let Some(window) = self.window_for_surface(surface.wl_surface())
            && let Some(seat) = Seat::from_resource(&seat)
        {
            self.start_window_move(window, seat, serial, GrabTrigger::Pointer);
        }
    }

    fn resize_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial, edges: xdg_toplevel::ResizeEdge) {
        if let Some(window) = self.window_for_surface(surface.wl_surface())
            && let Some(seat) = Seat::from_resource(&seat)
        {
            self.start_window_resize(window, seat, serial, edges.into(), GrabTrigger::Pointer);
        }
    }

    fn ack_configure(&mut self, surface: WlSurface, configure: Configure) {
        if let Configure::Toplevel(configure) = configure
            && let Some(window) = self.window_for_surface(&surface)
        {
            use xdg_decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
            let is_ssd = configure
                .state
                .decoration_mode
                .map(|mode| !configure.state.states.contains(xdg_toplevel::State::Fullscreen) && mode == Mode::ServerSide)
                .unwrap_or(false);
            if is_ssd && !window.decoration_state().has_decorations() {
                self.enable_decorations_for_window(&window);
            } else if !is_ssd && window.decoration_state().has_decorations() {
                self.disable_decorations_for_window(&window);
            }

            let update_window_icon = with_states(&surface, |states| {
                let mut icon_state = states.cached_state.get::<ToplevelIconCachedState>();
                let current = icon_state.current();

                let new_icon_state_hash = {
                    let mut hasher = DefaultHasher::new();
                    current.icon_name().hash(&mut hasher);
                    for (buffer, scale) in current.buffers() {
                        buffer.hash(&mut hasher);
                        scale.hash(&mut hasher);
                    }
                    hasher.finish()
                };

                let mut props = window.props();
                if new_icon_state_hash != props.toplevel_icon_state_hash {
                    let rasters = current
                        .buffers()
                        .iter()
                        .flat_map(|(buffer, scale)| shm_buffer_to_image_data(buffer, (*scale).max(1) as u32).ok())
                        .collect::<Vec<_>>();

                    props.toplevel_icon_state_hash = new_icon_state_hash;
                    props.window_icon.update_name(current.icon_name().map(ToOwned::to_owned));
                    props.window_icon.update_rasters(rasters);
                    true
                } else {
                    false
                }
            });
            if update_window_icon && let Some(window_decorations) = window.decoration_state_mut().window_decorations_mut() {
                let depends_on_theme = window.props().window_icon.depends_on_theme();
                window_decorations.update(DecorationInput::IconChanged { depends_on_theme });

                let state = window.state();
                let props = window.props();
                let icon_name = props.window_icon.window_icon_name();
                let icon_rasters = props.window_icon.window_icon_rasters();
                self.core.toplevel_changed(
                    &window,
                    ToplevelChangedInput {
                        state: Some(state),
                        icon_name: Some(icon_name.map(ToOwned::to_owned)),
                        icon_sizes: Some(
                            icon_rasters
                                .iter()
                                .map(|raster| IconSize::new(raster.size.w, raster.size.h, raster.scale))
                                .collect(),
                        ),
                        ..Default::default()
                    },
                );
            }
        }
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, wl_output: Option<wl_output::WlOutput>) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.set_window_fullscreen(&window, wl_output.as_ref().and_then(Output::from_resource));
        } else {
            send_unfulfilled_configure(&surface);
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.set_window_unfullscreen(&window);
        } else {
            send_unfulfilled_configure(&surface);
        }
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.set_window_maximized(&window, FillMode::Both, None);
        } else {
            send_unfulfilled_configure(&surface);
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.set_window_unmaximized(&window, None);
        } else {
            send_unfulfilled_configure(&surface);
        }
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let kind = PopupKind::Xdg(surface);
        if let Some(seat) = Seat::from_resource(&seat)
            && let Some(root) = find_popup_root_surface(&kind).ok().and_then(|root| {
                if let Some(window_menu_anchor) = self.core.window_menu_state.window_menu_anchor()
                    && window_menu_anchor.wl_surface().is_some_and(|surf| surf.as_ref() == &root)
                {
                    Some(KeyboardFocusTarget::from(window_menu_anchor.clone()))
                } else {
                    let workspace = self.core.workspace_manager.active_workspace();

                    workspace.window_for_surface(&root).map(KeyboardFocusTarget::from).or_else(|| {
                        self.core
                            .workspace_manager
                            .outputs()
                            .find_map(|o| {
                                let map = layer_map_for_output(o);
                                map.layer_for_surface(&root, WindowSurfaceType::TOPLEVEL).cloned()
                            })
                            .map(KeyboardFocusTarget::LayerSurface)
                    })
                }
            })
        {
            let ret = self.core.shell_state.popup_manager.grab_popup(root, kind, &seat, serial);

            if let Ok(mut grab) = ret {
                if let Some(keyboard) = seat.get_keyboard() {
                    if keyboard.is_grabbed() && !(keyboard.has_grab(serial) || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
                    {
                        grab.ungrab(PopupUngrabStrategy::All);
                        return;
                    }
                    // Don't move focus here.  I noticed GTK3 will (sometimes?) disable
                    // Cut/Copy/Paste if we move focus to the popup here, even though the xdg-shell
                    // spec says that xdg_popup.grab should always move the keyboard focus to the
                    // popup.  PopupKeyboardGrab will move focus if/when the user presses a key
                    // while the popup has the grab, which will be after GTK makes its decision on
                    // what to do with the menu entries.
                    keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
                }
                if let Some(pointer) = seat.get_pointer() {
                    if pointer.is_grabbed()
                        && !(pointer.has_grab(serial) || pointer.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
                    {
                        grab.ungrab(PopupUngrabStrategy::All);
                        return;
                    }
                    pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
                }
            }
        }
    }

    fn show_window_menu(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial, location: Point<i32, Logical>) {
        if let Some(window) = self
            .core
            .workspace_manager
            .active_workspace()
            .find_window(|e| e.0.toplevel() == Some(&surface))
            && let Some(seat) = Seat::<Self>::from_resource(&seat)
        {
            self.pop_up_window_menu(&window, &seat, serial, ActionLocation::WindowRelative(location));
        }
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        compositor::with_states(surface.wl_surface(), |states| {
            if let Some(data) = states.data_map.get::<XdgToplevelSurfaceData>()
                && let Some(elem) = self.window_for_toplevel_surface(&surface)
            {
                let data = data.lock().unwrap();

                if let Some(window_decorations) = elem.decoration_state_mut().window_decorations_mut() {
                    window_decorations.update(DecorationInput::Title(Some(data.title.clone().unwrap_or_default())));
                }

                self.core.toplevel_changed(
                    &elem,
                    ToplevelChangedInput {
                        title: data.title.clone(),
                        ..Default::default()
                    },
                );
            }
        });
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        if let Some(elem) = self.window_for_toplevel_surface(&surface) {
            let app_id = compositor::with_states(surface.wl_surface(), |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .and_then(|data| data.lock().unwrap().app_id.clone())
            });

            let mut props = elem.props();

            if props.window_icon.update_app_id(app_id.clone())
                && let Some(window_decorations) = elem.decoration_state_mut().window_decorations_mut()
            {
                let depends_on_theme = props.window_icon.depends_on_theme();
                window_decorations.update(DecorationInput::IconChanged { depends_on_theme });
            }

            self.core.toplevel_changed(
                &elem,
                ToplevelChangedInput {
                    app_id,
                    ..Default::default()
                },
            );
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some(elem) = self.window_for_toplevel_surface(&surface) {
            self.set_window_minimized(&elem);
        }
    }

    fn client_pong(&mut self, shell_client: ShellClient) {
        let (token, client) = shell_client
            .with_data(|user_data| {
                let token = user_data.get_or_insert(PingTimeoutToken::default).0.borrow_mut().take();
                let client = user_data.get::<Client>().cloned();
                (token, client)
            })
            .ok()
            .unwrap_or_default();

        if let Some(token) = token {
            self.core.loop_handle.remove(token);
        }

        if let Some(client) = client {
            for (dialog_id, window) in &self.core.shell_state.not_responding_dialogs {
                let window_client = window.0.wl_surface().and_then(|surface| surface.client());
                if window_client.is_some_and(|window_client| window_client == client) {
                    self.core.compositor_ui_state.cancel_dialog(*dialog_id);
                }
            }
        }
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            if self.core.cycling_state.is_tabwin_window(&window) {
                self.clear_window_cycling_state();
            }
            window.handle_destroyed();
            self.remove_window(&window);
            self.core.toplevel_destroyed(&window);
        }
    }

    fn popup_destroyed(&mut self, surface: PopupSurface) {
        // The PopupKeyboardGrab moves keyboard focus onto the popup while it is navigated with the
        // keyboard. If the popup still holds focus when it is dismissed, hand focus back to its
        // parent now: smithay only restores focus lazily on the next input event, and a
        // focus-requiring request (e.g. wl_data_device.set_selection from a menu's Copy action)
        // can arrive in that gap and be denied because the focused surface no longer exists.
        let focused_surface = self
            .core
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus())
            .and_then(|focus| focus.wl_surface().map(|surface| surface.into_owned()));
        if focused_surface.as_ref() == Some(surface.wl_surface())
            && let Some(parent_focus) = surface
                .get_parent_surface()
                .and_then(|parent| self.keyboard_focus_target_for_surface(&parent))
        {
            self.focus_target(parent_focus, SERIAL_COUNTER.next_serial(), None);
        }

        if let Some(parent) = surface.get_parent_surface()
            && let Some(anchor) = self.core.window_menu_state.window_menu_anchor()
            && anchor.wl_surface().as_deref() == Some(&parent)
        {
            self.core.window_menu_state.reset_pending();
        }
    }
}

impl<BackendData: Backend> XdgDialogHandler for Xfwl4State<BackendData> {
    fn dialog_hint_changed(&mut self, surface: ToplevelSurface, _hint: ToplevelDialogHint) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.update_window_capabilities(&window);
        }
    }
}

impl WindowElement {
    fn find_content_size(&self) -> Option<Size<i32, Logical>> {
        // For unmapped windows, some of these may be 0x0.  Use the inner Window's geometry
        // (content area only, without SSD decorations).
        let geometry = SpaceElement::geometry(&self.0);
        let bbox = SpaceElement::bbox(&self.0);
        let xdg_geometry = self.0.toplevel().and_then(|toplevel| {
            with_states(toplevel.wl_surface(), |states| {
                states.cached_state.get::<SurfaceCachedState>().current().geometry
            })
        });

        if geometry.size.w > 0 && geometry.size.h > 0 {
            Some(geometry.size)
        } else if bbox.size.w > 0 && bbox.size.h > 0 {
            Some(bbox.size)
        } else if let Some(xdg_geom) = xdg_geometry
            && xdg_geom.size.w > 0
            && xdg_geom.size.h > 0
        {
            Some(xdg_geom.size)
        } else {
            None
        }
    }
}

impl<BackendData: Backend> Xfwl4State<BackendData> {
    fn keyboard_focus_target_for_surface(&self, surface: &WlSurface) -> Option<KeyboardFocusTarget> {
        self.core
            .shell_state
            .popup_manager
            .find_popup(surface)
            .map(KeyboardFocusTarget::from)
            .or_else(|| self.window_for_surface(surface).map(KeyboardFocusTarget::from))
            .or_else(|| {
                self.core
                    .workspace_manager
                    .outputs()
                    .find_map(|output| {
                        layer_map_for_output(output)
                            .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                            .cloned()
                    })
                    .map(KeyboardFocusTarget::from)
            })
    }

    pub(super) fn unconstrain_popup(&self, popup: &PopupSurface) {
        let workspace = self.core.workspace_manager.active_workspace();

        // The popup's `state.geometry.loc` is relative to its parent surface's `window_geometry`
        // rect (see `xdg_popup.configure`). To constrain in those coords, compute the screen
        // position of that rect's upper-left in global (space) coordinates: for an xdg_toplevel
        // parent that's `mapped_loc + ssd_offset + window_geometry.loc` (xfwl4 maps windows at the
        // SSD top-left); for a layer-shell parent it's `layer_geometry.loc + output.loc`
        // (`layer_geometry` is output-local, so the output's global origin must be added).
        if let Some((outputs_for_popup, parent_geometry_origin, avoid_exclusive_zones)) =
            find_popup_root_surface(&PopupKind::Xdg(popup.clone())).ok().and_then(|root| {
                workspace
                    .window_for_surface(&root)
                    .and_then(|window| {
                        let outputs = workspace.outputs_for_window(&window);
                        if !outputs.is_empty()
                            && let Some(geom) = workspace.window_geometry(&window)
                        {
                            let decorations_offset = window
                                .decoration_state()
                                .window_decorations()
                                .map(|d| d.decorations_offset())
                                .unwrap_or_default();
                            let window_geometry_loc = compositor::with_states(&root, |states| {
                                states
                                    .cached_state
                                    .get::<SurfaceCachedState>()
                                    .current()
                                    .geometry
                                    .map(|g| g.loc)
                                    .unwrap_or_default()
                            });
                            Some((outputs, geom.loc + decorations_offset + window_geometry_loc, true))
                        } else {
                            None
                        }
                    })
                    .or_else(|| {
                        self.core.workspace_manager.outputs().find_map(|output| {
                            let output_loc = self
                                .core
                                .workspace_manager
                                .output_geometry(output)
                                .map(|geom| geom.loc)
                                .unwrap_or_default();
                            let layer_map = layer_map_for_output(output);
                            layer_map
                                .layer_for_surface(&root, WindowSurfaceType::TOPLEVEL)
                                .and_then(|layer_surface| layer_map.layer_geometry(layer_surface))
                                .map(|geom| (vec![output.clone()], geom.loc + output_loc, false))
                        })
                    })
            })
        {
            let parent_origin = parent_geometry_origin + get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));

            // xdg-shell only accepts a flip that fully removes the constraint, so a target the popup
            // cannot possibly fit in leaves it wherever the client asked for it -- usually off-screen
            // -- rather than somewhere sensible.  A layer-shell parent generally sits inside the
            // exclusive zone it reserved for itself, which puts its popups' anchor rects outside the
            // non-exclusive zone entirely, so those are constrained to the whole output.  Popups of
            // regular windows still prefer to leave exclusive zones alone, but fall back to the whole
            // output when that turns out to be unsatisfiable for the same reason.
            let fits = avoid_exclusive_zones
                && self
                    .popup_constraint_rect(&outputs_for_popup, parent_origin, true)
                    .is_some_and(|target| position_popup_within(popup, target));
            if !fits && let Some(target) = self.popup_constraint_rect(&outputs_for_popup, parent_origin, false) {
                position_popup_within(popup, target);
            }
        }
    }

    /// Union of the geometries of `outputs`, translated to be relative to a popup's parent's window
    /// geometry origin at `parent_origin`.  With `avoid_exclusive_zones`, each output contributes
    /// only the area that layer-shell surfaces have not reserved.
    fn popup_constraint_rect(
        &self,
        outputs: &[Output],
        parent_origin: Point<i32, Logical>,
        avoid_exclusive_zones: bool,
    ) -> Option<Rectangle<i32, Logical>> {
        outputs
            .iter()
            .filter_map(|output| {
                self.core.workspace_manager.output_geometry(output).map(|geom| {
                    if avoid_exclusive_zones {
                        let zone = layer_map_for_output(output).non_exclusive_zone();
                        geom.intersection(Rectangle::new(geom.loc + zone.loc, zone.size)).unwrap_or(geom)
                    } else {
                        geom
                    }
                })
            })
            .reduce(|acc, geom| acc.merge(geom))
            .map(|geom| Rectangle::new(geom.loc - parent_origin, geom.size))
    }

    /// Should be called on `WlSurface::commit` of xdg toplevel
    fn handle_toplevel_commit(&mut self, surface: &WlSurface) -> Option<()> {
        if let Some(window) = self.core.shell_state.pending_windows.get(surface) {
            if self.handle_new_window_placement(window.clone(), surface) {
                self.core.shell_state.pending_windows.remove(surface);
            }
        } else {
            let window = self
                .core
                .workspace_manager
                .active_workspace()
                .visible_windows()
                .find(|w: &&WindowElement| w.wl_surface().as_deref() == Some(surface))
                .cloned()?;

            self.update_window_capabilities(&window);

            if self.window_is_tabwin(&window, surface) {
                if let Some(size) = window.find_content_size() {
                    self.place_tabwin(&window, size);
                } else if let Some(toplevel_surface) = window.0.toplevel() {
                    toplevel_surface.send_configure();
                }
            } else {
                let space = self.core.workspace_manager.active_workspace_mut();
                let mut window_loc = space.window_location(&window)?;
                // `Window::geometry()` clamps to a bounding box that smithay only recomputes after this
                // commit's post-commit hooks have returned, so it would report the pre-commit size for a
                // commit that grows the window.
                let bbox = bbox_from_surface_tree(surface, (0, 0));
                let inner_geometry = with_states(surface, |states| {
                    states
                        .cached_state
                        .get::<SurfaceCachedState>()
                        .current()
                        .geometry
                        .and_then(|geo| geo.intersection(bbox))
                })
                .unwrap_or(bbox);
                let decorations_offset = window
                    .decoration_state()
                    .window_decorations()
                    .map(|d| d.decorations_offset())
                    .unwrap_or_default();

                let resize_result = with_states(window.wl_surface().as_deref()?, |states| {
                    let mut data = states.data_map.get::<RefCell<SurfaceData>>()?.borrow_mut();

                    let (resize_data, resize_finished) = match data.resize_state {
                        ResizeState::Resizing(d) => Some((d, false)),
                        ResizeState::WaitingForCommit(d, size_at_grab_end) => {
                            let mut toplevel_state = states.cached_state.get::<ToplevelCachedState>();
                            let last_acked = toplevel_state.current().last_acked.as_ref().map(|configure| &configure.state);
                            let still_resizing = last_acked.is_some_and(|state| state.states.contains(xdg_toplevel::State::Resizing));
                            // A client can ack the final configure and commit its old buffer before it has
                            // redrawn at the new size; ending the resize there would strand the window at
                            // its mid-resize position.  Any size change counts as applying the configure,
                            // so clients whose size hints keep them off the acked size still finish.  One
                            // that never applies it at all stays here until something else places the
                            // window and clears the state.
                            let applied = last_acked.and_then(|state| state.size) == Some(inner_geometry.size)
                                || inner_geometry.size != size_at_grab_end;
                            let finished = !still_resizing && applied;
                            if finished {
                                data.resize_state = ResizeState::NotResizing;
                            }
                            Some((d, finished))
                        }
                        ResizeState::NotResizing => None,
                    }?;

                    let edges = resize_data.edges;
                    let loc = resize_data.initial_window_location;
                    let size = resize_data.initial_window_size;

                    let new_loc = edges.intersects(ResizeEdge::TOP_LEFT).then(|| {
                        let new_x = edges
                            .intersects(ResizeEdge::LEFT)
                            .then_some(loc.x + (size.w - inner_geometry.size.w));

                        let new_y = edges
                            .intersects(ResizeEdge::TOP)
                            .then_some(loc.y + (size.h - inner_geometry.size.h));

                        Point::<Option<i32>, Logical>::from((new_x, new_y))
                    });

                    // Warping mid-resize keeps the pointer on the edge being dragged; the last warp
                    // waits for the commit that ends the resize, so that it lands on the geometry the
                    // client settled on rather than on one it is about to replace.
                    let warp_pointer =
                        resize_data.warp_pointer && (matches!(data.resize_state, ResizeState::Resizing(_)) || resize_finished);

                    Some((new_loc, edges, warp_pointer))
                });

                if let Some((new_loc, edges, warp_pointer)) = resize_result {
                    if let Some(new_loc) = new_loc {
                        if let Some(new_x) = new_loc.x {
                            window_loc.x = new_x - decorations_offset.x;
                        }
                        if let Some(new_y) = new_loc.y {
                            window_loc.y = new_y - decorations_offset.y;
                        }

                        self.core.workspace_manager.relocate_window(&window, window_loc);
                    }

                    if warp_pointer {
                        if let Some(surface) = window.wl_surface() {
                            with_states(&surface, |states| {
                                if let Some(data) = states.data_map.get::<RefCell<SurfaceData>>() {
                                    let mut data = data.borrow_mut();
                                    if let ResizeState::Resizing(ref mut rd) = data.resize_state {
                                        rd.warp_in_progress = true;
                                    }
                                }
                            });
                        }
                        self.warp_pointer_to_resize_edge(&window, window_loc, edges);
                    }
                }
            }
        }

        Some(())
    }

    pub(in crate::core) fn warp_pointer_to_resize_edge(
        &mut self,
        window: &WindowElement,
        window_loc: Point<i32, Logical>,
        edges: ResizeEdge,
    ) {
        let inner_geometry = SpaceElement::geometry(&window.0);
        let geometry = window
            .decoration_state()
            .window_decorations()
            .map(|decorations| {
                let e = decorations.decorations_extents();
                Rectangle::new(
                    window_loc,
                    (inner_geometry.size.w + e.left + e.right, inner_geometry.size.h + e.top + e.bottom).into(),
                )
            })
            .unwrap_or_else(|| Rectangle::new(window_loc, inner_geometry.size));

        let new_pointer_location: Option<(CursorIcon, Point<i32, Logical>)> = match edges {
            ResizeEdge::TOP => Some((CursorIcon::NResize, (geometry.loc.x + geometry.size.w / 2, geometry.loc.y).into())),
            ResizeEdge::LEFT => Some((CursorIcon::WResize, (geometry.loc.x, geometry.loc.y + geometry.size.h / 2).into())),
            ResizeEdge::RIGHT => Some((
                CursorIcon::EResize,
                (geometry.loc.x + geometry.size.w, geometry.loc.y + geometry.size.h / 2).into(),
            )),
            ResizeEdge::BOTTOM => Some((
                CursorIcon::SResize,
                (geometry.loc.x + geometry.size.w / 2, geometry.loc.y + geometry.size.h).into(),
            )),
            ResizeEdge::BOTTOM_RIGHT => Some((
                CursorIcon::SeResize,
                (geometry.loc.x + geometry.size.w, geometry.loc.y + geometry.size.h).into(),
            )),
            _ => None,
        };

        if let Some((cursor_icon, location)) = new_pointer_location {
            let pointer = self.core.pointer.clone();
            let event = MotionEvent {
                location: location.to_f64(),
                serial: SERIAL_COUNTER.next_serial(),
                time: self.core.now_input(),
            };
            pointer.motion(self, None, &event);
            self.core.cursor_state.set_cursor(cursor_icon);
        }
    }

    fn handle_new_window_placement(&mut self, window: WindowElement, surface: &WlSurface) -> bool {
        if self.window_is_window_menu_anchor(surface) {
            self.handle_new_window_menu_parent(&window);
            true
        } else if let Some(size) = window.find_content_size() {
            if self.window_is_tabwin(&window, surface) {
                self.place_tabwin(&window, size);
                self.focus_window(&window, SERIAL_COUNTER.next_serial(), None);
            } else if self.window_is_system_dialog(&window, surface) {
                self.place_system_dialog(&window, size);
                self.focus_window(&window, SERIAL_COUNTER.next_serial(), None);
            } else {
                let StackResult {
                    location,
                    allow_activate,
                    needs_attention,
                } = self.stack_new_window(&window);
                self.place_window(&window, size, location, allow_activate);

                if needs_attention {
                    self.set_window_urgent_state(&window, true);
                }

                self.core.toplevel_created::<Self>(&window);
            }

            if let Some(toplevel_surface) = window.0.toplevel() {
                toplevel_surface.send_pending_configure();
            }

            true
        } else {
            tracing::debug!("No window size available during initial placement; sending configure");
            if let Some(toplevel_surface) = window.0.toplevel() {
                toplevel_surface.send_configure();
            }
            false
        }
    }

    pub(super) fn window_is_window_menu_anchor(&self, surface: &WlSurface) -> bool {
        self.core.client_is_ui_thread(surface.client())
            && compositor::with_states(surface, |states| {
                states.data_map.get::<XdgToplevelSurfaceData>().map(|data| {
                    data.lock()
                        .unwrap()
                        .title
                        .as_ref()
                        .is_some_and(|title| title == WINDOW_MENU_TOPLEVEL_TITLE)
                })
            })
            .unwrap_or(false)
    }

    fn handle_new_window_menu_parent(&mut self, window: &WindowElement) {
        if let Some(surface) = window.0.toplevel() {
            window.props().flags = WindowFlags::NO_CYCLE;
            self.core.window_menu_state.update_window_menu_anchor(window.clone());
            window.0.override_z_index(RenderZindex::Overlay as u8);

            surface.with_pending_state(move |state| {
                state.size = Some((1, 1).into());
                state.decoration_mode = Some(xdg_decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode::ServerSide);
            });

            if surface.is_initial_configure_sent() {
                surface.send_pending_configure();
            }
        }
    }

    fn window_is_system_dialog(&self, window: &WindowElement, surface: &WlSurface) -> bool {
        self.core.client_is_ui_thread(surface.client())
            && !self.window_is_tabwin(window, surface)
            && !self.window_is_window_menu_anchor(surface)
    }

    fn place_system_dialog(&mut self, window: &WindowElement, size: Size<i32, Logical>) {
        if let Some(output) = self.output_under_pointer()
            && let Some(output_geo) = self.core.workspace_manager.output_geometry(&output)
        {
            let window_size = size.to_f64();
            let output_size = output_geo.size.to_f64();
            let new_x = output_geo.loc.x as f64 + (output_size.w - window_size.w) / 2.;
            let new_y = output_geo.loc.y as f64 + (output_size.h - window_size.h) / 2.;
            let new_location = Point::new(new_x as i32, new_y as i32);

            window.props().flags |= WindowFlags::NO_CYCLE;
            self.set_window_stacking_layer(window, WindowStackingLayer::System);
            self.new_window(window.clone(), new_location, true, None);
        }
    }

    fn window_for_toplevel_surface(&self, surface: &ToplevelSurface) -> Option<WindowElement> {
        self.core
            .workspace_manager
            .find_window(|elem| elem.0.toplevel().is_some_and(|surf| surf == surface))
            .or_else(|| self.core.shell_state.pending_windows.get(surface.wl_surface()).cloned())
    }
}

impl<BackendData: Backend + 'static> Xfwl4Core<BackendData> {
    pub(super) fn xdg_send_client_ping(&self, client: ShellClient, window: &WindowElement) {
        if let Some(token_holder) = client
            .with_data(|user_data| {
                if let Some(client) = window.0.toplevel().and_then(|toplevel| toplevel.wl_surface().client()) {
                    user_data.insert_if_missing(|| client);
                }
                Rc::clone(&user_data.get_or_insert(PingTimeoutToken::default).0)
            })
            .ok()
            && client.send_ping(SERIAL_COUNTER.next_serial()).is_ok()
        {
            let window = window.clone();
            let token = self
                .loop_handle
                .insert_source(Timer::from_duration(WINDOW_PING_TIMEOUT), {
                    let token_holder_holder = RefCell::new(Some(Rc::clone(&token_holder)));
                    move |_, _, state| {
                        if let Some(token_holder) = token_holder_holder.borrow_mut().take() {
                            state.core.xdg_update_client_ping_token(token_holder, None);
                        }
                        state.core.show_window_unresponsive_dialog(&window);
                        TimeoutAction::Drop
                    }
                })
                .ok();

            if token.is_some() {
                self.xdg_update_client_ping_token(token_holder, token);
            }
        }
    }

    fn xdg_update_client_ping_token(&self, token_holder: Rc<RefCell<Option<RegistrationToken>>>, token: Option<RegistrationToken>) {
        if let Some(old_token) = token_holder.replace(token) {
            self.loop_handle.remove(old_token);
        }
    }

    pub(super) fn xdg_clear_ping_timeout(&self, surface: &ToplevelSurface) {
        if let Ok(Some(token_holder)) = surface
            .client()
            .with_data(|user_data| user_data.get::<PingTimeoutToken>().map(|timeout| Rc::clone(&timeout.0)))
        {
            self.xdg_update_client_ping_token(token_holder, None);
        }
    }
}

/// Runs `popup`'s positioner against `target`, which must be relative to the popup's parent's
/// window geometry, and stores the result as the popup's pending geometry.  Returns whether the
/// popup ended up entirely inside `target`.
fn position_popup_within(popup: &PopupSurface, target: Rectangle<i32, Logical>) -> bool {
    let geometry = popup.with_pending_state(|state| {
        let geometry = state.positioner.get_unconstrained_geometry(target);
        state.geometry = geometry;
        geometry
    });
    target.contains_rect(geometry)
}

/// The protocol demands us to always reply with a configure, regardless of whether we fulfilled
/// the request or not.
pub(in crate::core) fn send_unfulfilled_configure(surface: &ToplevelSurface) {
    if surface.is_initial_configure_sent() {
        surface.send_configure();
    }
}

pub fn app_id_for_xdg_toplevel(toplevel_surface: &ToplevelSurface) -> Option<String> {
    compositor::with_states(toplevel_surface.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|state| state.lock().unwrap().app_id.clone())
    })
}

pub fn desktop_app_info_for_xdg_toplevel(toplevel_surface: &ToplevelSurface) -> Option<gio::DesktopAppInfo> {
    compositor::with_states(toplevel_surface.wl_surface(), |states| {
        states.data_map.get::<XdgToplevelSurfaceData>().and_then(|state| {
            let s = state.lock().unwrap();
            s.app_id.as_ref().and_then(|app_id| {
                let desktop_name = if app_id.ends_with(".desktop") {
                    app_id
                } else {
                    &format!("{app_id}.desktop")
                };
                gio::DesktopAppInfo::new(desktop_name)
            })
        })
    })
}

pub fn app_name_for_xdg_toplevel(toplevel_surface: &ToplevelSurface, desktop_app_info: Option<&gio::DesktopAppInfo>) -> Option<String> {
    desktop_app_info
        .as_ref()
        .and_then(|app_info| {
            let name = app_info.name().to_string();
            (!name.is_empty()).then_some(name)
        })
        .or_else(|| {
            compositor::with_states(toplevel_surface.wl_surface(), |states| {
                states.data_map.get::<XdgToplevelSurfaceData>().and_then(|state| {
                    let s = state.lock().unwrap();
                    s.app_id.as_ref().and_then(|s| prettify_name(s))
                })
            })
        })
}

pub fn window_title_for_xdg_toplevel(surface: &ToplevelSurface) -> Option<String> {
    compositor::with_states(surface.wl_surface(), |states| {
        states.data_map.get::<XdgToplevelSurfaceData>().and_then(|data| {
            let d = data.lock().unwrap();
            d.title.clone()
        })
    })
}
