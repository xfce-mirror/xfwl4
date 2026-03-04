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

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::anyhow;
use glib::Sender;
use smithay::{
    backend::renderer::{Texture, element::memory::MemoryRenderBuffer},
    desktop::PopupManager,
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
            Client, Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Clock, Monotonic, Point},
    wayland::{
        commit_timing::CommitTimingManagerState,
        compositor::{CompositorClientState, CompositorState},
        fifo::FifoManagerState,
        fixes::FixesState,
        fractional_scale::FractionalScaleManagerState,
        idle_inhibit::IdleInhibitManagerState,
        idle_notify::IdleNotifierState,
        image_copy_capture::ImageCopyCaptureState,
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
        xdg_toplevel_icon::XdgToplevelIconManager,
    },
};
#[cfg(feature = "xwayland")]
use smithay::{
    utils::Size,
    wayland::{xwayland_keyboard_grab::XWaylandKeyboardGrabState, xwayland_shell},
    xwayland::{X11Wm, XWayland, XWaylandEvent},
};
use tracing::{error, info, warn};

use crate::{
    backend::{Backend, BackendType},
    core::{
        config::{DEFAULT_KEY_REPEAT_DELAY, DEFAULT_KEY_REPEAT_RATE, KeyboardConfig, OutputsConfig, Xfwl4Config},
        cursor::{Cursor, CursorName, CursorTheme},
        drawing::{
            PointerElement,
            decorations::{DecorBackgroundState, DecorButtonName, DecorButtonState, DecorationTheme},
        },
        handlers::{
            DecorationState, ExtImageCaptureSourceState, ExtSessionLockState, ForeignToplevelState, ProtocolDelegates, data_device::DndIcon,
        },
        shell::{ShellProtocolDelegates, WindowElement},
        util::{ClientExt, icon_theme::FreedesktopIconsIconTheme},
        workspaces::WorkspaceManager,
    },
    protocols::{wlr_output_management::WlrOutputManagementState, wlr_screencopy::WlrScreencopyState},
    ui::{FromUiMessage, PointerBehavior, ToUiMessage, tabwin::TabwinMode},
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
    pub(crate) core: Xfwl4Core<BackendData>,
    pub(crate) backend: BackendData,
}

