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
    cmp::Ordering,
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex},
};

use glib::CastNone;
use gtk::gio::{
    self,
    traits::{AppInfoExt, FileExt},
};
use smithay::{
    backend::{input::ButtonState, renderer::utils::Buffer},
    delegate_xdg_shell,
    desktop::{
        PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, Window, WindowSurfaceType, find_popup_root_surface,
        get_popup_toplevel_coords, layer_map_for_output, space::SpaceElement,
    },
    input::{
        Seat,
        pointer::{ButtonEvent, Focus, MotionEvent},
    },
    output::Output,
    reexports::{
        wayland_protocols::xdg::{decoration as xdg_decoration, shell::server::xdg_toplevel},
        wayland_server::{
            Resource,
            protocol::{wl_output, wl_seat, wl_surface::WlSurface},
        },
    },
    utils::{HookId, Logical, Point, Rectangle, SERIAL_COUNTER, Serial, Size},
    wayland::{
        compositor::{self, with_states},
        seat::WaylandFocus,
        shell::xdg::{
            Configure, PopupSurface, PositionerState, SurfaceCachedState, ToplevelCachedState, ToplevelSurface, XdgShellHandler,
            XdgShellState, XdgToplevelSurfaceData,
        },
        shm,
        xdg_toplevel_icon::ToplevelIconCachedState,
    },
};
use tracing::{trace, warn};

use crate::{
    backend::Backend,
    focus::{KeyboardFocusTarget, PointerFocusTarget},
    shell::{TouchMoveSurfaceGrab, TouchResizeSurfaceGrab, WindowIcon},
    state::Xfwl4State,
    ui::{
        ToUiMessage,
        window_menu::{self, FullscreenState, MaximizeState, RolledState, StackingState, WindowMenuState},
    },
    util::prettify_name,
};

use super::{
    FullscreenSurface, PointerMoveSurfaceGrab, PointerResizeSurfaceGrab, ResizeData, ResizeEdge, ResizeState, SurfaceData, WindowElement,
    fullscreen_output_geometry, place_new_window,
};

#[derive(Debug, Default)]
pub struct XdgSurfacePropsInner {
    pub is_minimized: bool,
}

#[derive(Debug, Default)]
pub struct XdgSurfaceProps(pub Mutex<XdgSurfacePropsInner>);

impl<BackendData: Backend> XdgShellHandler for Xfwl4State<BackendData> {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Do not send a configure here, the initial configure
        // of a xdg_surface has to be sent during the commit if
        // the surface is not already configured

        // Set the initial toplevel bounds so the client knows what size to use
        let pointer_location = self.pointer.current_location();
        let space = self.workspace_manager.active_workspace();
        let output = space
            .output_under(pointer_location)
            .next()
            .or_else(|| space.outputs().next())
            .cloned();
        let output_geometry = output
            .and_then(|o| {
                let geo = space.output_geometry(&o)?;
                let map = layer_map_for_output(&o);
                let zone = map.non_exclusive_zone();
                Some(Rectangle::new(geo.loc + zone.loc, zone.size))
            })
            .unwrap_or_else(|| Rectangle::from_size((800, 800).into()));
        surface.with_pending_state(|state| {
            state.bounds = Some(output_geometry.size);
        });

        let window = WindowElement(Window::new_wayland_window(surface.clone()));
        self.pending_windows.insert(surface.wl_surface().clone(), window);

        compositor::add_post_commit_hook(surface.wl_surface(), |state: &mut Self, _, surface| {
            state.handle_toplevel_commit(surface);
        });
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        // Do not send a configure here, the initial configure
        // of a xdg_surface has to be sent during the commit if
        // the surface is not already configured

        self.unconstrain_popup(&surface);

        if let Err(err) = self.popups.track_popup(PopupKind::from(surface)) {
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

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat: Seat<Xfwl4State<BackendData>> = Seat::from_resource(&seat).unwrap();
        self.move_request_xdg(&surface, &seat, serial)
    }

