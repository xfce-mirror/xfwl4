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

use glib::{ObjectExt, ToValue};
use xfconf::ChannelExtManual;

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
const PROPERTY_NAME_FIXUPS: &[(&str, &str)] = &[("gtk-xft-hint-style", "gtk-xft-hintstyle")];

#[derive(Debug)]
pub struct GtkSettingsSync(xfconf::Channel);

impl GtkSettingsSync {
    pub fn new() -> Self {
        let sync = Self(xfconf::Channel::new(XSETTINGS_CHANNEL_NAME));

        sync.0.connect_property_changed(None, |_channel, property_name, value| {
            if SYNC_PROPERTIES.contains(&property_name) {
                Self::handle_property_update(property_name, value);
            }
        });

        for (property_name, value) in sync.0.get_properties(None) {
            if SYNC_PROPERTIES.contains(&property_name.as_str()) {
                Self::handle_property_update(property_name.as_str(), &value);
            }
        }

        sync
    }

    fn handle_property_update(property_name: &str, value: &glib::Value) {
        if SYNC_PROPERTIES.contains(&property_name)
            && let Some(gtk_setting_name) = xfconf_property_name_to_gtk_setting_name(property_name)
        {
            let settings = gtk::Settings::default().unwrap();
            if let Some(pspec) = settings.object_class().find_property(property_name) {
                let default_value = pspec.default_value();

                if value.value_type() == glib::Type::INVALID {
                    settings.set_property(&gtk_setting_name, default_value);
                } else {
                    match value.transform_with_type(default_value.value_type()) {
                        Ok(trans_value) => {
                            tracing::debug!("Xfconf property {property_name} changed; updating GTK setting {gtk_setting_name}");
                            settings.set_property(&gtk_setting_name, trans_value);
                        }
                        Err(err) => {
                            tracing::info!("Failed to convert value for GTK setting {gtk_setting_name}: {err}");
                            settings.set_property(&gtk_setting_name, default_value);
                        }
                    }
                }
            } else {
                tracing::debug!("Got GtkSettings update for unknown property {property_name} -> {gtk_setting_name}");
            }
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
    let name = format!("{prefix}{suffix}");

    PROPERTY_NAME_FIXUPS
        .iter()
        .find_map(|(transformed, corrected)| (name == *transformed).then(|| (*corrected).to_owned()))
        .or(Some(name))
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