pub struct Xfwl4Core<BackendData: Backend + 'static> {
    pub socket_name: Option<String>,
    pub display_handle: DisplayHandle,
    pub stop_signal: LoopSignal,
    pub handle: LoopHandle<'static, Xfwl4State<BackendData>>,

    pub config: Xfwl4Config,
    pub outputs_config: OutputsConfig,

    // desktop
    pub workspace_manager: WorkspaceManager<BackendData>,
    pub popups: PopupManager,
    pub pending_windows: HashMap<WlSurface, WindowElement>,
    pub decoration_theme: Option<DecorationTheme>,
    pub font_map: gtk::pango::FontMap,
    pub font_options: gtk::cairo::FontOptions,
    pub icon_theme: FreedesktopIconsIconTheme,
    pub cursor_theme: CursorTheme,
    pub pointer_behavior_settings: PointerBehavior,

    // UI thread communication
    pub to_ui_channel_tx: Sender<ToUiMessage>,
    pub ui_thread_client: Option<Client>,
    pub cycling_windows: bool,
    pub window_menu_anchor: Option<WindowElement>,

    // smithay state
    pub protocol_delegates: ProtocolDelegates<BackendData>,
    pub shell_protocol_delegates: ShellProtocolDelegates,
    pub xdg_toplevel_icon_manager: XdgToplevelIconManager,
    pub shm_state: ShmState,
    pub foreign_toplevel_state: ForeignToplevelState<BackendData>,
    pub wlr_output_management_state: WlrOutputManagementState,

    // rendering
    pub cursor_status: CursorImageStatus,
    pub pointer_image_cache: Vec<(xcursor::parser::Image, MemoryRenderBuffer)>,
    pub pointer_element: PointerElement,
    pub pointer_image: Cursor,
    pub dnd_icon: Option<DndIcon>,
    #[cfg(feature = "debug")]
    pub debug: Option<crate::core::debug::BackendDebug>,

    // input-related fields
    pub suppressed_keys: Vec<Keysym>,
    pub seat_name: String,
    pub seat: Seat<Xfwl4State<BackendData>>,
    pub keyboard_config: KeyboardConfig<Xfwl4State<BackendData>>,
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
                    match state
                        .core
                        .display_handle
                        .insert_client(client_stream, Arc::new(ClientState::default()))
                    {
                        Ok(client) => {
                            match client.get_credentials(&state.core.display_handle) {
                                Ok(creds) => {
                                    let my_pid = rustix::process::getpid();
                                    if creds.pid == my_pid.as_raw_pid() {
                                        // This is our UI thread connecting back to us.
                                        tracing::debug!("UI thread connected");
                                        state.core.ui_thread_client = Some(client);
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

        let (config, config_notifier) = Xfwl4Config::new(handle.clone()).expect("Failed to load initial config");
        handle
            .insert_source(config_notifier, |event, _, state| {
                if let channel::Event::Msg(property_name) = event {
                    if property_name == "theme" {
                        if let Err(err) = state.load_decoration_theme() {
                            tracing::warn!("Failed to load theme: {err}");
                        }
                    } else if state.core.config.is_decoration_setting(&property_name) {
                        state.update_window_decorations_properties();
                    }
                }
            })
            .expect("Failed to register xfconf xfwm4 source with event loop");

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
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);
        let data_control_state =
            DataControlState::new::<Self, _>(&dh, Some(&primary_selection_state), |client| !client.has_security_context());
        let mut seat_state = SeatState::new();
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let viewporter_state = ViewporterState::new::<Self>(&dh);
        let xdg_activation_state = XdgActivationState::new::<Self>(&dh);
        let decoration_state = DecorationState::new::<BackendData>(&dh, handle.clone());
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let xdg_toplevel_icon_manager = XdgToplevelIconManager::new::<Self>(&dh);
        let presentation_state = PresentationState::new::<Self>(&dh, clock.id() as u32);
        let fractional_scale_manager_state = FractionalScaleManagerState::new::<Self>(&dh);
        let xdg_foreign_state = XdgForeignState::new::<Self>(&dh);
        let single_pixel_buffer_state = SinglePixelBufferState::new::<Self>(&dh);
        let fifo_manager_state = FifoManagerState::new::<Self>(&dh);
        let commit_timing_manager_state = CommitTimingManagerState::new::<Self>(&dh);
        TextInputManagerState::new::<Self>(&dh);
        InputMethodManagerState::new::<Self, _>(&dh, |client| !client.has_security_context());
        VirtualKeyboardManagerState::new::<Self, _>(&dh, |client| !client.has_security_context());
        // Expose global only if backend supports relative motion events
        if BackendData::HAS_RELATIVE_MOTION {
            RelativePointerManagerState::new::<Self>(&dh);
        }
        PointerConstraintsState::new::<Self>(&dh);
        if BackendData::HAS_GESTURES {
            PointerGesturesState::new::<Self>(&dh);
        }
        TabletManagerState::new::<Self>(&dh);
        SecurityContextState::new::<Self, _>(&dh, |client| !client.has_security_context());
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
                    && let Some(keyboard_handle) = state.core.seat.get_keyboard()
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

        let ext_session_lock_state = ExtSessionLockState::new::<BackendData>(&dh);

        let foreign_toplevel_state = ForeignToplevelState::<BackendData>::new(&dh);

        let ext_image_capture_source_state = ExtImageCaptureSourceState::new::<BackendData>(&dh);
        let image_copy_capture_state = ImageCopyCaptureState::new_with_filter::<Self, _>(&dh, |client| !client.has_security_context());
        let wlr_screencopy_state = WlrScreencopyState::new::<Self, _>(&dh, |client| !client.has_security_context());

        let wlr_output_management_state = WlrOutputManagementState::new::<Self, _>(&dh, |client| !client.has_security_context());

        #[cfg(feature = "xwayland")]
        let xwayland_shell_state = xwayland_shell::XWaylandShellState::new::<Self>(&dh.clone());

        #[cfg(feature = "xwayland")]
        XWaylandKeyboardGrabState::new::<Self>(&dh.clone());

        let workspace_manager = WorkspaceManager::new(&dh, &handle);

        let (cursor_theme, notifier) = CursorTheme::new(handle.clone());
        handle
            .insert_source(notifier, |_, _, _state| {
                // TODO: update cursor?
            })
            .unwrap();
        let pointer_image = cursor_theme.load_cursor(CursorName::Default).unwrap_or_else(|_| Cursor::fallback());

        Xfwl4State {
            backend: backend_data,
            core: Xfwl4Core {
                display_handle: dh,
                socket_name,
                stop_signal,
                handle,
                config,
                outputs_config: OutputsConfig::default(),
                workspace_manager,
                popups: PopupManager::default(),
                pending_windows: HashMap::new(),
                decoration_theme: None,
                font_map: pangocairo::FontMap::new(),
                font_options: gtk::cairo::FontOptions::new().expect("creating cairo FontOptions should not fail"),
                icon_theme: FreedesktopIconsIconTheme::new(),
                cursor_theme,
                pointer_behavior_settings: PointerBehavior::default(),
                to_ui_channel_tx,
                ui_thread_client: None,
                cycling_windows: false,
                window_menu_anchor: None,

                protocol_delegates: ProtocolDelegates::new(
                    commit_timing_manager_state,
                    data_control_state,
                    data_device_state,
                    decoration_state,
                    ext_idle_notifier_state,
                    ext_image_capture_source_state,
                    ext_session_lock_state,
                    fifo_manager_state,
                    fractional_scale_manager_state,
                    image_copy_capture_state,
                    keyboard_shortcuts_inhibit_state,
                    output_manager_state,
                    presentation_state,
                    primary_selection_state,
                    seat_state,
                    single_pixel_buffer_state,
                    viewporter_state,
                    wlr_screencopy_state,
                    xdg_activation_state,
                    xdg_foreign_state,
                ),
                shell_protocol_delegates: ShellProtocolDelegates::new(
                    compositor_state,
                    layer_shell_state,
                    xdg_shell_state,
                    #[cfg(feature = "xwayland")]
                    xwayland_shell_state,
                ),
                shm_state,
                xdg_toplevel_icon_manager,
                foreign_toplevel_state,
                wlr_output_management_state,

                cursor_status: CursorImageStatus::default_named(),
                pointer_image,
                pointer_image_cache: Vec::new(),
                pointer_element: PointerElement::default(),
                dnd_icon: None,
                #[cfg(feature = "debug")]
                debug: crate::core::debug::BackendDebug::new(),

                suppressed_keys: Vec::new(),
                seat_name,
                seat,
                keyboard_config,
                pointer,
                clock,

                #[cfg(feature = "xwayland")]
                xwm: None,
                #[cfg(feature = "xwayland")]
                xdisplay: None,
                #[cfg(feature = "xwayland")]
                x11conn: None,
                #[cfg(feature = "debug")]
                renderdoc: renderdoc::RenderDoc::new().ok(),
            },
        }
    }

    pub fn socket_name(&self) -> Option<&str> {
        self.core.socket_name.as_deref()
    }

    pub fn backend_type(&self) -> BackendType {
        self.backend.backend_type()
    }

    pub fn send_to_ui(&self, msg: ToUiMessage) {
        let _ = self.core.to_ui_channel_tx.send(msg);
    }

    pub fn cycle_tabwin_mode(&self) -> TabwinMode {
        self.core.config.cycle_tabwin_mode()
    }

    pub fn cycle_preview(&self) -> bool {
        self.core.config.cycle_preview()
    }

    #[cfg(feature = "xwayland")]
    pub fn start_xwayland(&mut self, xwayland_scale: f64) -> anyhow::Result<u32> {
        use std::process::Stdio;

        use smithay::wayland::compositor::CompositorHandler;

        let (xwayland, client) = XWayland::spawn(
            &self.core.display_handle,
            None,
            std::iter::empty::<(String, String)>(),
            true,
            Stdio::null(),
            Stdio::null(),
            |_| (),
        )
        .expect("failed to start XWayland");

        let display_number = xwayland.display_number();

        let display_handle = self.core.display_handle.clone();
        let ret = self.core.handle.insert_source(xwayland, move |event, _, data| match event {
            XWaylandEvent::Ready {
                x11_socket,
                display_number,
            } => {
                data.client_compositor_state(&client).set_client_scale(xwayland_scale);
                let mut wm = X11Wm::start_wm(data.core.handle.clone(), &display_handle, x11_socket, client.clone())
                    .expect("Failed to attach X11 Window Manager");

                let cursor = data
                    .core
                    .cursor_theme
                    .load_cursor(CursorName::Default)
                    .unwrap_or_else(|_| data.core.cursor_theme.fallback_cursor());
                let (image, _) = cursor.get_image(1, Duration::ZERO);
                wm.set_cursor(
                    &image.pixels_rgba,
                    Size::from((image.width as u16, image.height as u16)),
                    Point::from((image.xhot as u16, image.yhot as u16)),
                )
                .expect("Failed to set xwayland default cursor");
                data.core.xwm = Some(wm);
                data.core.xdisplay = Some(display_number);
                data.core.x11conn = Some(x11rb::connect(Some(&format!(":{display_number}"))).unwrap())
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

    pub fn load_decoration_theme(&mut self) -> anyhow::Result<DecorationTheme> {
        let theme_path = self.core.config.theme_path().ok_or_else(|| anyhow!("Unable to find theme path"))?;
        let renderer = self.backend.renderer(None)?;
        let decoration_theme = DecorationTheme::load(renderer, theme_path, &self.core.config.color_names())?;
        self.core.decoration_theme = Some(decoration_theme.clone());

        self.update_window_decorations_theme(&decoration_theme);

        if let Some(menu_button) =
            decoration_theme.button_texture(DecorButtonName::Menu, DecorButtonState::Active, DecorBackgroundState::Active)
        {
            let icon_size = menu_button.size().w.min(menu_button.size().h);
            self.core.xdg_toplevel_icon_manager.add_icon_size(icon_size);
        }

        Ok(decoration_theme)
    }

    fn update_window_decorations_theme(&self, decoration_theme: &DecorationTheme) {
        for workspace in self.core.workspace_manager.workspaces() {
            for window in workspace.elements() {
                if let Some(window_decorations) = window.decoration_state().window_decorations_mut() {
                    window_decorations.update_theme(decoration_theme);
                }
            }
        }
    }

    pub(crate) fn update_window_decorations_icon_theme(&self) {
        for workspace in self.core.workspace_manager.workspaces() {
            for window in workspace.elements() {
                if let Some(window_decorations) = window.decoration_state().window_decorations_mut() {
                    window_decorations.icon_theme_updated();
                }
            }
        }
    }

    pub(crate) fn update_window_decorations_properties(&self) {
        for workspace in self.core.workspace_manager.workspaces() {
            for window in workspace.elements() {
                if let Some(window_decorations) = window.decoration_state().window_decorations_mut() {
                    window_decorations.theme_properties_updated();
                }
            }
        }
    }

    pub(crate) fn update_window_decorations_font_options(&self) {
        for workspace in self.core.workspace_manager.workspaces() {
            for window in workspace.elements() {
                if let Some(window_decorations) = window.decoration_state().window_decorations_mut() {
                    window_decorations.update_font_options(self.core.font_options.clone());
                }
            }
        }
    }

    pub fn refresh_and_flush_clients(&mut self) {
        self.core.workspace_manager.refresh_spaces();
        self.core.popups.cleanup();

        if let Err(err) = self.core.display_handle.flush_clients() {
            error!("Fatal error: Failed to flush Wayland clients: {err}");
            std::process::exit(1);
        }
    }

    pub fn shutdown(&self) {
        self.core.stop_signal.stop();
        self.core.stop_signal.wakeup();
    }
}

impl<BackendData: Backend + 'static> Xfwl4Core<BackendData> {
    pub fn set_cursor(&mut self, cursor_name: CursorName) {
        if let Ok(cursor) = self.cursor_theme.load_cursor(cursor_name) {
            self.pointer_image = cursor;
        }

        // XXX: set for xwayland WM too?  probably not?
    }

    pub(super) fn notify_activity(&mut self, seat: &Seat<Xfwl4State<BackendData>>) {
        self.protocol_delegates.notify_activity(seat);
    }

    pub(super) fn session_lock_surface_for_output(&self, output: &Output) -> Option<WlSurface> {
        self.protocol_delegates.session_lock_surface_for_output(output)
    }
}
