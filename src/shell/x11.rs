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

use std::os::unix::io::OwnedFd;

use smithay::{
    delegate_xwayland_keyboard_grab, delegate_xwayland_shell,
    desktop::Window,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Rectangle, SERIAL_COUNTER},
    wayland::{
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
        xwm::{Reorder, ResizeEdge as X11ResizeEdge, XwmId},
    },
};
use tracing::{error, trace};

use crate::{Xfwl4State, backend::Backend, focus::KeyboardFocusTarget, shell::GrabTrigger, util::ImageData};

use super::{FullscreenSurface, WindowElement, place_new_window};

impl<BackendData: Backend> XWaylandShellHandler for Xfwl4State<BackendData> {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }
}

delegate_xwayland_shell!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);

impl<BackendData: Backend> XwmHandler for Xfwl4State<BackendData> {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm.as_mut().unwrap()
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}
    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        window.set_mapped(true).unwrap();
        let window = WindowElement(Window::new_x11_window(window));
        let workspace = self.workspace_manager.active_workspace_mut();
        place_new_window(workspace, self.pointer.current_location(), &window, true);
        let bbox = workspace.element_bbox(&window).unwrap();
        let Some(xsurface) = window.0.x11_surface() else { unreachable!() };
        xsurface.configure(Some(bbox)).unwrap();
        if !xsurface.is_decorated() {
            self.enable_decorations_for_window(&window);
        } else {
            window.disable_decorations();
        }
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let location = window.geometry().loc;
        let window = WindowElement(Window::new_x11_window(window));
        self.workspace_manager.active_workspace_mut().map_element(window, location, true);
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        for workspace in self.workspace_manager.workspaces_mut() {
            let maybe = workspace
                .elements()
                .find(|e| matches!(e.0.x11_surface(), Some(w) if w == &window))
                .cloned();
            if let Some(elem) = maybe {
                workspace.unmap_elem(&elem);
                break;
            }
        }
        if !window.is_override_redirect() {
            window.set_mapped(false).unwrap();
        }
    }

    fn destroyed_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        _x: Option<i32>,
        _y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        // we just set the new size, but don't let windows move themselves around freely
        let mut geo = window.geometry();
        if let Some(w) = w {
            geo.size.w = w as i32;
        }
        if let Some(h) = h {
            geo.size.h = h as i32;
        }
        let _ = window.configure(geo);
    }

    fn configure_notify(&mut self, _xwm: XwmId, window: X11Surface, geometry: Rectangle<i32, Logical>, _above: Option<u32>) {
        let workspace = self.workspace_manager.active_workspace_mut();
        let elem = workspace
            .elements()
            .find(|e| matches!(e.0.x11_surface(), Some(w) if w == &window))
            .cloned();
        if let Some(elem) = elem {
            workspace.map_element(elem, geometry.loc, false);
            // TODO: We don't properly handle the order of override-redirect windows here,
            //       they are always mapped top and then never reordered.
        }
    }

    fn minimize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(window) = self
            .workspace_manager
            .find_element(|e| matches!(e.0.x11_surface(), Some(w) if w == &window))
        {
            self.workspace_manager.set_window_minimized(&window);
        }
    }

    fn maximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(wl_surface) = surface.wl_surface()
            && let Some(window) = self.window_for_surface(&wl_surface)
        {
            self.set_window_maximized(&window, true);
        }
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(wl_surface) = surface.wl_surface()
            && let Some(window) = self.window_for_surface(&wl_surface)
        {
            self.set_window_maximized(&window, false);
        }
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let workspace = self.workspace_manager.active_workspace();
        if let Some(elem) = workspace.elements().find(|e| matches!(e.0.x11_surface(), Some(w) if w == &window)) {
            let outputs_for_window = workspace.outputs_for_element(elem);
            let output = outputs_for_window
                .first()
                .or_else(|| workspace.outputs().next())
                .expect("No outputs found");
            let geometry = workspace.output_geometry(output).unwrap();

            window.set_fullscreen(true).unwrap();
            elem.disable_decorations();
            window.configure(geometry).unwrap();
            output.user_data().insert_if_missing(FullscreenSurface::default);
            output.user_data().get::<FullscreenSurface>().unwrap().set(elem.clone());
            trace!("Fullscreening: {:?}", elem);
        }
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        // This is kinda dumb, but keeps the borrow checker happy
        if let Some(elem) = self
            .workspace_manager
            .find_element(|e| matches!(e.0.x11_surface(), Some(w) if w == &window))
        {
            window.set_fullscreen(false).unwrap();
            if !window.is_decorated() {
                self.enable_decorations_for_window(&elem);
            } else {
                elem.disable_decorations();
            }

            for workspace in self.workspace_manager.workspaces_mut() {
                if let Some(elem) = workspace.elements().find(|e| matches!(e.0.x11_surface(), Some(w) if w == &window)) {
                    if let Some(output) = workspace.outputs().find(|o| {
                        o.user_data()
                            .get::<FullscreenSurface>()
                            .and_then(|f| f.get())
                            .map(|w| &w == elem)
                            .unwrap_or(false)
                    }) {
                        trace!("Unfullscreening: {:?}", elem);
                        output.user_data().get::<FullscreenSurface>().unwrap().clear();
                        window.configure(workspace.element_bbox(elem)).unwrap();
                        self.backend_data.reset_buffers(output);
                    }
                    break;
                }
            }
        }
    }

    fn resize_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32, edges: X11ResizeEdge) {
        if let Some(wl_surface) = window.wl_surface()
            && let Some(window) = self.window_for_surface(&wl_surface)
        {
            self.start_window_resize(
                window,
                self.seat.clone(),
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
            self.start_window_move(window, self.seat.clone(), SERIAL_COUNTER.next_serial(), GrabTrigger::Pointer);
        }
    }

    fn allow_selection_access(&mut self, xwm: XwmId, _selection: SelectionTarget) -> bool {
        if let Some(keyboard) = self.seat.get_keyboard() {
            // check that an X11 window is focused
            if let Some(KeyboardFocusTarget::Window(w)) = keyboard.current_focus()
                && let Some(surface) = w.x11_surface()
                && surface.xwm_id().unwrap() == xwm
            {
                return true;
            }
        }
        false
    }

    fn send_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_type: String, fd: OwnedFd) {
        match selection {
            SelectionTarget::Clipboard => {
                if let Err(err) = request_data_device_client_selection(&self.seat, mime_type, fd) {
                    error!(?err, "Failed to request current wayland clipboard for Xwayland",);
                }
            }
            SelectionTarget::Primary => {
                if let Err(err) = request_primary_client_selection(&self.seat, mime_type, fd) {
                    error!(?err, "Failed to request current wayland primary selection for Xwayland",);
                }
            }
        }
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        trace!(?selection, ?mime_types, "Got Selection from X11",);
        // TODO check, that focused windows is X11 window before doing this
        match selection {
            SelectionTarget::Clipboard => set_data_device_selection(&self.display_handle, &self.seat, mime_types, ()),
            SelectionTarget::Primary => set_primary_selection(&self.display_handle, &self.seat, mime_types, ()),
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        match selection {
            SelectionTarget::Clipboard => {
                if current_data_device_selection_userdata(&self.seat).is_some() {
                    clear_data_device_selection(&self.display_handle, &self.seat)
                }
            }
            SelectionTarget::Primary => {
                if current_primary_selection_userdata(&self.seat).is_some() {
                    clear_primary_selection(&self.display_handle, &self.seat)
                }
            }
        }
    }

    fn disconnected(&mut self, _xwm: XwmId) {
        self.xwm = None;
    }
}

impl<BackendData: Backend + 'static> XWaylandKeyboardGrabHandler for Xfwl4State<BackendData> {
    fn keyboard_focus_for_xsurface(&self, surface: &WlSurface) -> Option<KeyboardFocusTarget> {
        let elem = self
            .workspace_manager
            .active_workspace()
            .elements()
            .find(|elem| elem.wl_surface().as_deref() == Some(surface))?;
        Some(KeyboardFocusTarget::Window(elem.0.clone()))
    }
}

delegate_xwayland_keyboard_grab!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);

impl<BackendData: Backend> Xfwl4State<BackendData> {
    pub fn window_icon_for_x11_window(&self, x11_surface: &X11Surface) -> Option<ImageData> {
        // TODO: check WmHints for icon as well
        self.x11conn
            .as_ref()
            .and_then(|(x11conn, _)| crate::util::x11_net_wm_icon_to_image_data(x11conn, x11_surface.window_id()).ok())
    }
}
