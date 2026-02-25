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

use std::{cell::RefCell, path::PathBuf, sync::Mutex};

use indexmap::Equivalent;
#[cfg(feature = "xwayland")]
use smithay::xwayland::XWaylandClientData;

#[cfg(feature = "udev")]
use smithay::wayland::drm_syncobj::DrmSyncobjCachedState;

use smithay::{
    backend::renderer::utils::{Buffer, on_commit_buffer_handler},
    delegate_compositor, delegate_layer_shell,
    desktop::{LayerSurface, PopupKind, Space, WindowSurfaceType, layer_map_for_output, space::SpaceElement},
    input::pointer::{CursorImageStatus, CursorImageSurfaceData},
    output::Output,
    reexports::{
        calloop::Interest,
        wayland_server::{
            Client, Resource,
            protocol::{wl_buffer::WlBuffer, wl_output, wl_surface::WlSurface},
        },
    },
    utils::{Logical, Point, Rectangle, Size},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes, TraversalAction, add_blocker,
            add_pre_commit_hook, get_parent, is_sync_subsurface, with_states, with_surface_tree_upward,
        },
        dmabuf::get_dmabuf,
        idle_inhibit::IdleInhibitHandler,
        shell::{
            wlr_layer::{Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData, WlrLayerShellHandler, WlrLayerShellState},
            xdg::{PopupSurface, XdgToplevelSurfaceData},
        },
        xdg_toplevel_icon::ToplevelIconCachedState,
    },
};

use crate::{ClientState, backend::Backend, state::Xfwl4State, workspaces::WorkspaceManager};

mod element;
mod element_impls;
mod grabs;
pub(crate) mod ssd;
#[cfg(feature = "xwayland")]
mod x11;
pub(crate) mod xdg;

pub use self::element::*;
pub use self::grabs::*;

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct WindowState: u8 {
        const ACTIVATED = (1 << 0);
        const MINIMIZED = (1 << 1);
        const MAXIMIZED = (1 << 2);
        const SHADED = (1 << 3);
        const STICKY = (1 << 4);
        const FULLSCREEN = (1 << 5);
    }
}

#[derive(Debug, Default)]
pub struct XdgToplevelIconState {
    icon_name: Option<String>,
    buffers: Vec<(WlBuffer, i32)>,
}

impl Equivalent<ToplevelIconCachedState> for XdgToplevelIconState {
    fn equivalent(&self, key: &ToplevelIconCachedState) -> bool {
        self.icon_name.as_deref() == key.icon_name() && self.buffers.as_slice() == key.buffers()
    }
}

#[derive(Debug, Default)]
pub struct WindowPropsInner {
    pub pre_maximize_geom: Option<Rectangle<i32, Logical>>,
    pub is_shaded: bool,
    pub last_seen_xdg_icon_state: Option<XdgToplevelIconState>,
    pub window_icon: Option<WindowIcon>,
}

#[derive(Debug, Default)]
pub struct WindowProps(pub Mutex<WindowPropsInner>);

#[derive(Debug, Clone)]
pub enum WindowIcon {
    Named(String),
    File(PathBuf),
    Buffer(Buffer),
}

impl WindowIcon {
    fn name(&self) -> Option<&str> {
        match self {
            Self::Named(name) => Some(name.as_str()),
            _ => None,
        }
    }

    fn path(&self) -> Option<&PathBuf> {
        match self {
            Self::File(path) => Some(path),
            _ => None,
        }
    }

    fn buffer(&self) -> Option<&Buffer> {
        match self {
            Self::Buffer(buffer) => Some(buffer),
            _ => None,
        }
    }
}

impl PartialEq for WindowIcon {
    fn eq(&self, other: &Self) -> bool {
        match self {
            WindowIcon::Named(name) => other.name().is_some_and(|other| name == other),
            WindowIcon::File(path) => other.path().is_some_and(|other| path == other),
            WindowIcon::Buffer(buffer) => other.buffer().is_some_and(|other| (*buffer).id() == (*other).id()),
        }
    }
}

