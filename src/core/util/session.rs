// xfwl4 -- Wayland compositor for the Xfce Desktop Environment
//
// Copyright (C) 2026 Andre Miranda <andreldm@xfce.org>
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

use glib::ToVariant;
use gtk::gio::{self, BusType, DBusCallFlags, DBusProxyFlags, traits::DBusProxyExt};
use tracing::error;

const NO_CANCELLABLE: Option<&gio::Cancellable> = None::<&gio::Cancellable>;

/// Ask xfce4-session to log the user out (shows the logout dialog).
///
/// This mirrors what xfwm4 does on X11 via libxfce4ui's session client, but talks
/// to xfce4-session directly over D-Bus since we have no ICE/libSM under Wayland.
/// `Logout(show_dialog = true, allow_save = true)` is the D-Bus equivalent of
/// xfwm4's libsm call (interactive shutdown prompt, session saved).
///
/// Fire-and-forget: the call is issued asynchronously and we don't block the
/// compositor's main loop waiting for a reply. xfce4-session schedules the
/// actual logout on an idle callback and returns immediately regardless, and it
/// only honors the request once it has reached its idle state (i.e. once startup
/// has completed).
pub fn request_logout() {
    let bus = match gio::bus_get_sync(BusType::Session, NO_CANCELLABLE) {
        Ok(bus) => bus,
        Err(err) => {
            error!("Failed to connect to the session bus to request logout: {err}");
            return;
        }
    };
    let proxy = match gio::DBusProxy::new_sync(
        &bus,
        DBusProxyFlags::DO_NOT_CONNECT_SIGNALS | DBusProxyFlags::DO_NOT_LOAD_PROPERTIES,
        None,
        Some("org.xfce.SessionManager"),
        "/org/xfce/SessionManager",
        "org.xfce.Session.Manager",
        NO_CANCELLABLE,
    ) {
        Ok(proxy) => proxy,
        Err(err) => {
            error!("Failed to create xfce4-session proxy to request logout: {err}");
            return;
        }
    };
    proxy.call(
        "Logout",
        Some(&(true, true).to_variant()),
        DBusCallFlags::NONE,
        -1,
        NO_CANCELLABLE,
        |res| {
            if let Err(err) = res {
                error!("Failed to request logout from xfce4-session: {err}");
            }
        },
    );
}
