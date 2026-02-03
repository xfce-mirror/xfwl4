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
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use glib::Sender;
use smithay::{
    backend::renderer::element::{RenderElementStates, default_primary_scanout_output_compare, utils::select_dmabuf_feedback},
    desktop::{
        PopupManager, Space,
        utils::{
            OutputPresentationFeedback, surface_presentation_feedback_flags_from_states, surface_primary_scanout_output,
            update_surface_primary_scanout_output, with_surfaces_surface_tree,
        },
    },
    input::{
        Seat, SeatState,
        keyboard::{Keysym, XkbConfig},
        pointer::{CursorImageStatus, PointerHandle},
    },
    output::Output,
    reexports::{
        calloop::{Interest, LoopHandle, LoopSignal, Mode, PostAction, channel, generic::Generic},
        rustix,
        wayland_server::{
            Client, Display, DisplayHandle, Resource,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Clock, Monotonic, Point, Time},
    wayland::{
        commit_timing::{CommitTimerBarrierStateUserData, CommitTimingManagerState},
        compositor::{CompositorClientState, CompositorHandler, CompositorState},
        dmabuf::DmabufFeedback,
        fifo::{FifoBarrierCachedState, FifoManagerState},
        fixes::FixesState,
        fractional_scale::{FractionalScaleManagerState, with_fractional_scale},
        idle_inhibit::IdleInhibitManagerState,
        idle_notify::IdleNotifierState,
        input_method::InputMethodManagerState,
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitState,
        output::OutputManagerState,
        pointer_constraints::PointerConstraintsState,
        pointer_gestures::PointerGesturesState,
        presentation::PresentationState,
        relative_pointer::RelativePointerManagerState,
        security_context::{SecurityContext, SecurityContextState},
        selection::{data_device::DataDeviceState, primary_selection::PrimarySelectionState, wlr_data_control::DataControlState},
        shell::{wlr_layer::WlrLayerShellState, xdg::XdgShellState},
        shm::ShmState,
        single_pixel_buffer::SinglePixelBufferState,
        socket::ListeningSocketSource,
        tablet_manager::TabletManagerState,
        text_input::TextInputManagerState,
        viewporter::ViewporterState,
        virtual_keyboard::VirtualKeyboardManagerState,
        xdg_activation::XdgActivationState,
        xdg_foreign::XdgForeignState,
    },
};
#[cfg(feature = "xwayland")]
use smithay::{
    utils::Size,
    wayland::xwayland_keyboard_grab::XWaylandKeyboardGrabState,
    wayland::xwayland_shell,
    xwayland::{X11Wm, XWayland, XWaylandEvent},
};
use tracing::{error, info, warn};

#[cfg(feature = "xwayland")]
use crate::cursor::Cursor;
use crate::{
    backend::Backend,
    config::{DEFAULT_KEY_REPEAT_DELAY, DEFAULT_KEY_REPEAT_RATE, KeyboardConfig, Xfwl4Config},
    handlers::{DecorationState, data_device::DndIcon},
    protocols::wlr_gamma_control::WlrGammaControlState,
    shell::WindowElement,
    ui::{FromUiMessage, ToUiMessage},
    workspaces::WorkspaceManager,
};

#[derive(Debug, Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
    pub security_context: Option<SecurityContext>,
}

impl ClientData for ClientState {
    /// Notification that a client was initialized
    fn initialized(&self, _client_id: ClientId) {}
    /// Notification that a client is disconnected
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

pub struct Xfwl4State<BackendData: Backend + 'static> {
    pub backend_data: BackendData,
    pub socket_name: Option<String>,
    pub display_handle: DisplayHandle,
    pub stop_signal: LoopSignal,
    pub handle: LoopHandle<'static, Xfwl4State<BackendData>>,

    pub config: Xfwl4Config,

    // desktop
    pub workspace_manager: WorkspaceManager<BackendData>,
    pub popups: PopupManager,
    pub pending_windows: HashMap<WlSurface, WindowElement>,

