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
    os::unix::io::OwnedFd,
    sync::{Mutex, MutexGuard},
};

use smithay::{
    delegate_xwayland_keyboard_grab, delegate_xwayland_shell,
    desktop::{Window, WindowSurface, space::SpaceElement},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Rectangle, SERIAL_COUNTER},
    wayland::{
        seat::WaylandFocus,
        selection::{
            SelectionTarget,
            data_device::{
                clear_data_device_selection, current_data_device_selection_userdata, request_data_device_client_selection,
                set_data_device_selection,
            },
            primary_selection::{
                clear_primary_selection, current_primary_selection_userdata, request_primary_client_selection, set_primary_selection,
            },
        },
        xwayland_keyboard_grab::XWaylandKeyboardGrabHandler,
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        X11Surface, X11Wm, XwmHandler,
        xwm::{Reorder, ResizeEdge as X11ResizeEdge, WmWindowProperty, WmWindowType, XwmId},
    },
};
use tracing::{error, trace};

use crate::{
    backend::Backend,
    core::{
        config::ActivateAction,
        focus::KeyboardFocusTarget,
        placement::StackResult,
        shell::{GrabTrigger, WindowState},
        state::{WindowClient, Xfwl4State},
        util::ImageData,
    },
};

use super::WindowElement;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct X11ClientId(pub u32);

#[derive(Debug, Default)]
pub struct X11WindowPropsInner {
    pub client_frame_left: u32,
    pub client_frame_right: u32,
    pub client_frame_top: u32,
    pub client_frame_bottom: u32,
}

#[derive(Debug, Default)]
pub struct X11WindowProps(pub Mutex<X11WindowPropsInner>);

impl<BackendData: Backend> XWaylandShellHandler for Xfwl4State<BackendData> {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.core.shell_protocol_delegates.xwayland_shell_state
    }
}

delegate_xwayland_shell!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);

