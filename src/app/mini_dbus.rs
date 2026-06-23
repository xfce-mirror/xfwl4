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
    env,
    os::{
        linux::net::SocketAddrExt,
        unix::net::{SocketAddr as UnixSocketAddr, UnixStream},
    },
    path::PathBuf,
};

enum DBusUnixSocket {
    Path(PathBuf),
    Abstract(String),
}

impl DBusUnixSocket {
    fn connect(&self) -> std::io::Result<UnixStream> {
        match self {
            DBusUnixSocket::Path(path) => UnixStream::connect(path),
            DBusUnixSocket::Abstract(name) => UnixSocketAddr::from_abstract_name(name).and_then(|addr| UnixStream::connect_addr(&addr)),
        }
    }
}

/// Extracts the unix socket from a single D-Bus address (the `path=` or `abstract=` key)
///
/// Percent-encoded values aren't decoded, but bus socket paths don't use characters that require
/// escaping in practice.
fn parse_unix_dbus_socket(address: &str) -> Option<DBusUnixSocket> {
    address.strip_prefix("unix:").and_then(|params| {
        params.split(',').find_map(|param| {
            param
                .strip_prefix("path=")
                .map(|path| DBusUnixSocket::Path(PathBuf::from(path)))
                .or_else(|| {
                    param
                        .strip_prefix("abstract=")
                        .map(|name| DBusUnixSocket::Abstract(name.to_owned()))
                })
        })
    })
}

/// Whether a usable D-Bus session bus is reachable via `DBUS_SESSION_BUS_ADDRESS`
///
/// Connects to the socket directly so we don't pull in any GLib threads; see
/// [`ensure_dbus_session_daemon`].
pub(super) fn session_bus_running() -> bool {
    match env::var("DBUS_SESSION_BUS_ADDRESS") {
        Ok(addresses) => addresses
            .split(';')
            .filter_map(parse_unix_dbus_socket)
            .any(|socket| socket.connect().is_ok()),
        Err(_) => false,
    }
}