    // UI thread communication
    pub to_ui_channel_tx: Sender<ToUiMessage>,
    pub ui_thread_client: Option<Client>,
    pub cycling_windows: bool,

    // smithay state
    pub compositor_state: CompositorState,
    pub data_device_state: DataDeviceState,
    pub layer_shell_state: WlrLayerShellState,
    pub output_manager_state: OutputManagerState,
    pub wlr_gamma_control_state: WlrGammaControlState<Self>,
    pub primary_selection_state: PrimarySelectionState,
    pub data_control_state: DataControlState,
    pub seat_state: SeatState<Xfwl4State<BackendData>>,
    pub keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
    pub shm_state: ShmState,
    pub viewporter_state: ViewporterState,
    pub xdg_activation_state: XdgActivationState,
    pub xdg_shell_state: XdgShellState,
    pub decoration_state: DecorationState,
    pub presentation_state: PresentationState,
    pub fractional_scale_manager_state: FractionalScaleManagerState,
    pub xdg_foreign_state: XdgForeignState,
    #[cfg(feature = "xwayland")]
    pub xwayland_shell_state: xwayland_shell::XWaylandShellState,
    pub single_pixel_buffer_state: SinglePixelBufferState,
    pub fifo_manager_state: FifoManagerState,
    pub commit_timing_manager_state: CommitTimingManagerState,
    pub ext_idle_notifier_state: IdleNotifierState<Self>,
    pub idle_inhibit_surfaces: HashSet<WlSurface>,

    pub dnd_icon: Option<DndIcon>,

    // input-related fields
    pub suppressed_keys: Vec<Keysym>,
    pub cursor_status: CursorImageStatus,
    pub seat_name: String,
    pub seat: Seat<Xfwl4State<BackendData>>,
    pub keyboard_config: KeyboardConfig<Self>,
    pub clock: Clock<Monotonic>,
    pub pointer: PointerHandle<Xfwl4State<BackendData>>,

    #[cfg(feature = "xwayland")]
    pub xwm: Option<X11Wm>,
    #[cfg(feature = "xwayland")]
    pub xdisplay: Option<u32>,
    #[cfg(feature = "xwayland")]
    pub x11conn: Option<(x11rb::rust_connection::RustConnection, usize)>,

    #[cfg(feature = "debug")]
    pub renderdoc: Option<renderdoc::RenderDoc<renderdoc::V141>>,

