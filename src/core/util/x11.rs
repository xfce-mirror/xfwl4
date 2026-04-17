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

use std::{cell::RefCell, collections::HashMap};

use anyhow::anyhow;
use smithay::utils::{Logical, Size};
use x11rb::{
    connection::Connection,
    protocol::xproto::{Atom, AtomEnum, GetPropertyReply, PropMode, Window, WindowClass},
    wrapper::ConnectionExt,
};

use crate::core::util::ImageData;

pub struct X11<C: Connection + ConnectionExt> {
    x11_conn: C,
    screen_num: usize,
    atom_cache: RefCell<HashMap<String, Atom>>,
}

impl<C: Connection + ConnectionExt> X11<C> {
    pub fn new(x11_conn: C, screen_num: usize) -> Self {
        Self {
            x11_conn,
            screen_num,
            atom_cache: RefCell::new(HashMap::default()),
        }
    }

    pub fn create_selection_window(&self) -> anyhow::Result<Window> {
        let selection_window = self.x11_conn.generate_id()?;
        let screen = self
            .x11_conn
            .setup()
            .roots
            .get(self.screen_num)
            .ok_or_else(|| anyhow!("no screen available"))?;
        self.x11_conn.create_window(
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
        )?;

        let selection_name = format!("_NET_DESKTOP_LAYOUT_S{}", self.screen_num);
        let net_desktop_layout_sn = self.get_atom(&selection_name)?;
        self.x11_conn
            .set_selection_owner(selection_window, net_desktop_layout_sn, x11rb::CURRENT_TIME)?;

        Ok(selection_window)
    }

    pub fn get_atom(&self, name: &str) -> anyhow::Result<Atom> {
        if let Some(atom) = self.atom_cache.borrow().get(name) {
            Ok(*atom)
        } else {
            self.x11_conn
                .intern_atom(false, name.as_bytes())
                .inspect_err(|err| tracing::warn!("Failed to send X11 InternAtom request for atom {name}: {err}"))
                .map_err(anyhow::Error::from)
                .and_then(|cookie| {
                    cookie
                        .reply()
                        .inspect_err(|err| tracing::warn!("Failed to receive X11 InternAtom reply for atom {name}: {err}"))
                        .map_err(anyhow::Error::from)
                })
                .map(|reply| Atom::from(reply.atom))
                .inspect(|atom| {
                    self.atom_cache.borrow_mut().insert(name.to_owned(), *atom);
                })
        }
    }

    fn get_property<T: Into<Atom>>(&self, window_id: Window, name: &str, type_: T, length: u32) -> Option<GetPropertyReply> {
        let property = self.get_atom(name).ok()?;
        let cookie = self
            .x11_conn
            .get_property(false, window_id, property, type_, 0, length)
            .inspect_err(|err| tracing::warn!("Failed to send request for {name} for window {window_id}: {err}"))
            .ok()?;
        cookie
            .reply()
            .inspect_err(|err| tracing::warn!("Failed to fetch reply for {name} for window {window_id}: {err}"))
            .ok()
    }

    pub fn get_user_time(&self, window_id: Window) -> Option<u32> {
        let reply = self.get_property(window_id, "_NET_WM_USER_TIME", AtomEnum::CARDINAL, 1)?;
        reply.value32().and_then(|mut values| values.next())
    }

    pub fn get_net_wm_state(&self, window_id: Window) -> Option<Vec<Atom>> {
        let reply = self.get_property(window_id, "_NET_WM_STATE", AtomEnum::ATOM, u32::MAX)?;
        reply.value32().map(|iter| iter.collect::<Vec<_>>())
    }

    pub fn update_net_wm_state(&self, window_id: Window, add: &[&str], remove: &[&str]) -> Option<Vec<Atom>> {
        let mut state_atoms = self.get_net_wm_state(window_id)?;

        let add = add.iter().map(|name| self.get_atom(name).ok()).collect::<Option<Vec<_>>>()?;
        let remove = remove.iter().map(|name| self.get_atom(name).ok()).collect::<Option<Vec<_>>>()?;
        state_atoms.retain(|atom| !remove.contains(atom));
        state_atoms.extend(add);

        let net_wm_state = self.get_atom("_NET_WM_STATE").ok()?;
        if let Err(err) = self
            .x11_conn
            .change_property32(PropMode::REPLACE, window_id, net_wm_state, AtomEnum::ATOM, &state_atoms)
        {
            tracing::warn!("Failed to update _NET_WM_STATE for window {window_id}: {err}");
            None
        } else {
            Some(state_atoms)
        }
    }

    pub fn get_net_wm_icon(&self, window_id: Window) -> Option<ImageData> {
        let net_wm_icon = self.get_atom("_NET_WM_ICON").ok()?;
        let reply = self
            .x11_conn
            .get_property(false, window_id, net_wm_icon, AtomEnum::CARDINAL, 0, u32::MAX)
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

    fn root_window_id(&self) -> Window {
        // .unwrap() is safe here, as we'll always have a single screen
        self.x11_conn.setup().roots.get(self.screen_num).map(|screen| screen.root).unwrap()
    }

    pub fn update_net_number_of_desktops(&self, count: u32) {
        let do_update = || -> anyhow::Result<()> {
            let net_number_of_desktops = self.get_atom("_NET_NUMBER_OF_DESKTOPS")?;
            let cookie = self.x11_conn.change_property32(
                PropMode::REPLACE,
                self.root_window_id(),
                net_number_of_desktops,
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
            let utf8_string = self.get_atom("UTF8_STRING")?;
            let net_number_of_desktops = self.get_atom("_NET_DESKTOP_NAMES")?;
            let cookie = self.x11_conn.change_property8(
                PropMode::REPLACE,
                self.root_window_id(),
                net_number_of_desktops,
                utf8_string,
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
            let net_desktop_layout = self.get_atom("_NET_DESKTOP_LAYOUT")?;
            let cookie = self.x11_conn.change_property32(
                PropMode::REPLACE,
                self.root_window_id(),
                net_desktop_layout,
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

    pub fn update_net_current_desktop(&self, current: u32) {
        let do_update = || -> anyhow::Result<()> {
            let net_current_desktop = self.get_atom("_NET_CURRENT_DESKTOP")?;
            let cookie = self.x11_conn.change_property32(
                PropMode::REPLACE,
                self.root_window_id(),
                net_current_desktop,
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

    pub fn set_net_desktop_viewport(&self) {
        let do_set = || -> anyhow::Result<()> {
            let net_desktop_viewport = self.get_atom("_NET_DESKTOP_VIEWPORT")?;
            let cookie = self.x11_conn.change_property32(
                PropMode::REPLACE,
                self.root_window_id(),
                net_desktop_viewport,
                AtomEnum::CARDINAL,
                &[0, 0],
            )?;
            cookie.check()?;
            Ok(())
        };

        if let Err(err) = do_set() {
            tracing::warn!("Failed to set X11 property for desktop viewport: {err}");
        }
    }
}