impl<BackendData: Backend> BufferHandler for Xfwl4State<BackendData> {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl<BackendData: Backend> CompositorHandler for Xfwl4State<BackendData> {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        #[cfg(feature = "xwayland")]
        if let Some(state) = client.get_data::<XWaylandClientData>() {
            return &state.compositor_state;
        }
        if let Some(state) = client.get_data::<ClientState>() {
            return &state.compositor_state;
        }
        panic!("Unknown client data type")
    }

    fn new_surface(&mut self, surface: &WlSurface) {
        add_pre_commit_hook::<Self, _>(surface, move |state, _dh, surface| {
            #[cfg(feature = "udev")]
            let mut acquire_point = None;
            let maybe_dmabuf = with_states(surface, |surface_data| {
                #[cfg(feature = "udev")]
                acquire_point.clone_from(&surface_data.cached_state.get::<DrmSyncobjCachedState>().pending().acquire_point);
                surface_data
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .pending()
                    .buffer
                    .as_ref()
                    .and_then(|assignment| match assignment {
                        BufferAssignment::NewBuffer(buffer) => get_dmabuf(buffer).cloned().ok(),
                        _ => None,
                    })
            });
            if let Some(dmabuf) = maybe_dmabuf {
                #[cfg(feature = "udev")]
                if let Some(acquire_point) = acquire_point
                    && let Ok((blocker, source)) = acquire_point.generate_blocker()
                {
                    let client = surface.client().unwrap();
                    let res = state.handle.insert_source(source, move |_, _, data| {
                        let dh = data.display_handle.clone();
                        data.client_compositor_state(&client).blocker_cleared(data, &dh);
                        Ok(())
                    });
                    if res.is_ok() {
                        add_blocker(surface, blocker);
                        return;
                    }
                }
                if let Ok((blocker, source)) = dmabuf.generate_blocker(Interest::READ)
                    && let Some(client) = surface.client()
                {
                    let res = state.handle.insert_source(source, move |_, _, data| {
                        let dh = data.display_handle.clone();
                        data.client_compositor_state(&client).blocker_cleared(data, &dh);
                        Ok(())
                    });
                    if res.is_ok() {
                        add_blocker(surface, blocker);
                    }
                }
            }
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        self.backend_data.early_import(surface);

        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for_surface(&root) {
                window.0.on_commit();

                if &root == surface {
                    let buffer_offset = with_states(surface, |states| {
                        states.cached_state.get::<SurfaceAttributes>().current().buffer_delta.take()
                    });

                    if let Some(buffer_offset) = buffer_offset {
                        let workspace = self.workspace_manager.active_workspace_mut();
                        let current_loc = workspace.element_location(&window).unwrap();
                        workspace.map_element(window, current_loc + buffer_offset, false);
                    }
                }
            }
        }
        self.popups.commit(surface);

        if matches!(&self.cursor_status, CursorImageStatus::Surface(cursor_surface) if cursor_surface == surface) {
            with_states(surface, |states| {
                let cursor_image_attributes = states.data_map.get::<CursorImageSurfaceData>();

                if let Some(mut cursor_image_attributes) = cursor_image_attributes.map(|attrs| attrs.lock().unwrap()) {
                    let buffer_delta = states.cached_state.get::<SurfaceAttributes>().current().buffer_delta.take();
                    if let Some(buffer_delta) = buffer_delta {
                        tracing::trace!(hotspot = ?cursor_image_attributes.hotspot, ?buffer_delta, "decrementing cursor hotspot");
                        cursor_image_attributes.hotspot -= buffer_delta;
                    }
                }
            });
        }

        if matches!(&self.dnd_icon, Some(icon) if &icon.surface == surface) {
            let dnd_icon = self.dnd_icon.as_mut().unwrap();
            with_states(&dnd_icon.surface, |states| {
                let buffer_delta = states
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .current()
                    .buffer_delta
                    .take()
                    .unwrap_or_default();
                tracing::trace!(offset = ?dnd_icon.offset, ?buffer_delta, "moving dnd offset");
                dnd_icon.offset += buffer_delta;
            });
        }

        self.ensure_initial_configure(surface)
    }

    fn destroyed(&mut self, surface: &WlSurface) {
        self.uninhibit(surface.clone());
        self.pending_windows.retain(|a_surface, _| surface != a_surface);

        if let Some(window) = self.window_for_surface(surface) {
            for workspace in self.workspace_manager.workspaces_mut() {
                workspace.set_window_unfullscreen(&window);
                workspace.unmap_elem(&window);
            }
        }
    }
}

delegate_compositor!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);

