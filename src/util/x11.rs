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

use x11rb::{
    connection::Connection,
    protocol::xproto::{Atom, AtomEnum, ConnectionExt, PropMode, Window},
};

pub fn get_atom<C: Connection>(x11_conn: &C, name: &[u8]) -> Option<Atom> {
    x11_conn
        .intern_atom(false, name)
        .inspect_err(|err| tracing::warn!("Failed to send X11 InternAtom request: {err}"))
        .ok()
        .and_then(|cookie| {
            cookie
                .reply()
                .inspect_err(|err| tracing::warn!("Failed to receive X11 InternAtom reply: {err}"))
                .ok()
        })
        .map(|reply| Atom::from(reply.atom))
}

pub fn update_net_wm_state<C: Connection + x11rb::wrapper::ConnectionExt>(x11_conn: &C, window_id: Window, add: &[Atom], remove: &[Atom]) {
    if let Some(net_wm_state) = get_atom(x11_conn, b"_NET_WM_STATE")
        && let Some(mut state_atoms) = x11_conn
            .get_property(false, window_id, net_wm_state, AtomEnum::ATOM, 0, u32::MAX)
            .inspect_err(|err| tracing::warn!("Failed to get _NET_WM_STATE for window {window_id}: {err}"))
            .ok()
            .and_then(|cookie| {
                cookie
                    .reply()
                    .inspect_err(|err| tracing::warn!("Failed to get _NET_WM_STATE for window {window_id}: {err}"))
                    .ok()
                    .map(|reply| reply.value32().map(|iter| iter.collect::<Vec<_>>()).unwrap_or_default())
            })
    {
        state_atoms.retain(|atom| !remove.contains(atom));
        state_atoms.extend_from_slice(add);
        if let Err(err) = x11_conn.change_property32(PropMode::REPLACE, window_id, net_wm_state, AtomEnum::ATOM, &state_atoms) {
            tracing::warn!("Failed to update _NET_WM_STATE for window {window_id}: {err}");
        }
    }
}
