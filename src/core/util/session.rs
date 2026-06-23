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

use std::cell::Cell;

use glib::ToVariant;
use gtk::gio::{self, BusType, DBusCallFlags, DBusProxyFlags, traits::DBusProxyExt};
use tracing::error;

thread_local! {
    // The compositor runs on a single thread, so a thread-local flag is enough
    // to coalesce logout requests: it guards against a rapid double-press of the
    // shortcut queuing two D-Bus messages before the first one completes.
    static LOGOUT_PENDING: Cell<bool> = const { Cell::new(false) };
}

/// Ask xfce4-session to log the user out (shows the logout dialog).
///
/// This mirrors what xfwm4 does on X11 via libxfce4ui's session client, but talks
/// to xfce4-session directly over D-Bus since we have no ICE/libSM under Wayland.
/// `Logout(show_dialog = true, allow_save = true)` is the D-Bus equivalent of
/// xfwm4's libsm call (interactive shutdown prompt, session saved).
///
/// The whole exchange is asynchronous so it never blocks the compositor's main
/// loop, and is driven by the GMainContext iteration installed in `main()`.
/// xfce4-session only honors the request once it has reached its idle state (i.e.
/// once startup has completed).
pub fn request_logout() {
    // Ignore the request if one is already in flight.
    if LOGOUT_PENDING.with(|pending| pending.replace(true)) {
        return;
    }

    glib::spawn_future_local(async move {
        if let Err(err) = try_request_logout().await {
            error!("Failed to request logout from xfce4-session: {err}");
        }
        LOGOUT_PENDING.with(|pending| pending.set(false));
    });
}

async fn try_request_logout() -> Result<(), glib::Error> {
    let bus = gio::bus_get_future(BusType::Session).await?;
    let proxy = gio::DBusProxy::new_future(
        &bus,
        DBusProxyFlags::DO_NOT_CONNECT_SIGNALS | DBusProxyFlags::DO_NOT_LOAD_PROPERTIES,
        None,
        Some("org.xfce.SessionManager"),
        "/org/xfce/SessionManager",
        "org.xfce.Session.Manager",
    )
    .await?;
    // Logout(show_dialog = true, allow_save = true)
    proxy
        .call_future("Logout", Some(&(true, true).to_variant()), DBusCallFlags::NONE, -1)
        .await?;
    Ok(())
}