impl<BackendData: Backend> XwmHandler for Xfwl4State<BackendData> {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.core.xwayland.as_mut().unwrap().xwm()
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}
    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let parent = surface.is_transient_for().and_then(|window_id| {
            self.core
                .workspace_manager
                .active_workspace()
                .find_window(|elem| matches!(elem.0.underlying_surface(), WindowSurface::X11(surface) if surface.window_id() == window_id))
        });

        let _ = surface.set_mapped(true);

        if let Some(xw) = self.core.xwayland.as_ref() {
            let _ = xw.init_new_window_event_mask(surface.window_id());
        }

        surface
            .user_data()
            .insert_if_missing(|| X11ClientId(surface.window_id() & self.core.xwayland.as_ref().unwrap().client_resource_mask()));
        let window = WindowElement::new(
            Window::new_x11_window(surface.clone()),
            self.core.next_window_id(),
            &self.core.config,
        );
        self.x11_update_window_gtk_frame_extents(&window);
        self.set_window_parent(&window, parent.clone());

        if !surface.is_decorated() {
            self.enable_decorations_for_window(&window);
        } else {
            self.disable_decorations_for_window(&window);
        }

        let StackResult {
            location,
            allow_activate,
            needs_attention,
        } = self.stack_new_window(&window);
        self.place_window(&window, SpaceElement::geometry(&window).size, location, allow_activate);

        if needs_attention {
            self.set_window_urgent_state(&window, true);
        }

        let workspace = self.core.workspace_manager.active_workspace_mut();
        if let Some(bbox) = workspace.window_bbox(&window) {
            let _ = surface.configure(Some(bbox));
        }

        if surface.is_maximized() {
            self.set_window_maximized(&window, None);
        }
        if surface.is_shaded() {
            self.set_window_shaded(&window, true);
        }
        if surface.is_sticky() {
            self.set_window_sticky(&window, true);
        }
        if surface.is_hidden() {
            self.set_window_minimized(&window);
        }

        let outputs = self.core.workspace_manager.active_workspace_mut().outputs_for_window(&window);
        self.core.toplevel_created::<Self>(&window, outputs, parent.as_ref());

        self.x11_update_window_allowed_actions(&window);
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        let location = surface.geometry().loc;
        if let Some(xw) = self.core.xwayland.as_ref() {
            let _ = xw.init_new_window_event_mask(surface.window_id());
        }
        surface
            .user_data()
            .insert_if_missing(|| X11ClientId(surface.window_id() & self.core.xwayland.as_ref().unwrap().client_resource_mask()));
        let window = WindowElement::new(Window::new_x11_window(surface), self.core.next_window_id(), &self.core.config);
        self.x11_update_window_gtk_frame_extents(&window);
        self.new_window(window, location, true, None);
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let target_id = window.window_id();
        for workspace in self.core.workspace_manager.workspaces_mut() {
            let maybe = workspace
                .visible_windows()
                .find(|e| matches!(e.0.x11_surface(), Some(s) if s.window_id() == target_id))
                .cloned();
            if let Some(elem) = maybe {
                // FIXME: is this what we really want?
                self.set_window_minimized(&elem);
                break;
            }
        }
        if !window.is_override_redirect() {
            window.set_mapped(false).unwrap();
        }
    }

    fn destroyed_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        let target_id = surface.window_id();
        let found = self
            .core
            .workspace_manager
            .find_window(|elem| matches!(elem.0.underlying_surface(), WindowSurface::X11(s) if s.window_id() == target_id));
        if let Some(window) = found {
            window.handle_destroyed();
            self.remove_window(&window);
            self.core.toplevel_destroyed(&window);

            if let Some(xw) = self.core.xwayland.as_ref() {
                let client_mask = xw.client_resource_mask();
                let surface_client_id = surface.window_id() & client_mask;
                let has_remaining = self.core.workspace_manager.workspaces().iter().any(|workspace| {
                    workspace.all_windows().any(
                        |w| matches!(w.0.underlying_surface(), WindowSurface::X11(s) if s.window_id() & client_mask == surface_client_id),
                    )
                });
                if !has_remaining {
                    self.core.clients_with_windows.remove(&WindowClient::X11(surface_client_id));
                }
            }
        }
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        let surface_geometry = surface.geometry();

        let location = if (surface.is_override_redirect()
            || surface
                .window_type()
                .is_some_and(|ty| !matches!(ty, WmWindowType::Normal | WmWindowType::Dialog)))
            && (x.is_some() || y.is_some())
            && let Some((window, _, workspace)) = self
                .core
                .workspace_manager
                .find_window_and_workspace_mut(|elem| elem.0.x11_surface() == Some(&surface))
            && let Some(location) = workspace.window_location(&window)
        {
            // Allow these sorts of windows to set their own position.
            let location = (x.unwrap_or(location.x), y.unwrap_or(location.y)).into();
            self.core.workspace_manager.relocate_window(&window, location, false);
            location
        } else {
            // Other kinds of windows don't get to move around freely.
            surface_geometry.loc
        };

        let configure_geometry = Rectangle::new(
            location,
            (
                w.unwrap_or(surface_geometry.size.w as u32) as i32,
                h.unwrap_or(surface_geometry.size.h as u32) as i32,
            )
                .into(),
        );
        let _ = surface.configure(configure_geometry);
    }

    fn configure_notify(&mut self, _xwm: XwmId, window: X11Surface, geometry: Rectangle<i32, Logical>, _above: Option<u32>) {
        if let Some(elem) = self
            .core
            .workspace_manager
            .find_window(|elem| matches!(elem.0.x11_surface(), Some(w) if w == &window))
        {
            // `geometry.loc` is the X11 rect origin.  For CSD X11 windows the Space
            // position represents the visible-content origin (so smithay's
            // `render_location = Space - geometry().loc` cancels the extents offset
            // back out), so shift inward by the stored extents before relocating.
            let mut new_loc = geometry.loc;
            if let Some(x11_props) = elem.x11_props() {
                new_loc.x += x11_props.client_frame_left as i32;
                new_loc.y += x11_props.client_frame_top as i32;
            }
            self.core.workspace_manager.relocate_window(&elem, new_loc, false);
            // TODO: We don't properly handle the order of override-redirect windows here,
            //       they are always mapped top and then never reordered.
        }
    }

    fn minimize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(window) = self
            .core
            .workspace_manager
            .find_window(|e| matches!(e.0.x11_surface(), Some(w) if w == &window))
        {
            self.set_window_minimized(&window);
        }
    }

    fn maximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(wl_surface) = surface.wl_surface()
            && let Some(window) = self.window_for_surface(&wl_surface)
        {
            self.set_window_maximized(&window, None);
        }
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(wl_surface) = surface.wl_surface()
            && let Some(window) = self.window_for_surface(&wl_surface)
        {
            self.set_window_unmaximized(&window, None);
        }
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(elem) = self
            .core
            .workspace_manager
            .active_workspace()
            .find_window(|e| matches!(e.0.x11_surface(), Some(w) if w == &window))
        {
            self.set_window_fullscreen(&elem, None);
        }
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        // This is kinda dumb, but keeps the borrow checker happy
        if let Some(window) = self
            .core
            .workspace_manager
            .find_window(|e| matches!(e.0.x11_surface(), Some(w) if w == &window))
        {
            self.set_window_unfullscreen(&window);
        }
    }

    fn above_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(wl_surface) = surface.wl_surface()
            && let Some(window) = self.window_for_surface(&wl_surface)
        {
            self.set_window_always_on_top(&window);
        }
    }

    fn unabove_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(wl_surface) = surface.wl_surface()
            && let Some(window) = self.window_for_surface(&wl_surface)
        {
            self.set_window_normal_stacking(&window);
        }
    }

    fn below_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(wl_surface) = surface.wl_surface()
            && let Some(window) = self.window_for_surface(&wl_surface)
        {
            self.set_window_always_on_bottom(&window);
        }
    }

    fn unbelow_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(wl_surface) = surface.wl_surface()
            && let Some(window) = self.window_for_surface(&wl_surface)
        {
            self.set_window_normal_stacking(&window);
        }
    }

    fn shade_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(wl_surface) = surface.wl_surface()
            && let Some(window) = self.window_for_surface(&wl_surface)
        {
            self.set_window_shaded(&window, true);
        }
    }

    fn unshade_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(wl_surface) = surface.wl_surface()
            && let Some(window) = self.window_for_surface(&wl_surface)
        {
            self.set_window_shaded(&window, false);
        }
    }

    fn stick_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(wl_surface) = surface.wl_surface()
            && let Some(window) = self.window_for_surface(&wl_surface)
        {
            self.set_window_sticky(&window, true);
        }
    }

    fn unstick_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(wl_surface) = surface.wl_surface()
            && let Some(window) = self.window_for_surface(&wl_surface)
        {
            self.set_window_sticky(&window, false);
        }
    }

    fn demands_attention_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(wl_surface) = surface.wl_surface()
            && let Some(window) = self.window_for_surface(&wl_surface)
            && !window.active()
        {
            self.set_window_urgent_state(&window, true);
        }
    }

    fn undemands_attention_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(wl_surface) = surface.wl_surface()
            && let Some(window) = self.window_for_surface(&wl_surface)
        {
            self.set_window_urgent_state(&window, false);
        }
    }

    fn resize_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32, edges: X11ResizeEdge) {
        if let Some(wl_surface) = window.wl_surface()
            && let Some(window) = self.window_for_surface(&wl_surface)
        {
            self.start_window_resize(
                window,
                self.core.seat.clone(),
                SERIAL_COUNTER.next_serial(),
                edges.into(),
                GrabTrigger::Pointer,
            );
        }
    }

    fn move_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32) {
        if let Some(wl_surface) = window.wl_surface()
            && let Some(window) = self.window_for_surface(&wl_surface)
        {
            self.start_window_move(window, self.core.seat.clone(), SERIAL_COUNTER.next_serial(), GrabTrigger::Pointer);
        }
    }

    fn active_window_request(&mut self, _xwm: XwmId, surface: X11Surface, _timestamp: u32, currently_active_window: Option<X11Surface>) {
        // Smithay doesn't expose the 'source' field of the _NET_ACTIVE_WINDOW message, which
        // can tell us if the source was a pager application.  An X11 WM might unconditionally
        // allow the activation request for pagers, but that field can of course be spoofed.  Here
        // we are going to deviate from that behavior, because on Wayland, it would be surprising
        // if the pager app was an X11 app and not a Wayland app using foreign-toplevel-management
        // to do the activation.  (And if we did have an X11 pager app, it would only be able to
        // see other X11 windows, and not be that useful anyway.)

        let currently_active_window = currently_active_window.and_then(|caw| {
            self.core
                .workspace_manager
                .find_window(|elem| elem.0.x11_surface().is_some_and(|surf| *surf == caw))
        });

        if let Some(wl_surface) = surface.wl_surface()
            && let Some((window, _, workspace)) = self
                .core
                .workspace_manager
                .find_window_and_workspace_mut(|elem| elem.0.wl_surface().is_some_and(|surf| surf.as_ref() == &wl_surface))
        {
            if currently_active_window.is_some_and(|caw| window.same_application_as(&caw)) {
                self.activate_window(&window, self.core.config.raise_on_focus(), false, None);
            } else if self.core.config.prevent_focus_stealing()
                && (window
                    .last_user_interaction()
                    .is_none_or(|lui| lui < self.core.last_user_interaction)
                    || self.core.config.activate_action() == ActivateAction::None)
            {
                if let Some(topmost_window) = workspace.visible_windows().last().cloned() {
                    workspace.lower_window_below(&window, &topmost_window);
                } else {
                    workspace.raise_window(&window, false);
                }
            } else {
                self.set_window_urgent_state(&window, true);
                let current_focus = self.core.seat.get_keyboard().and_then(|keyboard| keyboard.current_focus());

                if current_focus != Some(window.clone().into()) {
                    self.activate_window(&window, self.core.config.raise_on_focus(), false, None);
                }
            }
        }
    }

    fn allow_selection_access(&mut self, xwm: XwmId, _selection: SelectionTarget) -> bool {
        if let Some(keyboard) = self.core.seat.get_keyboard() {
            // check that an X11 window is focused
            if let Some(KeyboardFocusTarget::Window(w)) = keyboard.current_focus()
                && let Some(surface) = w.x11_surface()
                && surface.xwm_id().as_ref().is_some_and(|id| id == &xwm)
            {
                return true;
            }
        }
        false
    }

    fn send_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_type: String, fd: OwnedFd) {
        match selection {
            SelectionTarget::Clipboard => {
                if let Err(err) = request_data_device_client_selection(&self.core.seat, mime_type, fd) {
                    error!(?err, "Failed to request current wayland clipboard for Xwayland",);
                }
            }
            SelectionTarget::Primary => {
                if let Err(err) = request_primary_client_selection(&self.core.seat, mime_type, fd) {
                    error!(?err, "Failed to request current wayland primary selection for Xwayland",);
                }
            }
        }
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        trace!(?selection, ?mime_types, "Got Selection from X11",);
        // TODO check, that focused windows is X11 window before doing this
        match selection {
            SelectionTarget::Clipboard => set_data_device_selection(&self.core.display_handle, &self.core.seat, mime_types, ()),
            SelectionTarget::Primary => set_primary_selection(&self.core.display_handle, &self.core.seat, mime_types, ()),
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        match selection {
            SelectionTarget::Clipboard => {
                if current_data_device_selection_userdata(&self.core.seat).is_some() {
                    clear_data_device_selection(&self.core.display_handle, &self.core.seat)
                }
            }
            SelectionTarget::Primary => {
                if current_primary_selection_userdata(&self.core.seat).is_some() {
                    clear_primary_selection(&self.core.display_handle, &self.core.seat)
                }
            }
        }
    }

    fn show_desktop_request(&mut self, _xwm: XwmId) {
        self.activate_show_desktop();
    }

    fn unshow_desktop_request(&mut self, _xwm: XwmId) {
        self.deactivate_show_desktop();
    }

    fn property_notify(&mut self, _xwm: XwmId, surface: X11Surface, property: WmWindowProperty) {
        if let Some(window) = surface.wl_surface().and_then(|surf| self.window_for_surface(&surf)) {
            match property {
                WmWindowProperty::Title => self.core.toplevel_changed(
                    &window,
                    Some(&surface.title()),
                    None,
                    WindowState::empty(),
                    WindowState::empty(),
                    Vec::new(),
                    Vec::new(),
                    None,
                ),
                WmWindowProperty::Class => self.core.toplevel_changed(
                    &window,
                    None,
                    Some(&surface.class()),
                    WindowState::empty(),
                    WindowState::empty(),
                    Vec::new(),
                    Vec::new(),
                    None,
                ),
                WmWindowProperty::TransientFor => {
                    if let Some(workspace) = self.core.workspace_manager.workspace_for_window_mut(&window) {
                        let parent = surface.is_transient_for().and_then(|window_id| {
                            workspace.find_window(|elem| matches!(elem.0.underlying_surface(), WindowSurface::X11(surface) if surface.window_id() == window_id))
                        });

                        self.set_window_parent(&window, parent.clone());

                        self.core.toplevel_changed(
                            &window,
                            None,
                            None,
                            WindowState::empty(),
                            WindowState::empty(),
                            Vec::new(),
                            Vec::new(),
                            Some(parent.as_ref()),
                        );
                    }
                }
                WmWindowProperty::Hints => {
                    let urgent = surface.hints().map(|hints| hints.urgent);
                    self.set_window_urgent_state(&window, urgent.unwrap_or(false));
                }
                // TODO: need to manually add a property notify for _NET_WM_STATE
                _ => (),
            }
        }
    }

    fn disconnected(&mut self, _xwm: XwmId) {
        if let Some((display_number, override_xwayland_scale)) = self.xwayland_destroyed()
            && self.core.is_running
        {
            self.maybe_schedule_xwayland_restart(display_number, override_xwayland_scale);
        }
    }
}

