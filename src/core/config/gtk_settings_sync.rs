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

use glib::Sender;
use smithay::reexports::calloop::LoopHandle;
use xfconf::ChannelExtManual;

use crate::{
    backend::Backend,
    core::{state::Xfwl4State, util::CalloopXfconfSource},
    ui::{GtkSettingsValue, ToUiMessage},
};

const XSETTINGS_CHANNEL_NAME: &str = "xsettings";

const SYNC_PROPERTIES: &[&str] = &[
    "/Gtk/ButtonImages",
    "/Gtk/CanChangeAccels",
    "/Gtk/ColorPalette",
    "/Gtk/CursorThemeName",
    "/Gtk/CursorThemeSize",
    "/Gtk/DecorationLayout",
    "/Gtk/DialogsUseHeader",
    "/Gtk/FontName",
    "/Gtk/IconSizes",
    "/Gtk/KeyThemeName",
    "/Gtk/MenuBarAccel",
    "/Gtk/MenuImages",
    "/Gtk/Modules",
    "/Gtk/TitlebarMiddleClick",
    "/Net/CursorBlink",
    "/Net/CursorBlinkTime",
    "/Net/DndDragThreshold",
    "/Net/DoubleClickDistance",
    "/Net/DoubleClickTime",
    "/Net/EnableEventSounds",
    "/Net/EnableInputFeedbackSounds",
    "/Net/IconThemeName",
    "/Net/SoundThemeName",
    "/Net/ThemeName",
    "/Xft/Antialias",
    "/Xft/HintStyle",
    "/Xft/Hinting",
    "/Xft/RGBA",
];

#[derive(Debug)]
pub struct GtkSettingsSync(xfconf::Channel);

impl GtkSettingsSync {
    pub fn new<BackendData: Backend + 'static>(handle: LoopHandle<'_, Xfwl4State<BackendData>>) -> Self {
        let channel = xfconf::Channel::new(XSETTINGS_CHANNEL_NAME);

        let source = CalloopXfconfSource::new(channel.clone(), SYNC_PROPERTIES.iter().copied());
        handle
            .insert_source(source, |(property_name, value), _, state| {
                state
                    .core
                    .gtk_settings_sync
                    .handle_property_update(&state.core.to_ui_channel_tx, &property_name, value);
            })
            .expect("Failed to insert GtkSettingsSync source into event loop");

        Self(channel)
    }

    pub fn sync(&self, to_ui_tx: &Sender<ToUiMessage>) {
        for (property_name, value) in self.0.get_properties(None) {
            if SYNC_PROPERTIES.contains(&property_name.as_str()) {
                self.handle_property_update(to_ui_tx, property_name.as_str(), value);
            }
        }
    }

    fn handle_property_update(&self, to_ui_tx: &Sender<ToUiMessage>, property_name: &str, value: glib::Value) {
        if let Some(gtk_setting_name) = xfconf_property_name_to_gtk_setting_name(property_name)
            && let Ok(value) = GtkSettingsValue::try_from(value)
        {
            let _ = to_ui_tx.send(ToUiMessage::GtkSettingChanged(gtk_setting_name, value));
        }
    }
}

fn xfconf_property_name_to_gtk_setting_name(xfconf_property_name: &str) -> Option<String> {
    let (prefix, rest) = if let Some(rest) = xfconf_property_name.strip_prefix("/Gtk/") {
        Some(("gtk-", rest))
    } else if let Some(rest) = xfconf_property_name.strip_prefix("/Net/") {
        Some(("gtk-", rest))
    } else if let Some(rest) = xfconf_property_name.strip_prefix("/Xft/") {
        Some(("gtk-xft-", rest))
    } else {
        tracing::debug!("Unhandled GtkSettings sync property from xfconf: {xfconf_property_name}");
        None
    }?;

    let suffix = title_to_kebab(rest);
    let suffix = if suffix == "hint-style" {
        // Special case...
        "hintstyle".to_owned()
    } else {
        suffix
    };

    Some(format!("{prefix}{suffix}"))
}

fn title_to_kebab(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    chars
        .iter()
        .enumerate()
        .flat_map(|(i, &c)| {
            // Don't add the '-' separator if a) it's the first character; or b) the previous
            // character was also an uppercase letter, unless the next character is lowercase.
            let needs_sep = i > 0
                && c.is_ascii_uppercase()
                && (chars.get(i - 1).is_some_and(|p| p.is_ascii_lowercase()) || chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase()));

            if needs_sep {
                vec!['-', c.to_ascii_lowercase()]
            } else if c.is_ascii_uppercase() {
                vec![c.to_ascii_lowercase()]
            } else {
                vec![c]
            }
        })
        .collect()
}