    fn resize_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial, edges: xdg_toplevel::ResizeEdge) {
        let seat: Seat<Xfwl4State<BackendData>> = Seat::from_resource(&seat).unwrap();

        if let Some(touch) = seat.get_touch()
            && touch.has_grab(serial)
        {
            let start_data = touch.grab_start_data().unwrap();
            tracing::info!(?start_data);

            // If the client disconnects after requesting a move
            // we can just ignore the request
            let Some(window) = self.window_for_surface(surface.wl_surface()) else {
                tracing::info!("no window");
                return;
            };

            // If the focus was for a different surface, ignore the request.
            if start_data.focus.is_none() || !start_data.focus.as_ref().unwrap().0.same_client_as(&surface.wl_surface().id()) {
                tracing::info!("different surface");
                return;
            }
            let geometry = window.geometry();
            let Some(loc) = self.workspace_manager.active_workspace().element_location(&window) else {
                return;
            };
            let (initial_window_location, initial_window_size) = (loc, geometry.size);

            with_states(surface.wl_surface(), move |states| {
                states.data_map.get::<RefCell<SurfaceData>>().unwrap().borrow_mut().resize_state = ResizeState::Resizing(ResizeData {
                    edges: edges.into(),
                    initial_window_location,
                    initial_window_size,
                });
            });

            let grab = TouchResizeSurfaceGrab {
                start_data,
                window,
                edges: edges.into(),
                initial_window_location,
                initial_window_size,
                last_window_size: initial_window_size,
            };

            touch.set_grab(self, grab, serial);
            return;
        }

        let pointer = seat.get_pointer().unwrap();

        // Check that this surface has a click grab.
        if !pointer.has_grab(serial) {
            return;
        }

        let start_data = pointer.grab_start_data().unwrap();

        let window = self.window_for_surface(surface.wl_surface()).unwrap();

        // If the focus was for a different surface, ignore the request.
        if start_data.focus.is_none() || !start_data.focus.as_ref().unwrap().0.same_client_as(&surface.wl_surface().id()) {
            return;
        }

        let geometry = window.geometry();
        let Some(loc) = self.workspace_manager.active_workspace().element_location(&window) else {
            return;
        };
        let (initial_window_location, initial_window_size) = (loc, geometry.size);

        with_states(surface.wl_surface(), move |states| {
            states.data_map.get::<RefCell<SurfaceData>>().unwrap().borrow_mut().resize_state = ResizeState::Resizing(ResizeData {
                edges: edges.into(),
                initial_window_location,
                initial_window_size,
            });
        });

        let grab = PointerResizeSurfaceGrab {
            start_data,
            window,
            edges: edges.into(),
            initial_window_location,
            initial_window_size,
            last_window_size: initial_window_size,
        };

        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    fn ack_configure(&mut self, surface: WlSurface, configure: Configure) {
        if let Configure::Toplevel(configure) = configure {
            if let Some(serial) = with_states(&surface, |states| {
                if let Some(data) = states.data_map.get::<RefCell<SurfaceData>>()
                    && let ResizeState::WaitingForFinalAck(_, serial) = data.borrow().resize_state
                {
                    return Some(serial);
                }

                None
            }) {
                // When the resize grab is released the surface
                // resize state will be set to WaitingForFinalAck
                // and the client will receive a configure request
                // without the resize state to inform the client
                // resizing has finished. Here we will wait for
                // the client to acknowledge the end of the
                // resizing. To check if the surface was resizing
                // before sending the configure we need to use
                // the current state as the received acknowledge
                // will no longer have the resize state set
                let is_resizing = with_states(&surface, |states| {
                    states
                        .cached_state
                        .get::<ToplevelCachedState>()
                        .current()
                        .last_acked
                        .as_ref()
                        .is_some_and(|c| c.state.states.contains(xdg_toplevel::State::Resizing))
                });

                if configure.serial >= serial && is_resizing {
                    with_states(&surface, |states| {
                        let mut data = states.data_map.get::<RefCell<SurfaceData>>().unwrap().borrow_mut();
                        if let ResizeState::WaitingForFinalAck(resize_data, _) = data.resize_state {
                            data.resize_state = ResizeState::WaitingForCommit(resize_data);
                        } else {
                            unreachable!()
                        }
                    });
                }
            }

            if let Some(window) = self.window_for_surface(&surface) {
                use xdg_decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
                let is_ssd = configure
                    .state
                    .decoration_mode
                    .map(|mode| mode == Mode::ServerSide)
                    .unwrap_or(false);
                if is_ssd {
                    self.enable_decorations_for_window(&window);
                } else {
                    window.disable_decorations();
                }
            }
        }
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, mut wl_output: Option<wl_output::WlOutput>) {
        // NOTE: This is only one part of the solution. We can set the
        // location and configure size here, but the surface should be rendered fullscreen
        // independently from its buffer size
        let wl_surface = surface.wl_surface();

        let output_geometry = fullscreen_output_geometry(wl_surface, wl_output.as_ref(), self.workspace_manager.active_workspace_mut());

        if let Some(geometry) = output_geometry {
            let output = wl_output
                .as_ref()
                .and_then(Output::from_resource)
                .unwrap_or_else(|| self.workspace_manager.active_workspace().outputs().next().unwrap().clone());
            let client = match self.display_handle.get_client(wl_surface.id()) {
                Ok(client) => client,
                Err(_) => return,
            };
            for output in output.client_outputs(&client) {
                wl_output = Some(output);
            }

            if let Some(window) = self.workspace_manager.active_workspace().window_for_surface(wl_surface) {
                surface.with_pending_state(|state| {
                    state.states.set(xdg_toplevel::State::Fullscreen);
                    state.size = Some(geometry.size);
                    state.fullscreen_output = wl_output;
                });
                output.user_data().insert_if_missing(FullscreenSurface::default);
                output.user_data().get::<FullscreenSurface>().unwrap().set(window.clone());
                trace!("Fullscreening: {:?}", window);
            }
        }

        // The protocol demands us to always reply with a configure,
        // regardless of we fulfilled the request or not
        if surface.is_initial_configure_sent() {
            surface.send_configure();
        } else {
            // Will be sent during initial configure
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        let ret = surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Fullscreen);
            state.size = None;
            state.fullscreen_output.take()
        });
        if let Some(output) = ret {
            let output = Output::from_resource(&output).unwrap();
            if let Some(fullscreen) = output.user_data().get::<FullscreenSurface>() {
                trace!("Unfullscreening: {:?}", fullscreen.get());
                fullscreen.clear();
                self.backend_data.reset_buffers(&output);
            }
        }

        // The protocol demands us to always reply with a configure,
        // regardless of we fulfilled the request or not
        if surface.is_initial_configure_sent() {
            surface.send_configure();
        } else {
            // Will be sent during initial configure
        }
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        // FIXME: This should use layer-shell when it is implemented to
        // get the correct maximum size
        let workspace = self.workspace_manager.active_workspace_mut();
        if let Some(window) = workspace.window_for_surface(surface.wl_surface()) {
            let outputs_for_window = workspace.outputs_for_element(&window);
            let output = outputs_for_window
                .first()
                // The window hasn't been mapped yet, use the primary output instead
                .or_else(|| workspace.outputs().next())
                // Assumes that at least one output exists
                .expect("No outputs found");
            let geometry = workspace.output_geometry(output).unwrap();

            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Maximized);
                state.size = Some(geometry.size);
            });
            workspace.map_element(window, geometry.loc, true);
        }

        // The protocol demands us to always reply with a configure,
        // regardless of we fulfilled the request or not
        if surface.is_initial_configure_sent() {
            surface.send_configure();
        } else {
            // Will be sent during initial configure
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Maximized);
            state.size = None;
        });

        // The protocol demands us to always reply with a configure,
        // regardless of we fulfilled the request or not
        if surface.is_initial_configure_sent() {
            surface.send_configure();
        } else {
            // Will be sent during initial configure
        }
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat: Seat<Xfwl4State<BackendData>> = Seat::from_resource(&seat).unwrap();
        let kind = PopupKind::Xdg(surface);
        if let Some(root) = find_popup_root_surface(&kind).ok().and_then(|root| {
            let workspace = self.workspace_manager.active_workspace();

            workspace.window_for_surface(&root).map(KeyboardFocusTarget::from).or_else(|| {
                workspace
                    .outputs()
                    .find_map(|o| {
                        let map = layer_map_for_output(o);
                        map.layer_for_surface(&root, WindowSurfaceType::TOPLEVEL).cloned()
                    })
                    .map(KeyboardFocusTarget::LayerSurface)
            })
        }) {
            let ret = self.popups.grab_popup(root, kind, &seat, serial);

            if let Ok(mut grab) = ret {
                if let Some(keyboard) = seat.get_keyboard() {
                    if keyboard.is_grabbed() && !(keyboard.has_grab(serial) || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
                    {
                        grab.ungrab(PopupUngrabStrategy::All);
                        return;
                    }
                    keyboard.set_focus(self, grab.current_grab(), serial);
                    keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
                }
                if let Some(pointer) = seat.get_pointer() {
                    if pointer.is_grabbed()
                        && !(pointer.has_grab(serial) || pointer.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
                    {
                        grab.ungrab(PopupUngrabStrategy::All);
                        return;
                    }
                    tracing::debug!("setting pointer grab");
                    pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
                }
            }
        }
    }

    fn show_window_menu(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial, location: Point<i32, Logical>) {
        tracing::debug!("show_window_menu at {location:?}");

        let workspace = self.workspace_manager.active_workspace();
        if let Some(window) = workspace.find_element(|e| e.0.toplevel() == Some(&surface))
            && let Some(window_location) = workspace.element_location(&window)
        {
            let location = window_location + location;
            tracing::info!("asking to position window menu at {location:?}");

            let _ = self.to_ui_channel_tx.send(ToUiMessage::ShowWindowMenu(WindowMenuState {
                location,
                maximize_state: MaximizeState::Normal,
                can_minimize: true,
                can_move: true,
                can_resize: true,
                stacking_state: StackingState::Normal,
                rolled_state: RolledState::Normal,
                fullscreen_state: FullscreenState::Normal,
                pinned: false,
                can_move_workspaces: true,
                current_workspace: 0,
                workspace_names: vec![],
                can_close: true,
            }));
        }
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        compositor::with_states(surface.wl_surface(), |states| {
            if let Some(data) = states.data_map.get::<XdgToplevelSurfaceData>()
                && let Some(elem) = self.window_for_toplevel_surface(&surface)
            {
                let data = data.lock().unwrap();
                if let Some(window_decorations) = elem.decoration_state().window_decorations_mut() {
                    window_decorations.update_window_title(data.title.as_deref().unwrap_or(""));
                }
            }
        });
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        if let Some(elem) = self.window_for_toplevel_surface(&surface) {
            // When the app_id changes, the app/window icon might change.
            if let Some(window_decorations) = elem.decoration_state().window_decorations_mut() {
                let scale = self
                    .workspace_manager
                    .find_element(|elem| elem.0.wl_surface().is_some_and(|surf| surf.as_ref() == surface.wl_surface()))
                    .map(|elem| self.workspace_manager.outputs_for_element(&elem))
                    .unwrap_or_else(|| self.workspace_manager.outputs().cloned().collect())
                    .first()
                    .map(|output| output.current_scale().integer_scale())
                    .unwrap_or(1);
                let app_info = desktop_app_info_for_xdg_toplevel(&surface);
                let icon =
                    icon_for_xdg_toplevel(&surface, scale, app_info.as_ref()).and_then(|icon| self.window_icon_to_image_data(&icon).ok());
                window_decorations.update_app_icon(icon);
            }
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some(elem) = self.window_for_toplevel_surface(&surface) {
            self.workspace_manager.set_window_minimized(&elem);
        }
    }
}

delegate_xdg_shell!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);