impl<BackendData: Backend> WlrLayerShellHandler for Xfwl4State<BackendData> {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(&mut self, surface: WlrLayerSurface, wl_output: Option<wl_output::WlOutput>, _layer: Layer, namespace: String) {
        let output = wl_output
            .as_ref()
            .and_then(Output::from_resource)
            .unwrap_or_else(|| self.workspace_manager.active_workspace().outputs().next().unwrap().clone());
        let mut map = layer_map_for_output(&output);
        map.map_layer(&LayerSurface::new(surface, namespace)).unwrap();
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        if let Some((mut map, layer)) = self.workspace_manager.active_workspace().outputs().find_map(|o| {
            let map = layer_map_for_output(o);
            let layer = map.layers().find(|&layer| layer.layer_surface() == &surface).cloned();
            layer.map(|layer| (map, layer))
        }) {
            map.unmap_layer(&layer);
        }
    }

    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        self.unconstrain_popup(&popup);

        if let Err(err) = popup.send_configure() {
            tracing::warn!("Failed to send configure event to popup with layer-shell parent: {err}");
        } else if let Err(err) = self.popups.track_popup(PopupKind::from(popup)) {
            tracing::warn!("Failed to track popup with layer-shell parent: {err}");
        }
    }
}

delegate_layer_shell!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);

#[derive(Default)]
pub struct SurfaceData {
    pub geometry: Option<Rectangle<i32, Logical>>,
    pub resize_state: ResizeState,
}

impl<BackendData: Backend> Xfwl4State<BackendData> {
    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<WindowElement> {
        self.workspace_manager
            .find_element(|window| window.wl_surface().map(|s| &*s == surface).unwrap_or(false))
            .or_else(|| self.pending_windows.get(surface).cloned())
    }

    fn ensure_initial_configure(&mut self, surface: &WlSurface) {
        with_surface_tree_upward(
            surface,
            (),
            |_, _, _| TraversalAction::DoChildren(()),
            |_, states, _| {
                states.data_map.insert_if_missing(|| RefCell::new(SurfaceData::default()));
            },
            |_, _, _| true,
        );

        if let Some(window) = self.window_for_surface(surface) {
            // send the initial configure if relevant
            #[cfg_attr(not(feature = "xwayland"), allow(irrefutable_let_patterns))]
            if let Some(toplevel) = window.0.toplevel() {
                let initial_configure_sent = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .initial_configure_sent
                });
                if !initial_configure_sent {
                    toplevel.send_configure();
                }
            }

            #[cfg(feature = "xwayland")]
            if window.is_x11() {
                // For wayland windows, the post-commit hook will handler transitioning out of
                // resizing and into NotResizing, but X11 works differently because the X protocol
                // supports an atomic resize+move operation.
                with_states(surface, |states| {
                    let mut data = states.data_map.get::<RefCell<SurfaceData>>().unwrap().borrow_mut();
                    if let ResizeState::WaitingForCommit(_, _) = data.resize_state {
                        data.resize_state = ResizeState::NotResizing;
                    }
                });
            }

