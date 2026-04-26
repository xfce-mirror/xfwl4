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

use std::{sync::Arc, time::Duration};

use anyhow::anyhow;
use indexmap::IndexMap;
use smithay::{
    reexports::calloop::{LoopHandle, channel::Event as ChannelEvent},
    utils::{Logical, Physical, Point, Rectangle, Size, x11rb::X11Source},
    xwayland::{X11Wm, xwm::settings::Value},
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
        cursor::{CursorName, CursorTheme},
        state::Xfwl4State,
        util::ImageData,
    },
};

pub struct X11 {
    xwm: X11Wm,
    x11_conn: Arc<RustConnection>,
    screen_num: usize,
    root_window: Window,
    atoms: Atoms,
    selection_window: Window,
    _xsettings_manager: XSettingsManager,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GtkFrameExtents {
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
        //_NET_FRAME_EXTENTS,
        _NET_MOVERESIZE_WINDOW,
        _NET_NUMBER_OF_DESKTOPS,
        //_NET_REQUEST_FRAME_EXTENTS,
        //_NET_SHOWING_DESKTOP,
        //_NET_STARTUP_ID,
        _NET_SUPPORTED,
        _NET_SUPPORTING_WM_CHECK,
        //_NET_WM_ACTION_ABOVE,
        //_NET_WM_ACTION_BELOW,
        //_NET_WM_ACTION_CHANGE_DESKTOP,
        //_NET_WM_ACTION_CLOSE,
        //_NET_WM_ACTION_FULLSCREEN,
        //_NET_WM_ACTION_MAXIMIZE_HORZ,
        //_NET_WM_ACTION_MAXIMIZE_VERT,
        //_NET_WM_ACTION_MINIMIZE,
        //_NET_WM_ACTION_MOVE,
        //_NET_WM_ACTION_RESIZE,
        //_NET_WM_ACTION_SHADE,
        //_NET_WM_ACTION_STICK,
        //_NET_WM_ALLOWED_ACTIONS,
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
        mut xwm: X11Wm,
        handle: LoopHandle<'_, Xfwl4State<BackendData>>,
    ) -> anyhow::Result<Self> {
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
            xwm,
            x11_conn: Arc::new(x11_conn),
            screen_num,
            root_window,
            atoms,
            selection_window,
            _xsettings_manager: xsettings_manager,
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
        {
            state.x11_update_gtk_frame_extents(event.window);
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
            b"xfwl4\0",
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

    pub fn init_new_window_event_mask(&self, window_id: Window) -> anyhow::Result<()> {
        let aux = ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE);
        let cookie = self.x11_conn.change_window_attributes(window_id, &aux)?;
        cookie.check()?;
        Ok(())
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

    pub fn get_gtk_frame_extents(&self, window_id: Window) -> GtkFrameExtents {
        self.x11_conn
            .get_property(false, window_id, self.atoms._GTK_FRAME_EXTENTS, AtomEnum::CARDINAL, 0, 4)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| {
                reply.value32().map(|mut values| GtkFrameExtents {
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
            //self.atoms._NET_FRAME_EXTENTS,
            self.atoms._NET_MOVERESIZE_WINDOW,
            self.atoms._NET_NUMBER_OF_DESKTOPS,
            //self.atoms._NET_REQUEST_FRAME_EXTENTS,
            //self.atoms._NET_SHOWING_DESKTOP,
            //self.atoms._NET_STARTUP_ID,
            self.atoms._NET_SUPPORTED,
            self.atoms._NET_SUPPORTING_WM_CHECK,
            //self.atoms._NET_WM_ACTION_ABOVE,
            //self.atoms._NET_WM_ACTION_BELOW,
            //self.atoms._NET_WM_ACTION_CHANGE_DESKTOP,
            //self.atoms._NET_WM_ACTION_CLOSE,
            //self.atoms._NET_WM_ACTION_FULLSCREEN,
            //self.atoms._NET_WM_ACTION_MAXIMIZE_HORZ,
            //self.atoms._NET_WM_ACTION_MAXIMIZE_VERT,
            //self.atoms._NET_WM_ACTION_MINIMIZE,
            //self.atoms._NET_WM_ACTION_MOVE,
            //self.atoms._NET_WM_ACTION_RESIZE,
            //self.atoms._NET_WM_ACTION_SHADE,
            //self.atoms._NET_WM_ACTION_STICK,
            //self.atoms._NET_WM_ALLOWED_ACTIONS,
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

    pub fn update_net_number_of_desktops(&self, count: u32) {
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

    pub fn update_net_desktop_names(&self, names: Vec<String>) {
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

    pub fn update_net_desktop_layout(&self, layout: Size<u32, Logical>) {
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

    pub fn update_net_current_desktop(&self, current: u32) {
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

    pub fn update_net_wm_desktop(&self, window_id: Window, current: u32) {
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

    pub fn update_net_workarea(&self, workarea: Rectangle<u32, Physical>, n_workareas: u32) {
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

    pub fn update_resource_manager(&self, values: impl Iterator<Item = (String, Option<String>)>) -> anyhow::Result<()> {
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

    pub fn set_xwm_cursor(&mut self, cursor_theme: &CursorTheme) {
        let cursor = cursor_theme
            .load_cursor(CursorName::Default)
            .unwrap_or_else(|_| cursor_theme.fallback_cursor());
        let (image, _) = cursor.get_image(1, Duration::ZERO);
        let _ = self.xwm.set_cursor(
            &image.pixels_rgba,
            Size::from((image.width as u16, image.height as u16)),
            Point::from((image.xhot as u16, image.yhot as u16)),
        );
    }
}
