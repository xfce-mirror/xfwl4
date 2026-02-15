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

use glib::{SignalHandlerId, clone};
use gtk::{
    cairo,
    traits::{CssProviderExt, GtkSettingsExt},
};
use smithay::reexports::calloop::channel;

use crate::ui::{FromUiMessage, TABWIN_DEFAULT_CSS, TABWIN_WIDGET_NAME, ToUiMessageState, util::ObjectExtExt};

#[derive(Debug, Clone)]
pub struct FontSettings {
    pub hint_style: cairo::HintStyle,
    pub subpixel_order: cairo::SubpixelOrder,
    pub antialias: cairo::Antialias,
}

pub fn init_notifiers(state: Rc<ToUiMessageState>, from_ui_tx: channel::Sender<FromUiMessage>) -> Vec<SignalHandlerId> {
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

    let icon_theme_changed = clone!(@strong from_ui_tx => move |settings: &gtk::Settings| {
        let icon_theme = settings
            .gtk_icon_theme_name()
            .map(|theme| theme.to_string())
            .unwrap_or_else(|| "hicolor".to_owned());
        from_ui_tx.send(FromUiMessage::IconThemeChanged(icon_theme)).unwrap();
    });
    icon_theme_changed(&settings);
    let icon_theme_id = settings.connect_gtk_icon_theme_name_notify(icon_theme_changed);

    let font_settings_changed = clone!(@strong from_ui_tx => move |settings: &gtk::Settings| {
        let antialias = settings.gtk_xft_antialias();
        let hinting = settings.gtk_xft_hinting();
        let hintstyle = settings.gtk_xft_hintstyle();
        let subpixel = settings.gtk_xft_rgba(); // TODO: wl_output supports per-monitor for this

        let subpixel_order = match subpixel.as_deref() {
            Some("rgb") => cairo::SubpixelOrder::Rgb,
            Some("bgr") => cairo::SubpixelOrder::Bgr,
            Some("vrgb") => cairo::SubpixelOrder::Vrgb,
            Some("vbgr") => cairo::SubpixelOrder::Vbgr,
            _ => cairo::SubpixelOrder::Default,
        };

        let settings = FontSettings {
            hint_style: if hinting == 0 {
                cairo::HintStyle::None
            } else if hinting == 1 {
                match hintstyle.as_deref() {
                    Some("hintnone") => cairo::HintStyle::None,
                    Some("hintslight") => cairo::HintStyle::Slight,
                    Some("hintmedium") => cairo::HintStyle::Medium,
                    Some("hintfull") => cairo::HintStyle::Full,
                    _ => cairo::HintStyle::Default,
                }
            } else {
                cairo::HintStyle::Default
            },

            subpixel_order,

            antialias: if antialias == 0 {
                cairo::Antialias::None
            } else if antialias == 1 {
                if subpixel_order != cairo::SubpixelOrder::Default {
                    cairo::Antialias::Subpixel
                } else {
                    cairo::Antialias::Gray
                }
            } else {
                cairo::Antialias::Default
            },
        };

        let _ = from_ui_tx.send(FromUiMessage::FontSettingsChanged(settings));
    });
    font_settings_changed(&settings);
    let antialias_id = settings.connect_gtk_xft_antialias_notify(font_settings_changed.clone());
    let hinting_id = settings.connect_gtk_xft_hinting_notify(font_settings_changed.clone());
    let hintstyle_id = settings.connect_gtk_xft_hintstyle_notify(font_settings_changed.clone());
    let subpixel_id = settings.connect_gtk_xft_rgba_notify(font_settings_changed);

    vec![theme_id, icon_theme_id, antialias_id, hinting_id, hintstyle_id, subpixel_id]
}
