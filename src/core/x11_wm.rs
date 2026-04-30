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

use std::{
    collections::HashMap,
    os::unix::net::UnixStream,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::anyhow;
use gtk::cairo;
use indexmap::IndexMap;
use smithay::{
    desktop::{WindowSurface, layer_map_for_output},
    input::pointer::CursorIcon,
    reexports::{
        calloop::{
            LoopHandle, RegistrationToken,
            channel::Event as ChannelEvent,
            timer::{TimeoutAction, Timer},
        },
        wayland_server::{Client, DisplayHandle, Resource},
    },
    utils::{Logical, Physical, Point, Rectangle, Size, x11rb::X11Source},
    wayland::compositor::CompositorHandler,
    xwayland::{
        X11Surface, X11Wm, XWaylandClientData, XwmHandler,
        xwm::{WmWindowType, settings::Value},
    },
};
use x11rb::{
    atom_manager,
    connection::Connection,
    protocol::{
        Event,
        xproto::{
            Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask, GetPropertyReply, PropMode, Window, WindowClass,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

use crate::{
    backend::Backend,
    core::{
        config::XSettingsManager,
        cursor::CursorTheme,
        shell::{WindowElement, WindowLayout, WorkspaceLocation},
        state::Xfwl4State,
        util::ImageData,
    },
};

const STICKY_DESKTOP_NUM: u32 = 0xffffffff;

const XWAYLAND_CRASH_TIME_DURATION: Duration = Duration::from_secs(3 * 60);
const XWAYLAND_CRASH_MAX_COUNT: u32 = 5;
const XWAYLAND_CRASH_RESTART_FIXED_DELAY: Duration = Duration::from_millis(400);
const XWAYLAND_CRASH_RESTART_FIRST_DELAY: Duration = Duration::from_millis(100);

#[derive(Default)]
pub struct XWaylandCrashHistory {
    first_crash_time: Option<Instant>,
    crash_count: u32,
}

pub struct X11 {
    token: RegistrationToken,
    display_number: u32,
    xwm: X11Wm,
    client: Client,
    override_scale: Option<f64>,
    x11_conn: Arc<RustConnection>,
    screen_num: usize,
    root_window: Window,
    atoms: Atoms,
    selection_window: Window,
    _xsettings_manager: XSettingsManager,

    pending_windows: HashMap<Window, WindowElement>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrameExtents {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
}

atom_manager! {
    Atoms:

    AtomsCookie {
        _GTK_FRAME_EXTENTS,
        //_GTK_HIDE_TITLEBAR_WHEN_MAXIMIZED,
        //_GTK_SHOW_WINDOW_MENU,
        _NET_ACTIVE_WINDOW,
        //_NET_CLIENT_LIST,
        //_NET_CLIENT_LIST_STACKING,
        //_NET_CLOSE_WINDOW,
        _NET_CURRENT_DESKTOP,
        _NET_DESKTOP_GEOMETRY,
        _NET_DESKTOP_LAYOUT,
        _NET_DESKTOP_NAMES,
        _NET_DESKTOP_VIEWPORT,
        _NET_FRAME_EXTENTS,
        _NET_MOVERESIZE_WINDOW,
        _NET_NUMBER_OF_DESKTOPS,
        //_NET_REQUEST_FRAME_EXTENTS,
        _NET_SHOWING_DESKTOP,
        //_NET_STARTUP_ID,
        _NET_SUPPORTED,
        _NET_SUPPORTING_WM_CHECK,
        _NET_WM_ACTION_ABOVE,
        _NET_WM_ACTION_BELOW,
        _NET_WM_ACTION_CHANGE_DESKTOP,
        _NET_WM_ACTION_CLOSE,
        _NET_WM_ACTION_FULLSCREEN,
        _NET_WM_ACTION_MAXIMIZE_HORZ,
        _NET_WM_ACTION_MAXIMIZE_VERT,
        _NET_WM_ACTION_MINIMIZE,
        _NET_WM_ACTION_MOVE,
        _NET_WM_ACTION_RESIZE,
        _NET_WM_ACTION_SHADE,
        _NET_WM_ACTION_STICK,
        _NET_WM_ALLOWED_ACTIONS,
        _NET_WM_DESKTOP,
        //_NET_WM_FULLSCREEN_MONITORS,
        _NET_WM_ICON,
        //_NET_WM_ICON_GEOMETRY,
        //_NET_WM_ICON_NAME,
        _NET_WM_MOVERESIZE,
        _NET_WM_NAME,
        //_NET_WM_OPAQUE_REGION,
        _NET_WM_PID,
        //_NET_WM_PING,
        _NET_WM_STATE,
        _NET_WM_STATE_ABOVE,
        _NET_WM_STATE_BELOW,
        _NET_WM_STATE_DEMANDS_ATTENTION,
        _NET_WM_STATE_FOCUSED,
        _NET_WM_STATE_FULLSCREEN,
        _NET_WM_STATE_HIDDEN,
        _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_STATE_MODAL,
        _NET_WM_STATE_SHADED,
        _NET_WM_STATE_SKIP_PAGER,
        _NET_WM_STATE_SKIP_TASKBAR,
        _NET_WM_STATE_STICKY,
        //_NET_WM_STRUT,
        //_NET_WM_STRUT_PARTIAL,
        //_NET_WM_SYNC_REQUEST,
        //_NET_WM_SYNC_REQUEST_COUNTER,
        _NET_WM_USER_TIME,
        //_NET_WM_USER_TIME_WINDOW,
        _NET_WM_WINDOW_OPACITY,
        //_NET_WM_WINDOW_OPACITY_LOCKED,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DESKTOP,
        _NET_WM_WINDOW_TYPE_DIALOG,
        _NET_WM_WINDOW_TYPE_DOCK,
        _NET_WM_WINDOW_TYPE_MENU,
        _NET_WM_WINDOW_TYPE_NORMAL,
        _NET_WM_WINDOW_TYPE_SPLASH,
        _NET_WM_WINDOW_TYPE_TOOLBAR,
        _NET_WM_WINDOW_TYPE_UTILITY,
        _NET_WORKAREA,
        _XFWL4_CLOSE_CONNECTION,
        UTF8_STRING,
    }
}

impl X11 {
    pub fn new<BackendData: Backend + 'static>(
        display_number: u32,
        xwayland_client: Client,
        x11_socket: UnixStream,
        token: RegistrationToken,
        override_scale: Option<f64>,
        handle: LoopHandle<'static, Xfwl4State<BackendData>>,
        display_handle: &DisplayHandle,
    ) -> anyhow::Result<Self> {
        if let Some(scale) = override_scale
            && let Some(state) = xwayland_client.get_data::<XWaylandClientData>()
        {
            state.compositor_state.set_client_scale(scale);
        }

        let mut xwm = X11Wm::start_wm(handle.clone(), display_handle, x11_socket, xwayland_client.clone())
            .map_err(|err| anyhow!("Failed to start X11Wm: {err}"))?;

        let (x11_conn, screen_num) = x11rb::connect(Some(&format!(":{display_number}")))
            .map_err(|err| anyhow!("failed to connect back to XWayland server: {err}"))?;
        let root_window = x11_conn
            .setup()
            .roots
            .get(screen_num)
            .ok_or_else(|| anyhow!("unable to find X11 root window"))?
            .root;

        let atoms = Atoms::new(&x11_conn)?.reply()?;

        let selection_window =
            Self::create_selection_window(&x11_conn, screen_num).map_err(|err| anyhow!("failed to create X11 selection window: {err}"))?;

        let xsettings_manager = XSettingsManager::new(handle.clone());
        let xsettings = xsettings_manager.all_xsettings();
        xwm.set_xsettings(xsettings.into_iter())?;

        let x11 = Self {
            token,
            display_number,
            xwm,
            client: xwayland_client,
            override_scale,
            x11_conn: Arc::new(x11_conn),
            screen_num,
            root_window,
            atoms,
            selection_window,
            _xsettings_manager: xsettings_manager,
            pending_windows: Default::default(),
        };

        x11.init()?;

        let x11_source = X11Source::new(Arc::clone(&x11.x11_conn), x11.selection_window, x11.atoms._XFWL4_CLOSE_CONNECTION);
        handle
            .insert_source(x11_source, |event, _, state| {
                if let ChannelEvent::Msg(event) = event {
                    Self::handle_xevent(state, event);
                }
            })
            .map_err(|err| anyhow!("{err}"))?;

        Ok(x11)
    }

    fn create_selection_window(x11_conn: &RustConnection, screen_num: usize) -> anyhow::Result<Window> {
        let selection_window = x11_conn.generate_id()?;
        let screen = x11_conn
            .setup()
            .roots
            .get(screen_num)
            .ok_or_else(|| anyhow!("no screen available"))?;
        x11_conn
            .create_window(
                screen.root_depth,
                selection_window,
                screen.root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &Default::default(),
            )
            .map_err(|err| anyhow!("failed to create X11 selection window: {err}"))?;

        Ok(selection_window)
    }

    fn handle_xevent<BackendData: Backend + 'static>(state: &mut Xfwl4State<BackendData>, event: Event) {
        if let Event::PropertyNotify(event) = event
            && Some(event.atom) == state.core.xwayland.as_ref().map(|xw| xw.atoms._GTK_FRAME_EXTENTS)
            && let Some(window) = state.core.workspace_manager.find_window(|elem| {
                elem.0
                    .x11_surface()
                    .is_some_and(|x11_surface| x11_surface.window_id() == event.window)
            })
        {
            state.x11_update_window_gtk_frame_extents(&window);
        }
    }

    fn init(&self) -> anyhow::Result<()> {
        let selection_name = format!("_NET_DESKTOP_LAYOUT_S{}", self.screen_num);
        let net_desktop_layout_sn = self.x11_conn.intern_atom(false, selection_name.as_bytes())?.reply()?.atom;

        self.x11_conn
            .set_selection_owner(self.selection_window, net_desktop_layout_sn, x11rb::CURRENT_TIME)?;

        self.x11_conn.change_property8(
            PropMode::REPLACE,
            self.selection_window,
            self.atoms._NET_WM_NAME,
            self.atoms.UTF8_STRING,
            b"xfwl4",
        )?;

        self.x11_conn.change_property32(
            PropMode::REPLACE,
            self.selection_window,
            self.atoms._NET_SUPPORTING_WM_CHECK,
            AtomEnum::WINDOW,
            &[self.selection_window],
        )?;
        self.x11_conn.change_property32(
            PropMode::REPLACE,
            self.root_window,
            self.atoms._NET_SUPPORTING_WM_CHECK,
            AtomEnum::WINDOW,
            &[self.selection_window],
        )?;

        self.set_net_supported()?;
        self.set_net_desktop_viewport()?;

        Ok(())
    }

    /// The resource mask helps us determine if two `X11Surface`s belong to the same X11 client.
    /// We can't check the Wayland surface's Client, because they are all the same client (it's the
    /// XWayland connection with our compositor).  Most (all?) X11 server implementations reserve a
    /// portion of the Window ID to uniquely identify the client that created the window.  This is
    /// not fixed; it's a runtime setting that the X server returns as part of connection setup.
    /// The mask is inverted from what we need (it identifies the resource portion of the ID, not
    /// the client portion), so we need to invert it.  We also don't use the full number of bits,
    /// because X11 only uses 29 bits for resource IDs.
    pub fn client_resource_mask(&self) -> u32 {
        self.x11_conn.setup().resource_id_mask & 0x1fffffff
    }

    pub fn xwm(&mut self) -> &mut X11Wm {
        &mut self.xwm
    }

    pub fn update_xsetting(&mut self, name: &str, value: Value) -> anyhow::Result<()> {
        self.xwm.set_xsettings([(name.to_owned(), value)].into_iter())?;
        Ok(())
    }

    fn get_property<T: Into<Atom>>(&self, window_id: Window, property: Atom, type_: T, length: u32) -> Option<GetPropertyReply> {
        let cookie = self
            .x11_conn
            .get_property(false, window_id, property, type_, 0, length)
            .inspect_err(|err| tracing::warn!("Failed to send request for {property} for window {window_id}: {err}"))
            .ok()?;
        cookie
            .reply()
            .inspect_err(|err| tracing::warn!("Failed to fetch reply for {property} for window {window_id}: {err}"))
            .ok()
    }

    pub fn init_window_as_pending(&mut self, window: WindowElement) -> anyhow::Result<()> {
        if let WindowSurface::X11(surface) = window.0.underlying_surface() {
            let window_id = surface.window_id();

            let aux = ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE);
            let cookie = self.x11_conn.change_window_attributes(window_id, &aux)?;
            cookie.check()?;

            self.pending_windows.insert(window_id, window);

            Ok(())
        } else {
            Err(anyhow!("Window is not an X11 window"))
        }
    }

    pub fn remove_pending_window(&mut self, window_id: Window) -> Option<WindowElement> {
        self.pending_windows.remove(&window_id)
    }

    pub fn get_user_time(&self, window_id: Window) -> Option<u32> {
        let reply = self.get_property(window_id, self.atoms._NET_WM_USER_TIME, AtomEnum::CARDINAL, 1)?;
        reply.value32().and_then(|mut values| values.next())
    }

    pub fn get_net_wm_icon(&self, window_id: Window) -> Option<ImageData> {
        let reply = self
            .x11_conn
            .get_property(false, window_id, self.atoms._NET_WM_ICON, AtomEnum::CARDINAL, 0, u32::MAX)
            .inspect_err(|err| tracing::warn!("Failed to send request for _NET_WM_ICON for window {window_id}: {err}"))
            .ok()
            .and_then(|cookie| {
                cookie
                    .reply()
                    .inspect_err(|err| tracing::warn!("Failed to fetch reply for _NET_WM_ICON for window {window_id}: {err}"))
                    .ok()
            })?;
        let mut prop_data = reply.value32()?;

        let mut icons = Vec::new();
        while let (Some(width), Some(height)) = (prop_data.next(), prop_data.next()) {
            let n_pixels = (width * height) as usize;
            let bytes = prop_data
                .by_ref()
                .take(n_pixels)
                .flat_map(|argb| {
                    [
                        ((argb >> 16) & 0xff) as u8,
                        ((argb >> 8) & 0xff) as u8,
                        (argb & 0xff) as u8,
                        ((argb >> 24) & 0xff) as u8,
                    ]
                })
                .collect::<Vec<u8>>();

            if bytes.len() == n_pixels {
                icons.push(ImageData::RgbaPixels { bytes, width, height });
            } else {
                break;
            }
        }

        // XXX: This just picks the largest icon, which may not be what we really want
        icons.into_iter().max_by_key(|data| match data {
            ImageData::RgbaPixels { width, .. } => *width,
            _ => 0,
        })
    }

    fn get_gtk_frame_extents(&self, window_id: Window) -> FrameExtents {
        self.x11_conn
            .get_property(false, window_id, self.atoms._GTK_FRAME_EXTENTS, AtomEnum::CARDINAL, 0, 4)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| {
                reply.value32().map(|mut values| FrameExtents {
                    left: values.next().filter(|v| (*v as i32) >= 0).unwrap_or(0),
                    right: values.next().filter(|v| (*v as i32) >= 0).unwrap_or(0),
                    top: values.next().filter(|v| (*v as i32) >= 0).unwrap_or(0),
                    bottom: values.next().filter(|v| (*v as i32) >= 0).unwrap_or(0),
                })
            })
            .unwrap_or_default()
    }

    fn set_net_supported(&self) -> anyhow::Result<()> {
        let supported = &[
            self.atoms._GTK_FRAME_EXTENTS,
            //self.atoms._GTK_HIDE_TITLEBAR_WHEN_MAXIMIZED,
            //self.atoms._GTK_SHOW_WINDOW_MENU,
            self.atoms._NET_ACTIVE_WINDOW,
            //self.atoms._NET_CLIENT_LIST,
            //self.atoms._NET_CLIENT_LIST_STACKING,
            //self.atoms._NET_CLOSE_WINDOW,
            self.atoms._NET_CURRENT_DESKTOP,
            self.atoms._NET_DESKTOP_GEOMETRY,
            self.atoms._NET_DESKTOP_LAYOUT,
            self.atoms._NET_DESKTOP_NAMES,
            self.atoms._NET_DESKTOP_VIEWPORT,
            self.atoms._NET_FRAME_EXTENTS,
            self.atoms._NET_MOVERESIZE_WINDOW,
            self.atoms._NET_NUMBER_OF_DESKTOPS,
            //self.atoms._NET_REQUEST_FRAME_EXTENTS,
            self.atoms._NET_SHOWING_DESKTOP,
            //self.atoms._NET_STARTUP_ID,
            self.atoms._NET_SUPPORTED,
            self.atoms._NET_SUPPORTING_WM_CHECK,
            self.atoms._NET_WM_ACTION_ABOVE,
            self.atoms._NET_WM_ACTION_BELOW,
            self.atoms._NET_WM_ACTION_CHANGE_DESKTOP,
            self.atoms._NET_WM_ACTION_CLOSE,
            self.atoms._NET_WM_ACTION_FULLSCREEN,
            self.atoms._NET_WM_ACTION_MAXIMIZE_HORZ,
            self.atoms._NET_WM_ACTION_MAXIMIZE_VERT,
            self.atoms._NET_WM_ACTION_MINIMIZE,
            self.atoms._NET_WM_ACTION_MOVE,
            self.atoms._NET_WM_ACTION_RESIZE,
            self.atoms._NET_WM_ACTION_SHADE,
            self.atoms._NET_WM_ACTION_STICK,
            self.atoms._NET_WM_ALLOWED_ACTIONS,
            self.atoms._NET_WM_DESKTOP,
            //self.atoms._NET_WM_FULLSCREEN_MONITORS,
            self.atoms._NET_WM_ICON,
            //self.atoms._NET_WM_ICON_GEOMETRY,
            //self.atoms._NET_WM_ICON_NAME,
            self.atoms._NET_WM_MOVERESIZE,
            self.atoms._NET_WM_NAME,
            //self.atoms._NET_WM_OPAQUE_REGION,
            self.atoms._NET_WM_PID,
            //self.atoms._NET_WM_PING,
            self.atoms._NET_WM_STATE,
            self.atoms._NET_WM_STATE_ABOVE,
            self.atoms._NET_WM_STATE_BELOW,
            self.atoms._NET_WM_STATE_DEMANDS_ATTENTION,
            self.atoms._NET_WM_STATE_FOCUSED,
            self.atoms._NET_WM_STATE_FULLSCREEN,
            self.atoms._NET_WM_STATE_HIDDEN,
            self.atoms._NET_WM_STATE_MAXIMIZED_HORZ,
            self.atoms._NET_WM_STATE_MAXIMIZED_VERT,
            self.atoms._NET_WM_STATE_MODAL,
            self.atoms._NET_WM_STATE_SHADED,
            self.atoms._NET_WM_STATE_SKIP_PAGER,
            self.atoms._NET_WM_STATE_SKIP_TASKBAR,
            self.atoms._NET_WM_STATE_STICKY,
            //self.atoms._NET_WM_STRUT,
            //self.atoms._NET_WM_STRUT_PARTIAL,
            //self.atoms._NET_WM_SYNC_REQUEST,
            //self.atoms._NET_WM_SYNC_REQUEST_COUNTER,
            self.atoms._NET_WM_USER_TIME,
            //self.atoms._NET_WM_USER_TIME_WINDOW,
            self.atoms._NET_WM_WINDOW_OPACITY,
            //self.atoms._NET_WM_WINDOW_OPACITY_LOCKED,
            self.atoms._NET_WM_WINDOW_TYPE,
            self.atoms._NET_WM_WINDOW_TYPE_DESKTOP,
            self.atoms._NET_WM_WINDOW_TYPE_DIALOG,
            self.atoms._NET_WM_WINDOW_TYPE_DOCK,
            self.atoms._NET_WM_WINDOW_TYPE_MENU,
            self.atoms._NET_WM_WINDOW_TYPE_NORMAL,
            self.atoms._NET_WM_WINDOW_TYPE_SPLASH,
            self.atoms._NET_WM_WINDOW_TYPE_TOOLBAR,
            self.atoms._NET_WM_WINDOW_TYPE_UTILITY,
            self.atoms._NET_WORKAREA,
        ];

        let cookie = self.x11_conn.change_property32(
            PropMode::REPLACE,
            self.root_window,
            self.atoms._NET_SUPPORTED,
            AtomEnum::ATOM,
            supported,
        )?;
        cookie.check()?;
        Ok(())
    }

    fn set_net_desktop_viewport(&self) -> anyhow::Result<()> {
        let cookie = self.x11_conn.change_property32(
            PropMode::REPLACE,
            self.root_window,
            self.atoms._NET_DESKTOP_VIEWPORT,
            AtomEnum::CARDINAL,
            &[0, 0],
        )?;
        cookie.check()?;
        Ok(())
    }

    fn update_net_number_of_desktops(&self, count: u32) {
        let do_update = || -> anyhow::Result<()> {
            let cookie = self.x11_conn.change_property32(
                PropMode::REPLACE,
                self.root_window,
                self.atoms._NET_NUMBER_OF_DESKTOPS,
                AtomEnum::CARDINAL,
                &[count],
            )?;
            cookie.check()?;
            Ok(())
        };

        if let Err(err) = do_update() {
            tracing::warn!("Failed to update X11 property for number of desktops: {err}");
        }
    }

    fn update_net_desktop_names(&self, names: Vec<String>) {
        let do_update = |names_bytes: &[u8]| -> anyhow::Result<()> {
            let cookie = self.x11_conn.change_property8(
                PropMode::REPLACE,
                self.root_window,
                self.atoms._NET_DESKTOP_NAMES,
                self.atoms.UTF8_STRING,
                names_bytes,
            )?;
            cookie.check()?;
            Ok(())
        };

        let names_bytes = names
            .into_iter()
            .flat_map(|name| name.into_bytes().into_iter().chain(std::iter::once(0u8)))
            .collect::<Vec<_>>();

        if let Err(err) = do_update(&names_bytes) {
            tracing::warn!("Failed to update X11 property for desktop names: {err}");
        }
    }

    fn update_net_desktop_layout(&self, layout: Size<u32, Logical>) {
        let do_update = |layout_bytes: &[u32]| -> anyhow::Result<()> {
            let cookie = self.x11_conn.change_property32(
                PropMode::REPLACE,
                self.root_window,
                self.atoms._NET_DESKTOP_LAYOUT,
                AtomEnum::CARDINAL,
                layout_bytes,
            )?;
            cookie.check()?;
            Ok(())
        };

        let layout_bytes = [
            0, // _NET_WM_ORIENTATION_HORZ
            layout.w, layout.h, 0, // _NET_WM_TOPLEFT
        ];

        if let Err(err) = do_update(&layout_bytes) {
            tracing::warn!("Failed to update X11 property for desktop layout: {err}");
        }
    }

    pub fn update_net_desktop_geometry(&self, geometry: Size<u32, Physical>) {
        let do_update = || -> anyhow::Result<()> {
            let cookie = self.x11_conn.change_property32(
                PropMode::REPLACE,
                self.root_window,
                self.atoms._NET_DESKTOP_GEOMETRY,
                AtomEnum::CARDINAL,
                &[geometry.w, geometry.h],
            )?;
            cookie.check()?;
            Ok(())
        };

        if let Err(err) = do_update() {
            tracing::warn!("Failed to update X11 property for desktop geometry: {err}");
        }
    }

    fn update_net_current_desktop(&self, current: u32) {
        let do_update = || -> anyhow::Result<()> {
            let cookie = self.x11_conn.change_property32(
                PropMode::REPLACE,
                self.root_window,
                self.atoms._NET_CURRENT_DESKTOP,
                AtomEnum::CARDINAL,
                &[current],
            )?;
            cookie.check()?;
            Ok(())
        };

        if let Err(err) = do_update() {
            tracing::warn!("Failed to update X11 property for current desktop: {err}");
        }
    }

    fn update_net_showing_desktop(&mut self, showing: bool) {
        tracing::debug!("setting showing desktop to {showing}");
        if let Err(err) = self.xwm.set_showing_desktop(showing) {
            tracing::warn!("Failed to update X11 property for showing desktop: {err}");
        }
    }

    fn update_net_wm_desktop(&self, window_id: Window, current: u32) {
        let do_update = || -> anyhow::Result<()> {
            let cookie = self.x11_conn.change_property32(
                PropMode::REPLACE,
                window_id,
                self.atoms._NET_WM_DESKTOP,
                AtomEnum::CARDINAL,
                &[current],
            )?;
            cookie.check()?;
            Ok(())
        };

        if let Err(err) = do_update() {
            tracing::warn!("Failed to update X11 property for window current desktop: {err}");
        }
    }

    fn update_net_workarea(&self, workarea: Rectangle<u32, Physical>, n_workareas: u32) {
        let do_update = |workarea_data: &[u32]| -> anyhow::Result<()> {
            let cookie = self.x11_conn.change_property32(
                PropMode::REPLACE,
                self.root_window,
                self.atoms._NET_WORKAREA,
                AtomEnum::CARDINAL,
                workarea_data,
            )?;
            cookie.check()?;
            Ok(())
        };

        let workarea_data = std::iter::repeat_n(
            [workarea.loc.x, workarea.loc.y, workarea.size.w, workarea.size.h],
            n_workareas as usize,
        )
        .flatten()
        .collect::<Vec<_>>();

        if let Err(err) = do_update(&workarea_data) {
            tracing::warn!("Failed to update X11 property for desktop workarea: {err}");
        }
    }

    fn update_net_frame_extents(&self, window_id: Window, extents: FrameExtents) {
        let do_update = |extents_data: [u32; 4]| -> anyhow::Result<()> {
            let cookie = self.x11_conn.change_property32(
                PropMode::REPLACE,
                window_id,
                self.atoms._NET_FRAME_EXTENTS,
                AtomEnum::CARDINAL,
                &extents_data,
            )?;
            cookie.check()?;
            Ok(())
        };

        let extents_data = [extents.left, extents.right, extents.top, extents.bottom];

        if let Err(err) = do_update(extents_data) {
            tracing::warn!("Failed to update X11 property for window frame extents: {err}");
        }
    }

    fn update_net_wm_allowed_actions(&self, window_id: Window, actions: &[Atom]) {
        let do_update = || -> anyhow::Result<()> {
            let cookie = self.x11_conn.change_property32(
                PropMode::REPLACE,
                window_id,
                self.atoms._NET_WM_ALLOWED_ACTIONS,
                AtomEnum::ATOM,
                actions,
            )?;
            cookie.check()?;
            Ok(())
        };

        if let Err(err) = do_update() {
            tracing::warn!("Failed to update X11 property for window allowed actions: {err}");
        }
    }

    fn read_resource_manager(&self) -> anyhow::Result<IndexMap<String, String>> {
        let cookie = self
            .x11_conn
            .get_property(false, self.root_window, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, u32::MAX)?;
        let reply = cookie.reply()?;
        let bytes = reply
            .value8()
            .ok_or_else(|| anyhow!("RESOURCE_MANAGER wasn't format==8"))?
            .collect::<Vec<_>>();

        // Technically this is latin1, but in practice it should be ascii, so utf8 will work.
        let s = String::from_utf8(bytes)?;

        Ok(s.split('\n')
            .filter(|line| !line.trim().is_empty())
            .filter(|line| !line.trim().starts_with('!'))
            .flat_map(|line| {
                let mut parts = line.splitn(2, ':');
                match (parts.next(), parts.next()) {
                    (Some(key), Some(value)) => Some((key.trim().to_owned(), value.trim().to_owned())),
                    _ => None,
                }
            })
            .collect())
    }

    fn update_resource_manager(&self, values: impl Iterator<Item = (String, Option<String>)>) -> anyhow::Result<()> {
        let mut rm = self
            .read_resource_manager()
            .inspect_err(|err| tracing::warn!("Failed to read/parse RESOURCE_MANAGER; overwriting: {err}"))
            .unwrap_or_default();

        for (key, value) in values {
            if let Some(value) = value {
                rm.insert(key, value);
            } else {
                rm.shift_remove(&key);
            }
        }

        let rm_str = rm
            .into_iter()
            .map(|(key, value)| format!("{key}:\t{value}\n"))
            .collect::<Vec<_>>()
            .join("");
        let cookie = self.x11_conn.change_property8(
            PropMode::REPLACE,
            self.root_window,
            AtomEnum::RESOURCE_MANAGER,
            AtomEnum::STRING,
            rm_str.as_bytes(),
        )?;
        cookie.check()?;

        Ok(())
    }

    pub fn set_xwm_cursor(&mut self, cursor_theme: &mut CursorTheme, scale: f64) {
        let scale = self.override_scale.unwrap_or(scale);

        let cursor = cursor_theme
            .load_cursor(CursorIcon::Default)
            .unwrap_or_else(|_| cursor_theme.fallback_cursor());
        let image = cursor.get_image(scale, Duration::ZERO);
        let _ = self.xwm.set_cursor(
            &image.pixels_rgba,
            Size::from((image.width as u16, image.height as u16)),
            Point::from((image.xhot as u16, image.yhot as u16)),
        );
    }

    pub fn update_client_scale(&self, scale: f64) {
        if self.override_scale.is_none()
            && let Some(state) = self.client.get_data::<XWaylandClientData>()
        {
            state.compositor_state.set_client_scale(scale);
        }
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub(in crate::core) fn xwayland_destroyed(&mut self) -> Option<(u32, Option<f64>)> {
        if let Some(xw) = self.core.xwayland.as_ref() {
            self.core.handle.remove(xw.token);

            let dead_x11_surfaces = self
                .core
                .workspace_manager
                .workspaces()
                .iter()
                .flat_map(|workspace| workspace.all_windows().flat_map(|window| window.0.x11_surface()))
                .cloned()
                .collect::<Vec<_>>();
            let xwm_id = xw.xwm.id();
            for surface in dead_x11_surfaces {
                self.destroyed_window(xwm_id, surface);
            }

            let X11 {
                display_number,
                override_scale,
                ..
            } = self.core.xwayland.take().unwrap();
            Some((display_number, override_scale))
        } else {
            None
        }
    }

    pub(in crate::core) fn maybe_schedule_xwayland_restart(&mut self, display_number: u32, override_xwayland_scale: Option<f64>) {
        let should_restart = if let Some(first_crash_time) = self.core.xwayland_crash_history.first_crash_time.as_ref() {
            let since = first_crash_time.elapsed();
            if since > XWAYLAND_CRASH_TIME_DURATION {
                self.core.xwayland_crash_history.first_crash_time = None;
                self.core.xwayland_crash_history.crash_count = 0;
                true
            } else {
                self.core.xwayland_crash_history.crash_count += 1;
                self.core.xwayland_crash_history.crash_count < XWAYLAND_CRASH_MAX_COUNT
            }
        } else {
            true
        };

        if should_restart {
            if self.core.xwayland_crash_history.first_crash_time.is_none() {
                self.core.xwayland_crash_history.first_crash_time = Some(Instant::now());
            }

            let restart_delay = XWAYLAND_CRASH_RESTART_FIXED_DELAY
                + XWAYLAND_CRASH_RESTART_FIRST_DELAY * 2u32.pow(self.core.xwayland_crash_history.crash_count);
            tracing::warn!("XWayland server exited unexpectedly; restarting in {}ms", restart_delay.as_millis());

            let _ = self
                .core
                .handle
                .insert_source(Timer::from_duration(restart_delay), move |_, _, state| {
                    if state.core.is_running
                        && let Err(err) = state.start_xwayland(Some(display_number), override_xwayland_scale)
                    {
                        tracing::error!("Failed to restart XWayland: {err}");
                    }
                    TimeoutAction::Drop
                });
        } else {
            tracing::warn!("XWayland server exiting too often; won't restart");
        }
    }

    pub(in crate::core) fn x11_update_workspace_count(&self, num_workspaces: u32) {
        if let Some(xw) = self.core.xwayland.as_ref() {
            xw.update_net_number_of_desktops(num_workspaces);
        }
    }

    pub(in crate::core) fn x11_update_workspace_names(&self, names: Vec<String>) {
        if let Some(xw) = self.core.xwayland.as_ref() {
            xw.update_net_desktop_names(names);
        }
    }

    pub(in crate::core) fn x11_update_workspace_layout(&self, layout: Size<u32, Logical>) {
        if let Some(xw) = self.core.xwayland.as_ref() {
            xw.update_net_desktop_layout(layout);
        }
    }

    pub(in crate::core) fn x11_update_active_workspace(&self, active_ws_num: u32) {
        if let Some(xw) = self.core.xwayland.as_ref() {
            xw.update_net_current_desktop(active_ws_num);
        }
    }

    pub(in crate::core) fn x11_set_showing_desktop(&mut self, showing: bool) {
        if let Some(xw) = self.core.xwayland.as_mut() {
            xw.update_net_showing_desktop(showing);
        }
    }

    pub(in crate::core) fn x11_update_window_workspace_location(&self, window: &WindowElement) {
        if let WindowSurface::X11(surface) = window.0.underlying_surface()
            && let Some(xw) = self.core.xwayland.as_ref()
        {
            let desktop_value = match window.props().workspace_loc {
                WorkspaceLocation::All => STICKY_DESKTOP_NUM,
                WorkspaceLocation::Single(num) => num,
            };
            xw.update_net_wm_desktop(surface.window_id(), desktop_value);
        }
    }

    pub(in crate::core) fn x11_update_workarea(&self) {
        if let Some(xw) = self.core.xwayland.as_ref()
            && let Some((workarea, min_x, min_y)) = self
                .core
                .workspace_manager
                .outputs()
                .map(|output| {
                    let location = output.current_location();
                    let scale = output.current_scale().fractional_scale();
                    let phys_location = location.to_f64().to_physical(scale).to_i32_round::<i32>();

                    let map = layer_map_for_output(output);
                    let mut zone = map.non_exclusive_zone();
                    zone.loc += location;
                    let zone = zone.to_f64().to_physical(scale).to_i32_round::<i32>();

                    (zone, phys_location.x, phys_location.y)
                })
                .reduce(|(workarea, min_x, min_y), (geom, xorigin, yorigin)| {
                    let workarea = workarea.merge(geom);
                    let min_x = min_x.min(xorigin);
                    let min_y = min_y.min(yorigin);
                    (workarea, min_x, min_y)
                })
        {
            let workarea = Rectangle::new(
                // The X11 root window origin is always (0, 0), but ours could be basically
                // anything, so translate it if needed.
                ((workarea.loc.x - min_x) as u32, (workarea.loc.y - min_y) as u32).into(),
                (workarea.size.w as u32, workarea.size.h as u32).into(),
            );
            xw.update_net_workarea(workarea, self.core.workspace_manager.workspaces().len() as u32);
        }
    }

    pub(in crate::core) fn x11_update_window_gtk_frame_extents(&mut self, window: &WindowElement) {
        if let Some(xw) = self.core.xwayland.as_ref()
            && let Some(surface) = window.0.x11_surface()
        {
            let extents = xw.get_gtk_frame_extents(surface.window_id());
            let scale = self.xwayland_client_scale(surface);

            let new_left = ((extents.left as f64) / scale).round() as u32;
            let new_right = ((extents.right as f64) / scale).round() as u32;
            let new_top = ((extents.top as f64) / scale).round() as u32;
            let new_bottom = ((extents.bottom as f64) / scale).round() as u32;

            let changed = if let Some(mut x11_props) = window.x11_props() {
                let changed = new_left != x11_props.client_frame_left
                    || new_right != x11_props.client_frame_right
                    || new_top != x11_props.client_frame_top
                    || new_bottom != x11_props.client_frame_bottom;

                x11_props.client_frame_left = new_left;
                x11_props.client_frame_right = new_right;
                x11_props.client_frame_top = new_top;
                x11_props.client_frame_bottom = new_bottom;

                changed
            } else {
                false
            };

            let layout = window.current_layout();
            if changed && layout != WindowLayout::Normal {
                let output_and_geom = window
                    .props()
                    .anchored_output
                    .as_ref()
                    .and_then(|weak| weak.upgrade())
                    .and_then(|output| self.core.workspace_manager.output_geometry(&output).map(|geom| (output, geom)));
                if let Some((output, output_geom)) = output_and_geom
                    && self.apply_anchored_layout(window, layout, &output, output_geom).is_none()
                {
                    self.set_window_untiled(window, None);
                }
            }
        }
    }

    pub(in crate::core) fn x11_update_window_frame_extents(&self, window: &WindowElement) {
        if let Some(xw) = self.core.xwayland.as_ref()
            && let Some(window_id) = window.0.x11_surface().map(|surface| surface.window_id())
        {
            let extents = window
                .decoration_state()
                .window_decorations()
                .map(|decorations| FrameExtents {
                    left: decorations.left_decoration_width().max(0) as u32,
                    right: decorations.right_decoration_width().max(0) as u32,
                    top: decorations.top_decoration_height().max(0) as u32,
                    bottom: decorations.bottom_decoration_height().max(0) as u32,
                })
                .unwrap_or_default();
            xw.update_net_frame_extents(window_id, extents);
        }
    }

    pub(in crate::core) fn x11_update_window_allowed_actions(&self, window: &WindowElement) {
        if let Some(xw) = self.core.xwayland.as_ref()
            && let Some(surface) = window.0.x11_surface()
            && !surface.is_override_redirect()
        {
            let actions = compute_allowed_actions(xw, surface, window);
            xw.update_net_wm_allowed_actions(surface.window_id(), &actions);
        }
    }

    fn xwayland_client_scale(&self, surface: &X11Surface) -> f64 {
        surface
            .wl_surface()
            .and_then(|s| s.client())
            .map(|c| self.client_compositor_state(&c).client_scale())
            .unwrap_or(1.0)
    }

    pub(in crate::core) fn x11_update_xrm_xft(&self) {
        fn antialias(value: cairo::Antialias) -> Option<&'static str> {
            match value {
                cairo::Antialias::None => Some("0"),
                cairo::Antialias::Gray | cairo::Antialias::Subpixel => Some("1"),
                _ => None,
            }
        }

        fn hint_style(value: cairo::HintStyle) -> Option<(&'static str, &'static str)> {
            match value {
                cairo::HintStyle::None => Some(("0", "hintnone")),
                cairo::HintStyle::Slight => Some(("1", "hintslight")),
                cairo::HintStyle::Medium => Some(("1", "hintmedium")),
                cairo::HintStyle::Full => Some(("1", "hintfull")),
                _ => None,
            }
        }

        fn subpixel_order(value: cairo::SubpixelOrder) -> Option<&'static str> {
            match value {
                cairo::SubpixelOrder::Rgb => Some("rgb"),
                cairo::SubpixelOrder::Bgr => Some("bgr"),
                cairo::SubpixelOrder::Vrgb => Some("vrgb"),
                cairo::SubpixelOrder::Vbgr => Some("vbgr"),
                _ => None,
            }
        }

        if let Some(xw) = self.core.xwayland.as_ref() {
            let font_options = &self.core.font_options;
            let hint = hint_style(font_options.hint_style());
            let values = [
                ("Xft.antialias", antialias(font_options.antialias()).map(|a| a.to_owned())),
                ("Xft.hinting", hint.map(|(h, _)| h.to_owned())),
                ("Xft.hintstyle", hint.map(|(_, s)| s.to_owned())),
                ("Xft.rgba", subpixel_order(font_options.subpixel_order()).map(|s| s.to_owned())),
                ("Xft.dpi", Some(self.core.ui_settings.font_dpi().to_string())),
            ];
            if let Err(err) = xw.update_resource_manager(values.into_iter().map(|(key, value)| (key.to_owned(), value))) {
                tracing::warn!("Failed to update Xft settings in RESOURCE_MANAGER: {err}");
            }
        }
    }

    pub(in crate::core) fn x11_update_xrm_xcursor(&self) {
        if let Some(xw) = self.core.xwayland.as_ref() {
            let values = [
                ("Xcursor.theme", Some(self.core.cursor_theme.theme_name().to_owned())),
                ("Xcursor.size", Some(self.core.cursor_theme.cursor_size().to_string())),
                ("Xcursor.theme_core", Some("1".to_owned())),
            ];
            if let Err(err) = xw.update_resource_manager(values.into_iter().map(|(key, value)| (key.to_owned(), value))) {
                tracing::warn!("Failed to update Xcursor settings in RESOURCE_MANAGER: {err}");
            }
        }
    }
}