    pub show_window_preview: bool,
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub fn init(
        display: Display<Xfwl4State<BackendData>>,
        handle: LoopHandle<'static, Xfwl4State<BackendData>>,
        stop_signal: LoopSignal,
        backend_data: BackendData,
        from_ui_channel_rx: channel::Channel<FromUiMessage>,
        to_ui_channel_tx: Sender<ToUiMessage>,
        listen_on_socket: bool,
    ) -> Xfwl4State<BackendData> {
        let dh = display.handle();

        let clock = Clock::new();

        // init wayland clients
        let socket_name = if listen_on_socket {
            let source = ListeningSocketSource::new_auto().unwrap();
            let socket_name = source.socket_name().to_string_lossy().into_owned();
            handle
                .insert_source(source, |client_stream, _, state| {
                    match state.display_handle.insert_client(client_stream, Arc::new(ClientState::default())) {
                        Ok(client) => {
                            match client.get_credentials(&state.display_handle) {
                                Ok(creds) => {
                                    let my_pid = rustix::process::getpid();
                                    if creds.pid == my_pid.as_raw_pid() {
                                        // This is our UI thread connecting back to us.
                                        tracing::debug!("UI thread connected");
                                        state.ui_thread_client = Some(client);
                                    }
                                }
                                Err(err) => warn!("Failed to get credentials for new client: {err}"),
                            }
                        }
                        Err(err) => warn!("Error adding wayland client: {err}"),
                    };
                })
                .expect("Failed to init wayland socket source");
            info!(name = socket_name, "Listening on wayland socket");
            Some(socket_name)
        } else {
            None
        };
        handle
            .insert_source(Generic::new(display, Interest::READ, Mode::Level), |_, display, data| {
                profiling::scope!("dispatch_clients");
                // Safety: we don't drop the display
                unsafe {
                    display.get_mut().dispatch_clients(data).unwrap();
                }
                Ok(PostAction::Continue)
            })
            .expect("Failed to init wayland server source");

        let config = Xfwl4Config::new(handle.clone());

        // UI thread
        handle
            .insert_source(from_ui_channel_rx, |event, _, state| {
                if let channel::Event::Msg(message) = event
                    && let Err(err) = state.handle_ui_thread_message(message)
                {
                    warn!("Failed to handle UI thread message: {err}");
                }
            })
            .unwrap();

        // init globals
        let compositor_state = CompositorState::new::<Self>(&dh);
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let wlr_gamma_control_state = WlrGammaControlState::<Self>::new(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);
        let data_control_state = DataControlState::new::<Self, _>(&dh, Some(&primary_selection_state), |_| true);
        let mut seat_state = SeatState::new();
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let viewporter_state = ViewporterState::new::<Self>(&dh);
        let xdg_activation_state = XdgActivationState::new::<Self>(&dh);
        let decoration_state = DecorationState::new::<BackendData>(&dh, handle.clone());
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let presentation_state = PresentationState::new::<Self>(&dh, clock.id() as u32);
        let fractional_scale_manager_state = FractionalScaleManagerState::new::<Self>(&dh);
        let xdg_foreign_state = XdgForeignState::new::<Self>(&dh);
        let single_pixel_buffer_state = SinglePixelBufferState::new::<Self>(&dh);
        let fifo_manager_state = FifoManagerState::new::<Self>(&dh);
        let commit_timing_manager_state = CommitTimingManagerState::new::<Self>(&dh);
        TextInputManagerState::new::<Self>(&dh);
        InputMethodManagerState::new::<Self, _>(&dh, |_client| true);
        VirtualKeyboardManagerState::new::<Self, _>(&dh, |_client| true);
        // Expose global only if backend supports relative motion events
        if BackendData::HAS_RELATIVE_MOTION {
            RelativePointerManagerState::new::<Self>(&dh);
        }
        PointerConstraintsState::new::<Self>(&dh);
        if BackendData::HAS_GESTURES {
            PointerGesturesState::new::<Self>(&dh);
        }
        TabletManagerState::new::<Self>(&dh);
        SecurityContextState::new::<Self, _>(&dh, |client| {
            client
                .get_data::<ClientState>()
                .is_none_or(|client_state| client_state.security_context.is_none())
        });
        FixesState::new::<Self>(&dh);

        // init input
        let seat_name = backend_data.seat_name();
        let mut seat = seat_state.new_wl_seat(&dh, seat_name.clone());

        let pointer = seat.add_pointer();

        let keyboard_handle = seat
            .add_keyboard(XkbConfig::default(), DEFAULT_KEY_REPEAT_DELAY, DEFAULT_KEY_REPEAT_RATE)
            .expect("Failed to initialize the keyboard");
        let (keyboard_config, notifier) = KeyboardConfig::new(keyboard_handle.clone());
        handle
            .insert_source(notifier, |event, _, state| {
                if let channel::Event::Msg(xkb_config_owned) = event
                    && let Some(keyboard_handle) = state.seat.get_keyboard()
                {
                    let xkb_config = XkbConfig {
                        rules: "",
                        model: &xkb_config_owned.model.unwrap_or("".to_owned()),
                        layout: &xkb_config_owned.layout.unwrap_or("".to_owned()),
                        variant: &xkb_config_owned.variant.unwrap_or("".to_owned()),
                        options: xkb_config_owned.options,
                    };
                    tracing::debug!(
                        "Updating XKB config, model={}, layout={}, variant={}, options={}",
                        xkb_config.model,
                        xkb_config.layout,
                        xkb_config.variant,
                        xkb_config.options.as_ref().unwrap_or(&"".to_owned())
                    );
                    if let Err(err) = keyboard_handle.set_xkb_config(state, xkb_config) {
                        error!("Failed to set keyboard XKB config: {err}");
                    }
                }
            })
            .unwrap();

        let keyboard_shortcuts_inhibit_state = KeyboardShortcutsInhibitState::new::<Self>(&dh);

        let ext_idle_notifier_state = IdleNotifierState::new(&dh, handle.clone());
        IdleInhibitManagerState::new::<Self>(&dh);

        #[cfg(feature = "xwayland")]
        let xwayland_shell_state = xwayland_shell::XWaylandShellState::new::<Self>(&dh.clone());

        #[cfg(feature = "xwayland")]
        XWaylandKeyboardGrabState::new::<Self>(&dh.clone());

        let workspace_manager = WorkspaceManager::new(&dh, &handle);

        Xfwl4State {
            backend_data,
            display_handle: dh,
            socket_name,
            stop_signal,
            handle,
            config,
            workspace_manager,
            popups: PopupManager::default(),
            pending_windows: HashMap::new(),
            to_ui_channel_tx,
            ui_thread_client: None,
            cycling_windows: false,
            compositor_state,
            data_device_state,
            layer_shell_state,
            output_manager_state,
            wlr_gamma_control_state,
            primary_selection_state,
            data_control_state,
            seat_state,
            keyboard_shortcuts_inhibit_state,
            shm_state,
            viewporter_state,
            xdg_activation_state,
            xdg_shell_state,
            decoration_state,
            presentation_state,
            fractional_scale_manager_state,
            xdg_foreign_state,
            single_pixel_buffer_state,
            fifo_manager_state,
            commit_timing_manager_state,
            ext_idle_notifier_state,
            idle_inhibit_surfaces: HashSet::new(),

            dnd_icon: None,
            suppressed_keys: Vec::new(),
            cursor_status: CursorImageStatus::default_named(),
            seat_name,
            seat,
            keyboard_config,
            pointer,
            clock,

            #[cfg(feature = "xwayland")]
            xwayland_shell_state,
            #[cfg(feature = "xwayland")]
            xwm: None,
            #[cfg(feature = "xwayland")]
            xdisplay: None,
            #[cfg(feature = "xwayland")]
            x11conn: None,
            #[cfg(feature = "debug")]
            renderdoc: renderdoc::RenderDoc::new().ok(),
            show_window_preview: false,
        }
    }