impl<BackendData: Backend + 'static> XWaylandKeyboardGrabHandler for Xfwl4State<BackendData> {
    fn keyboard_focus_for_xsurface(&self, surface: &WlSurface) -> Option<KeyboardFocusTarget> {
        let elem = self
            .core
            .workspace_manager
            .active_workspace()
            .visible_windows()
            .find(|elem: &&WindowElement| elem.wl_surface().as_deref() == Some(surface))?;
        Some(KeyboardFocusTarget::Window(elem.0.clone()))
    }
}

delegate_xwayland_keyboard_grab!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);

impl<BackendData: Backend> Xfwl4State<BackendData> {
    pub(in crate::core) fn window_icon_for_x11_window(&self, x11_surface: &X11Surface) -> Option<ImageData> {
        // TODO: check WmHints for icon as well
        self.core
            .xwayland
            .as_ref()
            .and_then(|xw| xw.get_net_wm_icon(x11_surface.window_id()))
    }
}

impl WindowElement {
    pub(in crate::core) fn x11_props(&self) -> Option<MutexGuard<'_, X11WindowPropsInner>> {
        self.0
            .x11_surface()
            .map(|surface| surface.user_data().get_or_insert(X11WindowProps::default))
            .map(|x11_props| x11_props.0.lock().unwrap())
    }

    /// Given a rect in visible-content coordinates, return the rect in X11-window coordinates
    /// by shifting the origin outward and growing the size by the stored `_GTK_FRAME_EXTENTS`.
    /// No-op for non-X11 windows, and for X11 windows without the hint set.
    pub(in crate::core) fn grow_rect_by_gtk_frame_extents(&self, rect: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        if let Some(x11_props) = self.x11_props() {
            let left = x11_props.client_frame_left as i32;
            let right = x11_props.client_frame_right as i32;
            let top = x11_props.client_frame_top as i32;
            let bottom = x11_props.client_frame_bottom as i32;
            Rectangle::new(
                (rect.loc.x - left, rect.loc.y - top).into(),
                (rect.size.w + left + right, rect.size.h + top + bottom).into(),
            )
        } else {
            rect
        }
    }
}
