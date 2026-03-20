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

use std::{fs, time::Duration};

use glib::ToVariant;
use gtk::gio::{self, BusType, DBusCallFlags, DBusProxyFlags, traits::DBusProxyExt};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LaptopLidState {
    Open,
    Closed,
}

const DBUS_CALL_TIMEOUT: Duration = Duration::from_millis(50);
const NO_CANCELLABLE: Option<&gio::Cancellable> = None::<&gio::Cancellable>;

pub fn is_laptop_display_name(connector_name: &str) -> bool {
    connector_name.starts_with("eDP") || connector_name.starts_with("LVDS") || connector_name == "PANEL"
}

pub fn get_laptop_lid_state() -> Option<LaptopLidState> {
    acpi_lid_state().or_else(|| {
        let bus = gio::bus_get_sync(BusType::System, NO_CANCELLABLE).ok()?;
        upower_lid_state(&bus).or_else(|| logind_lid_state(&bus))
    })
}

fn acpi_lid_state() -> Option<LaptopLidState> {
    fs::read_to_string("/proc/acpi/button/lid/LID0/state")
        .or_else(|_| fs::read_to_string("/proc/acpi/button/lid/LID/state"))
        .ok()
        .map(|s| {
            if s.contains("open") {
                LaptopLidState::Open
            } else {
                LaptopLidState::Closed
            }
        })
}

fn upower_lid_state(bus: &gio::DBusConnection) -> Option<LaptopLidState> {
    let proxy = gio::DBusProxy::new_sync(
        bus,
        DBusProxyFlags::DO_NOT_CONNECT_SIGNALS | DBusProxyFlags::DO_NOT_LOAD_PROPERTIES,
        None,
        Some("org.freedesktop.UPower"),
        "/org/freedesktop/UPower",
        "org.freedesktop.DBus.Properties",
        NO_CANCELLABLE,
    )
    .ok()?;

    let has_lid = proxy
        .call_sync(
            "Get",
            Some(&("org.freedesktop.UPower", "LidIsPresent").to_variant()),
            DBusCallFlags::NONE,
            DBUS_CALL_TIMEOUT.as_millis() as i32,
            NO_CANCELLABLE,
        )
        .ok()?;

    if dbus_prop_get_return_value::<bool>(has_lid).unwrap_or(false) {
        let lid_closed = proxy
            .call_sync(
                "Get",
                Some(&("org.freedesktop.UPower", "LidIsClosed").to_variant()),
                DBusCallFlags::NONE,
                DBUS_CALL_TIMEOUT.as_millis() as i32,
                NO_CANCELLABLE,
            )
            .ok()?;
        dbus_prop_get_return_value::<bool>(lid_closed).map(
            |is_closed| {
                if is_closed { LaptopLidState::Closed } else { LaptopLidState::Open }
            },
        )
    } else {
        None
    }
}

fn logind_lid_state(bus: &gio::DBusConnection) -> Option<LaptopLidState> {
    let proxy = gio::DBusProxy::new_sync(
        bus,
        DBusProxyFlags::DO_NOT_CONNECT_SIGNALS | DBusProxyFlags::DO_NOT_LOAD_PROPERTIES,
        None,
        Some("org.freedesktop.login1"),
        "/org/freedesktop/login1",
        "org.freedesktop.DBus.Properties",
        NO_CANCELLABLE,
    )
    .ok()?;

    let lid_closed = proxy
        .call_sync(
            "Get",
            Some(&("org.freedesktop.login1.Manager", "LidClosed").to_variant()),
            DBusCallFlags::NONE,
            DBUS_CALL_TIMEOUT.as_millis() as i32,
            NO_CANCELLABLE,
        )
        .ok()?;
    dbus_prop_get_return_value::<bool>(lid_closed).map(|is_closed| if is_closed { LaptopLidState::Closed } else { LaptopLidState::Open })
}

fn dbus_prop_get_return_value<T: glib::FromVariant>(variant: glib::Variant) -> Option<T> {
    variant.child_value(0).get::<glib::Variant>().and_then(|v| v.get::<T>())
}