            return;
        }

        if let Some(popup) = self.popups.find_popup(surface) {
            let popup = match popup {
                PopupKind::Xdg(ref popup) => popup,
                // Doesn't require configure
                PopupKind::InputMethod(ref _input_popup) => {
                    return;
                }
            };

            if !popup.is_initial_configure_sent() {
                // NOTE: This should never fail as the initial configure is always
                // allowed.
                popup.send_configure().expect("initial configure failed");
            }

            return;
        };

        if let Some(output) = self.workspace_manager.active_workspace().outputs().find(|o| {
            let map = layer_map_for_output(o);
            map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).is_some()
        }) {
            let initial_configure_sent = with_states(surface, |states| {
                states
                    .data_map
                    .get::<LayerSurfaceData>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .initial_configure_sent
            });

            let mut map = layer_map_for_output(output);

            // arrange the layers before sending the initial configure
            // to respect any size the client may have sent
            map.arrange();
            // send the initial configure if relevant
            if !initial_configure_sent {
                let layer = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).unwrap();

                layer.layer_surface().send_configure();
            }
        };
    }
}

fn place_new_window(space: &mut Space<WindowElement>, pointer_location: Point<f64, Logical>, window: &WindowElement, activate: bool) {
    // place the window at a random location on same output as pointer
    // or if there is not output in a [0;800]x[0;800] square
    use rand::distributions::{Distribution, Uniform};

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

    // set the initial toplevel bounds
    #[allow(irrefutable_let_patterns)]
    if let Some(toplevel) = window.0.toplevel() {
        toplevel.with_pending_state(|state| {
            state.bounds = Some(output_geometry.size);
        });
    }

    let max_x = output_geometry.loc.x + (((output_geometry.size.w as f32) / 3.0) * 2.0) as i32;
    let max_y = output_geometry.loc.y + (((output_geometry.size.h as f32) / 3.0) * 2.0) as i32;
    let x_range = Uniform::new(output_geometry.loc.x, max_x);
    let y_range = Uniform::new(output_geometry.loc.y, max_y);
    let mut rng = rand::thread_rng();
    let x = x_range.sample(&mut rng);
    let y = y_range.sample(&mut rng);

    space.map_element(window.clone(), (x, y), activate);
}

pub fn fixup_positions<BackendData: Backend + 'static>(
    workspace_manager: &mut WorkspaceManager<BackendData>,
    pointer_location: Point<f64, Logical>,
) {
    // fixup outputs
    let outputs: Vec<_> = workspace_manager.active_workspace().space().outputs().cloned().collect();
    let mut offset = Point::<i32, Logical>::from((0, 0));
    for output in &outputs {
        let size = workspace_manager
            .active_workspace()
            .space()
            .output_geometry(output)
            .map(|geo| geo.size)
            .unwrap_or_else(|| Size::from((0, 0)));
        for workspace in workspace_manager.workspaces_mut() {
            workspace.space_mut().map_output(output, offset);
        }
        layer_map_for_output(output).arrange();
        offset.x += size.w;
    }

    // fixup windows
    for workspace in workspace_manager.workspaces_mut() {
        fixup_window_positions_on_space(workspace.space_mut(), pointer_location);
    }
}

fn fixup_window_positions_on_space(space: &mut Space<WindowElement>, pointer_location: Point<f64, Logical>) {
    let mut orphaned_windows = Vec::new();
    let outputs = space
        .outputs()
        .flat_map(|o| {
            let geo = space.output_geometry(o)?;
            let map = layer_map_for_output(o);
            let zone = map.non_exclusive_zone();
            Some(Rectangle::new(geo.loc + zone.loc, zone.size))
        })
        .collect::<Vec<_>>();
    for window in space.elements() {
        let window_location = match space.element_location(window) {
            Some(loc) => loc,
            None => continue,
        };
        let geo_loc = window.bbox().loc + window_location;

        if !outputs.iter().any(|o_geo| o_geo.contains(geo_loc)) {
            orphaned_windows.push(window.clone());
        }
    }
    for window in orphaned_windows.into_iter() {
        place_new_window(space, pointer_location, &window, false);
    }
}
