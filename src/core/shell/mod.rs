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

use std::{cell::RefCell, collections::HashMap, sync::Mutex, time::Duration};

use gettextrs::gettext;
#[cfg(feature = "xwayland")]
use smithay::desktop::WindowSurface;
#[cfg(feature = "udev")]
use smithay::wayland::drm_syncobj::DrmSyncobjCachedState;

use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    desktop::{PopupKind, PopupManager, space::SpaceElement},
    input::pointer::{CursorImageStatus, CursorImageSurfaceData},
    output::WeakOutput,
    reexports::{
        calloop::{
            Interest, RegistrationToken,
            timer::{TimeoutAction, Timer},
        },
        wayland_server::{
            Client, Resource,
            protocol::{wl_buffer::WlBuffer, wl_surface::WlSurface},
        },
    },
    utils::{IsAlive, Logical, Monotonic, Rectangle, Time},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes, TraversalAction, add_blocker,
            add_pre_commit_hook, get_parent, is_sync_subsurface, with_states, with_surface_tree_upward,
        },
        dmabuf::get_dmabuf,
        idle_inhibit::IdleInhibitHandler,
        shell::{
            wlr_layer::WlrLayerShellState,
            xdg::{XdgShellState, XdgToplevelSurfaceData, dialog::XdgDialogState},
        },
    },
};
use tr::tr;

use crate::{
    backend::Backend,
    core::{
        placement::FillMode,
        state::{ClientState, Xfwl4Core, Xfwl4State},
    },
    protocols::{foreign_toplevel_management::ToplevelChangedInput, xfwl4_compositor_ui::DialogId},
    util::icon::IconSource,
};

mod element;
mod element_impls;
mod grabs;
mod layer;
mod layout;
pub(crate) mod ssd;
#[cfg(feature = "xwayland")]
mod x11;
pub(crate) mod xdg;

pub use self::element::*;
pub use self::grabs::*;
pub use self::layout::*;

const MAX_URGENT_BLINK_ITERATIONS: u32 = 10;
const URGENT_BLINK_TIMEOUT: Duration = Duration::from_millis(500);
const WINDOW_PING_TIMEOUT: Duration = Duration::from_secs(3);

pub struct ShellState {
    compositor_state: CompositorState,
    layer_shell_state: WlrLayerShellState,
    _xdg_dialog_state: XdgDialogState,
    xdg_shell_state: XdgShellState,
    #[cfg(feature = "xwayland")]
    xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState,

    popup_manager: PopupManager,
    pending_windows: HashMap<WlSurface, WindowElement>,

    not_responding_dialogs: Vec<(DialogId, WindowElement)>,
}

impl ShellState {
    pub fn new(
        compositor_state: CompositorState,
        layer_shell_state: WlrLayerShellState,
        xdg_dialog_state: XdgDialogState,
        xdg_shell_state: XdgShellState,
        #[cfg(feature = "xwayland")] xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState,
    ) -> Self {
        Self {
            compositor_state,
            layer_shell_state,
            _xdg_dialog_state: xdg_dialog_state,
            xdg_shell_state,
            #[cfg(feature = "xwayland")]
            xwayland_shell_state,
            popup_manager: PopupManager::default(),
            pending_windows: HashMap::new(),
            not_responding_dialogs: Vec::new(),
        }
    }

    pub(in crate::core) fn popup_manager_mut(&mut self) -> &mut PopupManager {
        &mut self.popup_manager
    }

    #[inline]
    pub(super) fn layer_surfaces(&self) -> impl DoubleEndedIterator<Item = smithay::wayland::shell::wlr_layer::LayerSurface> {
        self.layer_shell_state.layer_surfaces()
    }

