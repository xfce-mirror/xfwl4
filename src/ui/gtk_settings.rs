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

use std::rc::Rc;

use anyhow::anyhow;
use glib::{ObjectExt, SignalHandlerId, ToValue, clone};
use gtk::traits::{CssProviderExt, GtkSettingsExt};
use smithay::reexports::calloop::channel;

use crate::ui::{FromUiMessage, TABWIN_DEFAULT_CSS, TABWIN_WIDGET_NAME, UiThreadState, util::ObjectExtExt};

// This is annoying: we can't send a glib::Value across threads because it contains a raw pointer
// and that's not safe.
#[derive(Debug)]
pub enum GtkSettingsValue {
    String(String),
    Boolean(bool),
    Int32(i32),
}

impl TryFrom<glib::Value> for GtkSettingsValue {
    type Error = anyhow::Error;

    fn try_from(value: glib::Value) -> Result<Self, Self::Error> {
        if let Ok(v) = value.get::<String>() {
            if v.is_empty() {
                Err(anyhow!("Not setting empty-string GtkSetting value"))
            } else {
                Ok(Self::String(v))
            }
        } else if let Ok(v) = value.get::<i32>() {
            Ok(Self::Int32(v))
        } else if let Ok(v) = value.get::<bool>() {
            Ok(Self::Boolean(v))
        } else {
            let err = anyhow!("Unhandled GtkSettings value type {}", value.value_type().name());
            tracing::warn!("{err}");
            Err(err)
        }
    }
}

impl From<GtkSettingsValue> for glib::Value {
    fn from(value: GtkSettingsValue) -> Self {
        match value {
            GtkSettingsValue::String(s) => Self::from(s),
            GtkSettingsValue::Int32(i) => Self::from(i),
            GtkSettingsValue::Boolean(b) => Self::from(b),
        }
    }
}

pub fn init_notifiers(state: Rc<UiThreadState>, from_ui_tx: channel::Sender<FromUiMessage>) -> Vec<SignalHandlerId> {
    let settings = gtk::Settings::default().expect("couldn't get GtkSettings");

    let theme_changed = clone!(@strong from_ui_tx => move |settings: &gtk::Settings| {
        #[allow(irrefutable_let_patterns)]
        if let Some(theme) = settings.property_safe::<String>("gtk-theme-name")
            && let Some(theme_provider) = gtk::CssProvider::named(&theme, None)
                && let css = theme_provider.to_str()
                && css.contains(&format!("#{TABWIN_WIDGET_NAME}"))
        {
            // Current theme has a style for the tabwin, so don't try to override.
            state.tabwin_style_provider.replace(None);
        } else {
            tracing::debug!("creating custom tabwin theme provider");
            let provider = gtk::CssProvider::new();
            provider
                .load_from_data(TABWIN_DEFAULT_CSS.as_bytes())
                .expect("failed to load fallback tabwin css");
            state.tabwin_style_provider.replace(Some(provider));
        }

        let theme_colors = super::theme::fetch_theme_colors();
        if !theme_colors.is_empty() {
            let _ = from_ui_tx.send(FromUiMessage::ThemeColorsChanged(theme_colors));
        }
    });
    theme_changed(&settings);
    let theme_id = settings.connect_gtk_theme_name_notify(theme_changed);

    vec![theme_id]
}

pub fn update_gtk_setting<V: Into<glib::Value>>(property_name: &str, value: V) {
    let settings = gtk::Settings::default().unwrap();
    if settings.has_property(property_name, None) {
        settings.set_property(property_name, value);
    } else {
        tracing::debug!("Got GtkSettings update for unknown property {property_name}");
    }
}