impl<BackendData: Backend> Xfwl4State<BackendData> {
    pub fn move_request_xdg(&mut self, surface: &ToplevelSurface, seat: &Seat<Self>, serial: Serial) {
        if let Some(touch) = seat.get_touch()
            && touch.has_grab(serial)
        {
            let start_data = touch.grab_start_data().unwrap();

            // If the client disconnects after requesting a move
            // we can just ignore the request
            let Some(window) = self.window_for_surface(surface.wl_surface()) else {
                return;
            };

            // If the focus was for a different surface, ignore the request.
            if start_data.focus.is_none() || !start_data.focus.as_ref().unwrap().0.same_client_as(&surface.wl_surface().id()) {
                return;
            }

            let Some(mut initial_window_location) = self.workspace_manager.active_workspace().element_location(&window) else {
                return;
            };

            // If surface is maximized then unmaximize it
            let changed = surface.with_pending_state(|state| {
                if state.states.unset(xdg_toplevel::State::Maximized) {
                    state.size = None;
                    true
                } else {
                    false
                }
            });
            if changed {
                surface.send_configure();

                // TODO: In real compositor mouse location should be mapped to a new window size
                // For example, you could:
                // 1) transform mouse pointer position from compositor space to window space (location relative)
                // 2) divide the x coordinate by width of the window to get the percentage
                //   - 0.0 would be on the far left of the window
                //   - 0.5 would be in middle of the window
                //   - 1.0 would be on the far right of the window
                // 3) multiply the percentage by new window width
                // 4) by doing that, drag will look a lot more natural
                initial_window_location = start_data.location.to_i32_round();
            }

            let grab = TouchMoveSurfaceGrab {
                start_data,
                window,
                initial_window_location,
            };

            touch.set_grab(self, grab, serial);
            return;
        }

        let pointer = seat.get_pointer().unwrap();

        // Check that this surface has a click grab.
        if !pointer.has_grab(serial) {
            return;
        }

        let start_data = pointer.grab_start_data().unwrap();

        // If the client disconnects after requesting a move
        // we can just ignore the request
        let Some(window) = self.window_for_surface(surface.wl_surface()) else {
            return;
        };

        // If the focus was for a different surface, ignore the request.
        if start_data.focus.is_none() || !start_data.focus.as_ref().unwrap().0.same_client_as(&surface.wl_surface().id()) {
            return;
        }

        if let Some(mut initial_window_location) = self.workspace_manager.active_workspace().element_location(&window) {
            // If surface is maximized then unmaximize it
            let changed = surface.with_pending_state(|state| {
                if state.states.unset(xdg_toplevel::State::Maximized) {
                    state.size = None;
                    true
                } else {
                    false
                }
            });
            if changed {
                surface.send_configure();

                // NOTE: In real compositor mouse location should be mapped to a new window size
                // For example, you could:
                // 1) transform mouse pointer position from compositor space to window space (location relative)
                // 2) divide the x coordinate by width of the window to get the percentage
                //   - 0.0 would be on the far left of the window
                //   - 0.5 would be in middle of the window
                //   - 1.0 would be on the far right of the window
                // 3) multiply the percentage by new window width
                // 4) by doing that, drag will look a lot more natural
                //
                // but for xfwl4 needs setting location to pointer location is fine
                let pos = pointer.current_location();
                initial_window_location = (pos.x as i32, pos.y as i32).into();
            }

            let grab = PointerMoveSurfaceGrab {
                start_data,
                window,
                initial_window_location,
            };

            pointer.set_grab(self, grab, serial, Focus::Clear);
        }
    }

    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self.window_for_surface(&root) else {
            return;
        };