fn compute_allowed_actions(xw: &X11, surface: &X11Surface, window: &WindowElement) -> Vec<Atom> {
    let window_type = surface.window_type().unwrap_or(WmWindowType::Normal);
    let regular_focusable = matches!(window_type, WmWindowType::Normal | WmWindowType::Dialog | WmWindowType::Utility);
    let real_toplevel = !matches!(
        window_type,
        WmWindowType::Desktop
            | WmWindowType::Dock
            | WmWindowType::Splash
            | WmWindowType::Toolbar
            | WmWindowType::Tooltip
            | WmWindowType::Combo
            | WmWindowType::DropdownMenu
            | WmWindowType::Menu
            | WmWindowType::PopupMenu
            | WmWindowType::Notification
            | WmWindowType::Dnd
    );

    let (min, max) = window.min_max_sizes();
    let resizable = real_toplevel && (max == (0, 0).into() || min != max);
    let minimized = window.minimized();
    let maximized = window.maximized();
    let has_decorations = window.decoration_state().has_decorations();

    let mut actions = Vec::with_capacity(13);
    actions.push(xw.atoms._NET_WM_ACTION_CLOSE);

    if regular_focusable {
        actions.push(xw.atoms._NET_WM_ACTION_ABOVE);
        actions.push(xw.atoms._NET_WM_ACTION_BELOW);
    }

    if !minimized {
        actions.push(xw.atoms._NET_WM_ACTION_FULLSCREEN);
        if real_toplevel {
            actions.push(xw.atoms._NET_WM_ACTION_MOVE);
        }
        if resizable && !maximized {
            actions.push(xw.atoms._NET_WM_ACTION_RESIZE);
        }
        if resizable {
            actions.push(xw.atoms._NET_WM_ACTION_MAXIMIZE_HORZ);
            actions.push(xw.atoms._NET_WM_ACTION_MAXIMIZE_VERT);
        }
        if has_decorations {
            actions.push(xw.atoms._NET_WM_ACTION_SHADE);
        }
    }

    if real_toplevel && !surface.is_skip_taskbar() {
        actions.push(xw.atoms._NET_WM_ACTION_MINIMIZE);
    }

    if real_toplevel {
        actions.push(xw.atoms._NET_WM_ACTION_CHANGE_DESKTOP);
        actions.push(xw.atoms._NET_WM_ACTION_STICK);
    }

    actions
}