    pub(in crate::core) fn dialog_destroyed(&mut self, dialog_id: DialogId) {
        self.not_responding_dialogs.retain(|(id, _)| *id != dialog_id);
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct WindowState: u16 {
        const ACTIVATED = (1 << 0);
        const MINIMIZED = (1 << 1);
        const MAXIMIZED = (1 << 2);
        const SHADED = (1 << 3);
        const STICKY = (1 << 4);
        const FULLSCREEN = (1 << 5);
        const SKIP_TASKBAR = (1 << 6);
        const SKIP_PAGER = (1 << 7);
        const KEEP_ABOVE = (1 << 8);
        const KEEP_BELOW = (1 << 9);
        const DEMANDS_ATTENTION = (1 << 10);
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct WindowFlags: u8 {
        const NO_CYCLE = (1 << 0);
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct WindowCapabilities: u16 {
        const CLOSE = (1 << 0);
        const MINIMIZE = (1 << 1);
        const MAXIMIZE = (1 << 2);
        const FULLSCREEN = (1 << 3);
        const MOVE = (1 << 4);
        const RESIZE = (1 << 5);
        const SHADE = (1 << 6);
        const STICK = (1 << 7);
        const CHANGE_WORKSPACE = (1 << 8);
        const ABOVE = (1 << 9);
        const BELOW = (1 << 10);
        const WINDOW_MENU = (1 << 11);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WorkspaceLocation {
    Single(u32),
    All,
}

impl Default for WorkspaceLocation {
    fn default() -> Self {
        Self::Single(0)
    }
}

#[derive(Debug, Default)]
pub struct DemandsAttentionState {
    pub demands_attention: bool,
    pub token: Option<RegistrationToken>,
    pub iterations: u32,
}

#[derive(Debug, Default)]
pub struct WindowPropsInner {
    pub flags: WindowFlags,
    pub saved_geom: Option<Rectangle<i32, Logical>>,
    pub anchored_output: Option<WeakOutput>,
    pub tile_mode: Option<TileMode>,
    pub workspace_loc: WorkspaceLocation,
    pub is_minimized: bool,
    pub maximized_mode: Option<FillMode>,
    pub is_fullscreened: bool,
    pub is_shaded: bool,
    pub is_opacity_locked: bool,
    pub hide_titlebar_when_maximized: bool,
    pub toplevel_icon_state_hash: u64,
    pub window_icon: IconSource,
    pub urgent: DemandsAttentionState,
    pub last_user_interaction: Option<Time<Monotonic>>,
    pub last_capabilities: Option<WindowCapabilities>,
    pub last_titlebar_buttons: Option<WindowCapabilities>,
    pub last_bbox: Option<Rectangle<i32, Logical>>,
    pub was_shown_before_show_desktop: bool,
}

#[derive(Debug, Default)]
pub struct WindowProps(pub Mutex<WindowPropsInner>);

impl<BackendData: Backend> BufferHandler for Xfwl4State<BackendData> {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl<BackendData: Backend> CompositorHandler for Xfwl4State<BackendData> {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.core.shell_state.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        #[cfg(feature = "xwayland")]
        if let Some(state) = client.get_data::<smithay::xwayland::XWaylandClientData>() {
            return &state.compositor_state;
        }

        if let Some(state) = client.get_data::<ClientState>() {
            &state.compositor_state
        } else {
            panic!("Unknown client data type");
        }
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
                    let res = state.core.loop_handle.insert_source(source, move |_, _, data| {
                        let dh = data.core.display_handle.clone();
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
                    let res = state.core.loop_handle.insert_source(source, move |_, _, data| {
                        let dh = data.core.display_handle.clone();
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
        self.backend.early_import(surface);

        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for_surface(&root) {
                window.0.on_commit();

                // A client that resizes itself, or that only now acked the configure giving it
                // decorations, moves its surface out from under the coordinates the pointer was
                // last told about.
                let bbox = SpaceElement::bbox(&window);
                let bbox_changed = {
                    let mut props = window.props();
                    let changed = props.last_bbox != Some(bbox);
                    props.last_bbox = Some(bbox);
                    changed
                };
                if bbox_changed {
                    self.core.set_pointer_focus_dirty();
                }

                if &root == surface {
                    let buffer_offset = with_states(surface, |states| {
                        states.cached_state.get::<SurfaceAttributes>().current().buffer_delta.take()
                    });

                    if let Some(buffer_offset) = buffer_offset {
                        let workspace = self.core.workspace_manager.active_workspace_mut();
                        let current_loc = workspace.window_location(&window).unwrap();
                        self.core.workspace_manager.relocate_window(&window, current_loc + buffer_offset);
                    }
                }
            }
        }
        self.core.shell_state.popup_manager.commit(surface);

        if matches!(&self.core.cursor_state.pointer_element().status(), CursorImageStatus::Surface(cursor_surface) if cursor_surface == surface)
        {
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

        if let Some(dnd_icon) = self.core.cursor_state.dnd_icon_mut()
            && &dnd_icon.surface == surface
        {
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
        self.core.shell_state.pending_windows.retain(|a_surface, _| surface != a_surface);

        if let Some(window) = self.window_for_surface(surface) {
            match window.0.underlying_surface() {
                WindowSurface::Wayland(_) => {
                    self.remove_window(&window);
                    self.core.toplevel_destroyed(&window);
                }
                #[cfg(feature = "xwayland")]
                WindowSurface::X11(_) => {
                    // An X11 window's wl_surface is torn down on X11 unmap, but the window is not
                    // actually destroyed at this point, and the client may choose to map it again.
                    // So do nothing here, and let the XwmHandler lifecycle events do what needs to
                    // be done.
                }
            }
        }
    }
}

#[derive(Default)]
pub struct SurfaceData {
    pub geometry: Option<Rectangle<i32, Logical>>,
    pub resize_state: ResizeState,
}

impl<BackendData: Backend> Xfwl4State<BackendData> {
    // Advertises what may be done to a window over whichever protocol it speaks, so callers do not
    // have to know which that is.
    pub(in crate::core) fn update_window_capabilities(&self, window: &WindowElement) {
        let capabilities = window.capabilities();
        let titlebar_buttons = window.titlebar_buttons();

        // Called on every toplevel commit, so that a client changing its size hints is picked up;
        // only the transitions are worth acting on.  The buttons can change on their own, when a
        // client asks for a different set of decorations without changing what it permits.
        let changed = {
            let mut props = window.props();
            let caps_changed = props.last_capabilities.replace(capabilities) != Some(capabilities);
            let buttons_changed = props.last_titlebar_buttons.replace(titlebar_buttons) != Some(titlebar_buttons);
            caps_changed || buttons_changed
        };

        if changed {
            if let Some(window_decorations) = window.decoration_state_mut().window_decorations_mut() {
                window_decorations.update(ssd::DecorationInput::TitlebarButtons(titlebar_buttons));
            }

            match window.0.underlying_surface() {
                WindowSurface::Wayland(toplevel) => {
                    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;

                    let wm_capabilities = [
                        (WindowCapabilities::WINDOW_MENU, xdg_toplevel::WmCapabilities::WindowMenu),
                        (WindowCapabilities::MAXIMIZE, xdg_toplevel::WmCapabilities::Maximize),
                        (WindowCapabilities::FULLSCREEN, xdg_toplevel::WmCapabilities::Fullscreen),
                        (WindowCapabilities::MINIMIZE, xdg_toplevel::WmCapabilities::Minimize),
                    ]
                    .into_iter()
                    .filter_map(|(capability, wm_capability)| capabilities.contains(capability).then_some(wm_capability));

                    toplevel.with_pending_state(|state| state.capabilities.replace(wm_capabilities));

                    if toplevel.is_initial_configure_sent() {
                        toplevel.send_pending_configure();
                    }
                }

                #[cfg(feature = "xwayland")]
                WindowSurface::X11(_) => self.x11_update_window_allowed_actions(window),
            }
        }
    }

    pub(in crate::core) fn window_for_surface(&self, surface: &WlSurface) -> Option<WindowElement> {
        self.core
            .workspace_manager
            .find_window(|window| window.wl_surface().map(|s| &*s == surface).unwrap_or(false))
            .or_else(|| self.core.shell_state.pending_windows.get(surface).cloned())
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

        if let Some(popup) = self.core.shell_state.popup_manager.find_popup(surface) {
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

        self.ensure_layer_initial_configure(surface);
    }

    pub fn set_window_urgent_state(&mut self, window: &WindowElement, is_urgent: bool) {
        let mut props = window.props();
        let was_urgent = props.urgent.demands_attention;
        if is_urgent != was_urgent {
            if !is_urgent {
                props.urgent.demands_attention = false;

                if let Some(token) = props.urgent.token.take() {
                    self.core.loop_handle.remove(token);
                }

                if let Some(decorations) = window.decoration_state_mut().window_decorations_mut() {
                    decorations.disable_titlebar_blink();
                }
            } else {
                props.urgent.demands_attention = true;
                props.urgent.iterations = 0;

                if self.core.config.urgent_blink() && !window.active() {
                    props.urgent.token = self
                        .core
                        .loop_handle
                        .insert_source(Timer::from_duration(URGENT_BLINK_TIMEOUT), {
                            let window = window.clone();

                            move |_, _, state| {
                                let mut props = window.props();
                                if window.alive()
                                    && (props.urgent.iterations < MAX_URGENT_BLINK_ITERATIONS || state.core.config.repeat_urgent_blink())
                                {
                                    if props.urgent.iterations < MAX_URGENT_BLINK_ITERATIONS {
                                        props.urgent.iterations += 1;
                                    } else {
                                        props.urgent.iterations = 0;
                                    }

                                    if let Some(decorations) = window.decoration_state_mut().window_decorations_mut() {
                                        decorations.toggle_titlebar_blink_state();
                                    }

                                    TimeoutAction::ToDuration(URGENT_BLINK_TIMEOUT)
                                } else {
                                    if let Some(decorations) = window.decoration_state_mut().window_decorations_mut() {
                                        decorations.disable_titlebar_blink();
                                    }
                                    props.urgent.token = None;
                                    TimeoutAction::Drop
                                }
                            }
                        })
                        .inspect_err(|err| tracing::warn!("Failed to register urgent blink timeout with event loop: {err}"))
                        .ok();
                }
            }

            #[cfg(feature = "xwayland")]
            if let WindowSurface::X11(x11_surface) = window.0.underlying_surface() {
                let _ = x11_surface.set_demands_attention(props.urgent.demands_attention);
            }
        }

        let changed = was_urgent != props.urgent.demands_attention;
        drop(props);

        if changed {
            self.core.toplevel_changed(
                window,
                ToplevelChangedInput {
                    state: Some(window.state()),
                    ..Default::default()
                },
            );
        }
    }
}

impl<BackendData: Backend + 'static> Xfwl4Core<BackendData> {
    fn ping_window(&self, window: &WindowElement) {
        match window.0.underlying_surface() {
            WindowSurface::Wayland(toplevel_surface) => self.xdg_send_client_ping(toplevel_surface.client(), window),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x11_surface) => self.x11_ping_window(window, x11_surface),
        }
    }

    fn show_window_unresponsive_dialog(&mut self, window: &WindowElement) {
        let is_alive = match window.0.underlying_surface() {
            WindowSurface::Wayland(surface) => surface.client().alive(),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(_) => window.0.alive(),
        };

        if is_alive {
            let title = window.title().unwrap_or_else(|| "".to_owned());

            match self.compositor_ui_state.show_dialog::<Xfwl4State<BackendData>, _, _, _>(
                gettext("Application Unresponsive"),
                Some(tr!("Window \"{0}\" might be busy and is not responding.", title)),
                Some(gettext("Do you want to terminate the application?")),
                Some("dialog-warning"),
                gettext("No"),
                "cancel",
                [(gettext("Terminate"), "accept")].into_iter(),
            ) {
                Err(err) => tracing::warn!("Failed to create app-unresponsive dialog: {err}"),
                Ok(dialog_id) => self.shell_state.not_responding_dialogs.push((dialog_id, window.clone())),
            }
        }
    }

    pub(in crate::core) fn kill_client_for_dialog(&mut self, dialog_id: DialogId) {
        if let Some(pos) = self.shell_state.not_responding_dialogs.iter().position(|(id, _)| *id == dialog_id) {
            let (_, window) = self.shell_state.not_responding_dialogs.remove(pos);
            match window.0.underlying_surface() {
                WindowSurface::Wayland(toplevel_surface) => {
                    let _ = toplevel_surface.client().unresponsive();
                }
                #[cfg(feature = "xwayland")]
                WindowSurface::X11(surface) => self.x11_kill_client_by_surface(surface),
            }
        }
    }

    pub(in crate::core) fn close_dialog_for_window(&mut self, window: &WindowElement) {
        if let Some(pos) = self
            .shell_state
            .not_responding_dialogs
            .iter()
            .position(|(_, dialog_window)| dialog_window == window)
        {
            let (dialog_id, _) = self.shell_state.not_responding_dialogs.remove(pos);
            self.compositor_ui_state.cancel_dialog(dialog_id);
        }

        match window.0.underlying_surface() {
            WindowSurface::Wayland(surface) => self.xdg_clear_ping_timeout(surface),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(surface) => self.x11_clear_ping_timeout(surface),
        }
    }
}