        let workspace = self.workspace_manager.active_workspace();

        let mut outputs_for_window = workspace.outputs_for_element(&window);
        if outputs_for_window.is_empty() {
            return;
        }

        // Get a union of all outputs' geometries.
        let mut outputs_geo = workspace.output_geometry(&outputs_for_window.pop().unwrap()).unwrap();
        for output in outputs_for_window {
            outputs_geo = outputs_geo.merge(workspace.output_geometry(&output).unwrap());
        }

        let window_geo = workspace.element_geometry(&window).unwrap();

        // The target geometry for the positioner should be relative to its parent's geometry, so
        // we will compute that here.
        let mut target = outputs_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }

    /// Should be called on `WlSurface::commit` of xdg toplevel
    fn handle_toplevel_commit(&mut self, surface: &WlSurface) -> Option<()> {
        if let Some(window) = self.pending_windows.get(surface) {
            if self.handle_new_window_placement(window.clone(), surface) {
                self.pending_windows.remove(surface);
            }
        } else {
            let window = self
                .workspace_manager
                .active_workspace()
                .elements()
                .find(|w| w.wl_surface().as_deref() == Some(surface))
                .cloned()?;

            if self.window_is_tabwin(&window, surface) {
                if let Some(size) = self.find_window_geometry(&window) {
                    self.place_tabwin(&window, size);
                } else if let Some(toplevel_surface) = window.0.toplevel() {
                    toplevel_surface.send_configure();
                }
            } else {
                let space = self.workspace_manager.active_workspace_mut();
                let mut window_loc = space.element_location(&window)?;
                let geometry = window.geometry();

                let new_loc: Point<Option<i32>, Logical> = with_states(window.wl_surface().as_deref()?, |states| {
                    let data = states.data_map.get::<RefCell<SurfaceData>>()?.borrow_mut();

                    if let ResizeState::Resizing(resize_data) = data.resize_state {
                        let edges = resize_data.edges;
                        let loc = resize_data.initial_window_location;
                        let size = resize_data.initial_window_size;

                        // If the window is being resized by top or left, its location must be adjusted
                        // accordingly.
                        edges.intersects(ResizeEdge::TOP_LEFT).then(|| {
                            let new_x = edges.intersects(ResizeEdge::LEFT).then_some(loc.x + (size.w - geometry.size.w));

                            let new_y = edges.intersects(ResizeEdge::TOP).then_some(loc.y + (size.h - geometry.size.h));

                            (new_x, new_y).into()
                        })
                    } else {
                        None
                    }
                })?;

                if let Some(new_x) = new_loc.x {
                    window_loc.x = new_x;
                }
                if let Some(new_y) = new_loc.y {
                    window_loc.y = new_y;
                }

                if new_loc.x.is_some() || new_loc.y.is_some() {
                    // If TOP or LEFT side of the window got resized, we have to move it
                    space.map_element(window, window_loc, false);
                }
            }
        }

        Some(())
    }

    fn handle_new_window_placement(&mut self, window: WindowElement, surface: &WlSurface) -> bool {
        if self.handle_new_window_menu_parent(&window) {
            if let Some(toplevel_surface) = window.0.toplevel() {
                toplevel_surface.send_pending_configure();
            }

            true
        } else if let Some(size) = self.find_window_geometry(&window) {
            if self.window_is_tabwin(&window, surface) {
                self.place_tabwin(&window, size);
                if let Some(keyboard) = self.seat.get_keyboard() {
                    keyboard.set_focus(self, Some(KeyboardFocusTarget::from(window.clone())), SERIAL_COUNTER.next_serial());
                }
            } else {
                let space = self.workspace_manager.active_workspace_mut();
                place_new_window(space, self.pointer.current_location(), &window, true);
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

    fn handle_new_window_menu_parent(&mut self, window: &WindowElement) -> bool {
        if let Some(toplevel_surface) = window.0.toplevel()
            && self.ui_thread_client.is_some()
            && toplevel_surface.wl_surface().client() == self.ui_thread_client
            && let Some(title) = compositor::with_states(toplevel_surface.wl_surface(), |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .and_then(|data| data.lock().unwrap().title.clone())
            })
            && let Some(location) = window_menu::parse_title(&title)
        {
            let location = location.to_f64();
            let workspace = self.workspace_manager.active_workspace();
            let output = workspace
                .output_under(location)
                .next()
                .or_else(|| workspace.outputs().next())
                .cloned();
            let output_geometry = output
                .and_then(|o| workspace.output_geometry(&o))
                .unwrap_or_else(|| Rectangle::from_size((800, 800).into()));

            toplevel_surface.with_pending_state(move |state| {
                state.size = Some(output_geometry.size);
            });

            tracing::info!("positioning window menu's toplevel at {location:?}");
            self.workspace_manager
                .active_workspace_mut()
                .map_element(window.clone(), (0, 0), true);
            self.update_keyboard_focus(location, SERIAL_COUNTER.next_serial());

            let hook_id = Arc::new(Mutex::new(None::<HookId>));
            let id = compositor::add_post_commit_hook::<Self, _>(toplevel_surface.wl_surface(), {
                let hook_id = Arc::clone(&hook_id);
                move |state, _, surface| {
                    const BTN_RIGHT: u32 = 0x111;

                    let pointer = state.pointer.clone();

                    let event = ButtonEvent {
                        serial: SERIAL_COUNTER.next_serial(), // TODO: use serial from show_window_menu request?
                        time: state.clock.now().as_millis(),
                        button: BTN_RIGHT,
                        state: ButtonState::Released,
                    };
                    tracing::debug!("synthesizing button release");
                    pointer.button(state, &event);

                    let target = (PointerFocusTarget::WlSurface(surface.clone()), location);
                    let event = MotionEvent {
                        location: pointer.current_location().to_i32_round(),
                        serial: SERIAL_COUNTER.next_serial(), // TODO: use serial from show_window_menu request?
                        time: state.clock.now().as_millis(),
                    };
                    tracing::debug!("synthesizing pointer motion");
                    pointer.motion(state, Some(target), &event);

                    let event = ButtonEvent {
                        serial: SERIAL_COUNTER.next_serial(), // TODO: use serial from show_window_menu request?
                        time: state.clock.now().as_millis(),
                        button: BTN_RIGHT,
                        state: ButtonState::Pressed,
                    };
                    tracing::debug!("synthesizing button press");
                    pointer.button(state, &event);

                    if let Some(hook_id) = hook_id.lock().unwrap().take() {
                        compositor::remove_post_commit_hook(surface, hook_id);
                    }
                }
            });
            hook_id.lock().unwrap().replace(id);

            true
        } else {
            tracing::debug!("didn't get window menu");
            false
        }
    }

    fn find_window_geometry(&mut self, window: &WindowElement) -> Option<Size<i32, Logical>> {
        // For unmapped windows, some of these may be 0x0.
        let geometry = window.geometry();
        let bbox = window.bbox();
        let xdg_geometry = window.0.toplevel().and_then(|toplevel| {
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

    fn window_for_toplevel_surface(&self, surface: &ToplevelSurface) -> Option<WindowElement> {
        self.workspace_manager
            .find_element(|elem| elem.0.toplevel().is_some_and(|surf| surf == surface))
            .or_else(|| self.pending_windows.get(surface.wl_surface()).cloned())
    }
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

pub fn icon_for_xdg_toplevel(
    toplevel_surface: &ToplevelSurface,
    scale: i32,
    desktop_app_info: Option<&gio::DesktopAppInfo>,
) -> Option<WindowIcon> {
    with_states(toplevel_surface.wl_surface(), |states| {
        let mut icon_state = states.cached_state.get::<ToplevelIconCachedState>();
        icon_state
            .current()
            .icon_name()
            .and_then(|name| {
                if name.starts_with('/') {
                    PathBuf::from_str(name).ok().map(WindowIcon::File)
                } else {
                    Some(WindowIcon::Named(name.to_owned()))
                }
            })
            .or_else(|| {
                let buffers_sorted = {
                    let mut bufs = icon_state.current().buffers().iter().collect::<Vec<_>>();
                    bufs.sort_by(|first, second| {
                        let scale_cmp = first.1.cmp(&second.1);
                        if scale_cmp != Ordering::Equal {
                            scale_cmp
                        } else {
                            // xdg-toplevel-icon requires that buffers passed are SHM buffers.
                            let first_size = shm::with_buffer_contents(&first.0, |_, _, data| data.width.max(data.height)).unwrap_or(0);
                            let second_size = shm::with_buffer_contents(&second.0, |_, _, data| data.width.max(data.height)).unwrap_or(0);
                            first_size.cmp(&second_size)
                        }
                    });
                    bufs
                };

                buffers_sorted
                    .iter()
                    .find(|(_, buf_scale)| *buf_scale == scale)
                    .or_else(|| buffers_sorted.first())
                    .map(|(buffer, _)| WindowIcon::Buffer(Buffer::with_implicit(buffer.clone())))
            })
    })
    .or_else(|| {
        desktop_app_info.and_then(|app_info| {
            app_info
                .icon()
                .and_downcast_ref::<gio::FileIcon>()
                .and_then(|icon| icon.file().path().map(WindowIcon::File))
                .or_else(|| {
                    app_info
                        .icon()
                        .and_downcast_ref::<gio::ThemedIcon>()
                        .and_then(|icon| icon.names().first().map(|s| WindowIcon::Named(s.to_string())))
                })
        })
    })
}
