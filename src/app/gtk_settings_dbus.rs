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

use zbus::fdo::RequestNameFlags;

use crate::app::zbus_ext::ZBusAdapter;

const ORG_GTK_SETTINGS_DBUS_NAME: &str = "org.gtk.Settings";
const ORG_GTK_SETTINGS_DBUS_PATH: &str = "/org/gtk/Settings";

struct OrgGtkSettings;

#[zbus::interface(name = "org.gtk.Settings")]
impl OrgGtkSettings {
    #[zbus(property, name = "Modules")]
    fn modules(&self) -> &'static str {
        "xfsettingsd-gtk-settings-sync"
    }
}

pub fn start(adapter: &ZBusAdapter) {
    let conn = adapter.connection().clone();
    adapter.schedule(async move {
        if let Err(err) = conn
            .request_name_with_flags(
                ORG_GTK_SETTINGS_DBUS_NAME,
                RequestNameFlags::ReplaceExisting | RequestNameFlags::DoNotQueue,
            )
            .await
        {
            tracing::error!("Failed to acquire DBus bus name {ORG_GTK_SETTINGS_DBUS_NAME}: {err}");
        } else if let Err(err) = conn.object_server().at(ORG_GTK_SETTINGS_DBUS_PATH, OrgGtkSettings).await {
            tracing::error!("Failed to register {ORG_GTK_SETTINGS_DBUS_NAME} DBus interface: {err}");
        } else {
            tracing::info!("Registered {ORG_GTK_SETTINGS_DBUS_NAME} at {ORG_GTK_SETTINGS_DBUS_PATH}");
        }
    });
}