    #[cfg(feature = "xwayland")]
    pub fn start_xwayland(&mut self, xwayland_scale: f64) -> anyhow::Result<u32> {
        use std::process::Stdio;

        use smithay::wayland::compositor::CompositorHandler;

        let (xwayland, client) = XWayland::spawn(
            &self.display_handle,
            None,
            std::iter::empty::<(String, String)>(),
            true,
            Stdio::null(),
            Stdio::null(),
            |_| (),
        )
        .expect("failed to start XWayland");

        let display_number = xwayland.display_number();

        let display_handle = self.display_handle.clone();
        let ret = self.handle.insert_source(xwayland, move |event, _, data| match event {
            XWaylandEvent::Ready {
                x11_socket,
                display_number,
            } => {
                data.client_compositor_state(&client).set_client_scale(xwayland_scale);
                let mut wm = X11Wm::start_wm(data.handle.clone(), &display_handle, x11_socket, client.clone())
                    .expect("Failed to attach X11 Window Manager");

                let cursor = Cursor::load();
                let image = cursor.get_image(1, Duration::ZERO);
                wm.set_cursor(
                    &image.pixels_rgba,
                    Size::from((image.width as u16, image.height as u16)),
                    Point::from((image.xhot as u16, image.yhot as u16)),
                )
                .expect("Failed to set xwayland default cursor");
                data.xwm = Some(wm);
                data.xdisplay = Some(display_number);
                data.x11conn = Some(x11rb::connect(Some(&format!(":{display_number}"))).unwrap())
            }
            XWaylandEvent::Error => {
                warn!("XWayland crashed on startup");
            }
        });
        if let Err(e) = ret {
            tracing::error!("Failed to insert the XWaylandSource into the event loop: {}", e);
        }

        Ok(display_number)
    }

    pub fn refresh_and_flush_clients(&mut self) {
        self.workspace_manager.refresh_spaces();
        self.popups.cleanup();

        if let Err(err) = self.display_handle.flush_clients() {
            error!("Fatal error: Failed to flush Wayland clients: {err}");
            std::process::exit(1);
        }
    }

    pub fn shutdown(&self) {
        self.stop_signal.stop();
        self.stop_signal.wakeup();
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub fn pre_repaint(&mut self, output: &Output, frame_target: impl Into<Time<Monotonic>>) {
        let frame_target = frame_target.into();

        #[allow(clippy::mutable_key_type)]
        let mut clients: HashMap<ClientId, Client> = HashMap::new();
        let workspace = self.workspace_manager.active_workspace();
        workspace.space().elements().for_each(|window| {
            window.with_surfaces(|surface, states| {
                if let Some(mut commit_timer_state) = states
                    .data_map
                    .get::<CommitTimerBarrierStateUserData>()
                    .map(|commit_timer| commit_timer.lock().unwrap())
                {
                    commit_timer_state.signal_until(frame_target);
                    let client = surface.client().unwrap();
                    clients.insert(client.id(), client);
                }
            });
        });

        let map = smithay::desktop::layer_map_for_output(output);
        for layer_surface in map.layers() {
            layer_surface.with_surfaces(|surface, states| {
                if let Some(mut commit_timer_state) = states
                    .data_map
                    .get::<CommitTimerBarrierStateUserData>()
                    .map(|commit_timer| commit_timer.lock().unwrap())
                {
                    commit_timer_state.signal_until(frame_target);
                    let client = surface.client().unwrap();
                    clients.insert(client.id(), client);
                }
            });
        }
        // Drop the lock to the layer map before calling blocker_cleared, which might end up
        // calling the commit handler which in turn again could access the layer map.
        std::mem::drop(map);

        if let CursorImageStatus::Surface(ref surface) = self.cursor_status {
            with_surfaces_surface_tree(surface, |surface, states| {
                if let Some(mut commit_timer_state) = states
                    .data_map
                    .get::<CommitTimerBarrierStateUserData>()
                    .map(|commit_timer| commit_timer.lock().unwrap())
                {
                    commit_timer_state.signal_until(frame_target);
                    let client = surface.client().unwrap();
                    clients.insert(client.id(), client);
                }
            });
        }

        if let Some(surface) = self.dnd_icon.as_ref().map(|icon| &icon.surface) {
            with_surfaces_surface_tree(surface, |surface, states| {
                if let Some(mut commit_timer_state) = states
                    .data_map
                    .get::<CommitTimerBarrierStateUserData>()
                    .map(|commit_timer| commit_timer.lock().unwrap())
                {
                    commit_timer_state.signal_until(frame_target);
                    let client = surface.client().unwrap();
                    clients.insert(client.id(), client);
                }
            });
        }

        let dh = self.display_handle.clone();
        for client in clients.into_values() {
            self.client_compositor_state(&client).blocker_cleared(self, &dh);
        }
    }

    pub fn post_repaint(
        &mut self,
        output: &Output,
        time: impl Into<Duration>,
        dmabuf_feedback: Option<SurfaceDmabufFeedback>,
        render_element_states: &RenderElementStates,
    ) {
        let time = time.into();
        // XXX: this was originally set to 1 second, which caused stuttering and lagginess on the
        // winit and X11 backends (but not the udev backend).  Setting to 16ms seems to fix the
        // problem on winit and X11, and so far seems to show no ill effects for udev.
        let throttle = Some(Duration::from_millis(16));

        #[allow(clippy::mutable_key_type)]
        let mut clients: HashMap<ClientId, Client> = HashMap::new();

        let workspace = self.workspace_manager.active_workspace();
        let space = workspace.space();
        space.elements().for_each(|window| {
            window.with_surfaces(|surface, states| {
                let primary_scanout_output = surface_primary_scanout_output(surface, states);

                if let Some(output) = primary_scanout_output.as_ref() {
                    with_fractional_scale(states, |fraction_scale| {
                        fraction_scale.set_preferred_scale(output.current_scale().fractional_scale());
                    });
                }

                if primary_scanout_output.as_ref().map(|o| o == output).unwrap_or(true) {
                    let fifo_barrier = states.cached_state.get::<FifoBarrierCachedState>().current().barrier.take();

                    if let Some(fifo_barrier) = fifo_barrier {
                        fifo_barrier.signal();
                        let client = surface.client().unwrap();
                        clients.insert(client.id(), client);
                    }
                }
            });

            if space.outputs_for_element(window).contains(output) {
                window.send_frame(output, time, throttle, surface_primary_scanout_output);
                if let Some(dmabuf_feedback) = dmabuf_feedback.as_ref() {
                    window.send_dmabuf_feedback(output, surface_primary_scanout_output, |surface, _| {
                        select_dmabuf_feedback(
                            surface,
                            render_element_states,
                            &dmabuf_feedback.render_feedback,
                            &dmabuf_feedback.scanout_feedback,
                        )
                    });
                }
            }
        });
        let map = smithay::desktop::layer_map_for_output(output);
        for layer_surface in map.layers() {
            layer_surface.with_surfaces(|surface, states| {
                let primary_scanout_output = surface_primary_scanout_output(surface, states);

                if let Some(output) = primary_scanout_output.as_ref() {
                    with_fractional_scale(states, |fraction_scale| {
                        fraction_scale.set_preferred_scale(output.current_scale().fractional_scale());
                    });
                }

                if primary_scanout_output.as_ref().map(|o| o == output).unwrap_or(true) {
                    let fifo_barrier = states.cached_state.get::<FifoBarrierCachedState>().current().barrier.take();

                    if let Some(fifo_barrier) = fifo_barrier {
                        fifo_barrier.signal();
                        let client = surface.client().unwrap();
                        clients.insert(client.id(), client);
                    }
                }
            });

            layer_surface.send_frame(output, time, throttle, surface_primary_scanout_output);
            if let Some(dmabuf_feedback) = dmabuf_feedback.as_ref() {
                layer_surface.send_dmabuf_feedback(output, surface_primary_scanout_output, |surface, _| {
                    select_dmabuf_feedback(
                        surface,
                        render_element_states,
                        &dmabuf_feedback.render_feedback,
                        &dmabuf_feedback.scanout_feedback,
                    )
                });
            }
        }
        // Drop the lock to the layer map before calling blocker_cleared, which might end up
        // calling the commit handler which in turn again could access the layer map.
        std::mem::drop(map);

        if let CursorImageStatus::Surface(ref surface) = self.cursor_status {
            with_surfaces_surface_tree(surface, |surface, states| {
                let primary_scanout_output = surface_primary_scanout_output(surface, states);

                if let Some(output) = primary_scanout_output.as_ref() {
                    with_fractional_scale(states, |fraction_scale| {
                        fraction_scale.set_preferred_scale(output.current_scale().fractional_scale());
                    });
                }

                if primary_scanout_output.as_ref().map(|o| o == output).unwrap_or(true) {
                    let fifo_barrier = states.cached_state.get::<FifoBarrierCachedState>().current().barrier.take();

                    if let Some(fifo_barrier) = fifo_barrier {
                        fifo_barrier.signal();
                        let client = surface.client().unwrap();
                        clients.insert(client.id(), client);
                    }
                }
            });
        }

        if let Some(surface) = self.dnd_icon.as_ref().map(|icon| &icon.surface) {
            with_surfaces_surface_tree(surface, |surface, states| {
                let primary_scanout_output = surface_primary_scanout_output(surface, states);

                if let Some(output) = primary_scanout_output.as_ref() {
                    with_fractional_scale(states, |fraction_scale| {
                        fraction_scale.set_preferred_scale(output.current_scale().fractional_scale());
                    });
                }

                if primary_scanout_output.as_ref().map(|o| o == output).unwrap_or(true) {
                    let fifo_barrier = states.cached_state.get::<FifoBarrierCachedState>().current().barrier.take();

                    if let Some(fifo_barrier) = fifo_barrier {
                        fifo_barrier.signal();
                        let client = surface.client().unwrap();
                        clients.insert(client.id(), client);
                    }
                }
            });
        }

        let dh = self.display_handle.clone();
        for client in clients.into_values() {
            self.client_compositor_state(&client).blocker_cleared(self, &dh);
        }
    }
}

pub fn update_primary_scanout_output(
    space: &Space<WindowElement>,
    output: &Output,
    dnd_icon: &Option<DndIcon>,
    cursor_status: &CursorImageStatus,
    render_element_states: &RenderElementStates,
) {
    space.elements().for_each(|window| {
        window.with_surfaces(|surface, states| {
            update_surface_primary_scanout_output(
                surface,
                output,
                states,
                render_element_states,
                default_primary_scanout_output_compare,
            );
        });
    });
    let map = smithay::desktop::layer_map_for_output(output);
    for layer_surface in map.layers() {
        layer_surface.with_surfaces(|surface, states| {
            update_surface_primary_scanout_output(
                surface,
                output,
                states,
                render_element_states,
                default_primary_scanout_output_compare,
            );
        });
    }

    if let CursorImageStatus::Surface(surface) = cursor_status {
        with_surfaces_surface_tree(surface, |surface, states| {
            update_surface_primary_scanout_output(
                surface,
                output,
                states,
                render_element_states,
                default_primary_scanout_output_compare,
            );
        });
    }

    if let Some(surface) = dnd_icon.as_ref().map(|icon| &icon.surface) {
        with_surfaces_surface_tree(surface, |surface, states| {
            update_surface_primary_scanout_output(
                surface,
                output,
                states,
                render_element_states,
                default_primary_scanout_output_compare,
            );
        });
    }
}

#[derive(Debug, Clone)]
pub struct SurfaceDmabufFeedback {
    pub render_feedback: DmabufFeedback,
    pub scanout_feedback: DmabufFeedback,
}

#[profiling::function]
pub fn take_presentation_feedback(
    output: &Output,
    space: &Space<WindowElement>,
    render_element_states: &RenderElementStates,
) -> OutputPresentationFeedback {
    let mut output_presentation_feedback = OutputPresentationFeedback::new(output);

    space.elements().for_each(|window| {
        if space.outputs_for_element(window).contains(output) {
            window.take_presentation_feedback(&mut output_presentation_feedback, surface_primary_scanout_output, |surface, _| {
                surface_presentation_feedback_flags_from_states(surface, render_element_states)
            });
        }
    });
    let map = smithay::desktop::layer_map_for_output(output);
    for layer_surface in map.layers() {
        layer_surface.take_presentation_feedback(&mut output_presentation_feedback, surface_primary_scanout_output, |surface, _| {
            surface_presentation_feedback_flags_from_states(surface, render_element_states)
        });
    }

    output_presentation_feedback
}
