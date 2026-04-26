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

use std::time::Duration;

use gtk::cairo;
use smithay::reexports::calloop::LoopHandle;
use xfconf::ChannelExtManual;

use crate::{
    backend::Backend,
    core::{state::Xfwl4State, util::CalloopXfconfSource},
};

const XSETTINGS_CHANNEL_NAME: &str = "xsettings";

const PROP_ICON_THEME_NAME: &str = "/Net/IconThemeName";
const PROP_FONT_HINTING_ENABLED: &str = "/Xft/Hinting";
const PROP_FONT_HINT_STYLE: &str = "/Xft/HintStyle";
const PROP_FONT_SUBPIXEL_ORDER: &str = "/Xft/RGBA";
const PROP_FONT_ANTIALIAS_ENABLED: &str = "/Xft/Antialias";
#[cfg(feature = "xwayland")]
const PROP_FONT_DPI: &str = "/Xft/DPI";
const PROP_DND_DRAG_THRESHOLD: &str = "/Net/DndDragThreshold";
const PROP_DOUBLE_CLICK_DISTANCE: &str = "/Net/DoubleClickDistance";
const PROP_DOUBLE_CLICK_TIME: &str = "/Net/DoubleClickTime";

// This is bad choice: 'hicolor' is not a real theme.
const FALLBACK_ICON_THEME_NAME: &str = "hicolor";
#[cfg(feature = "xwayland")]
const DEFAULT_FONT_DPI: i32 = 96;
const DEFAULT_DND_DRAG_THRESHOLD: i32 = 8;
const DEFAULT_DOUBLE_CLICK_DISTANCE: f64 = 5.;
const DEFAULT_DOUBLE_CLICK_TIME: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub struct UiSettings(xfconf::Channel);

impl UiSettings {
    pub fn new<BackendData: Backend + 'static>(handle: LoopHandle<'_, Xfwl4State<BackendData>>) -> Self {
        let channel = xfconf::Channel::new(XSETTINGS_CHANNEL_NAME);
        let settings = Self(channel.clone());

        let source = CalloopXfconfSource::new(
            channel,
            [
                PROP_ICON_THEME_NAME,
                PROP_FONT_HINTING_ENABLED,
                PROP_FONT_HINT_STYLE,
                PROP_FONT_SUBPIXEL_ORDER,
                PROP_FONT_ANTIALIAS_ENABLED,
                #[cfg(feature = "xwayland")]
                PROP_FONT_DPI,
                PROP_DND_DRAG_THRESHOLD,
                PROP_DOUBLE_CLICK_DISTANCE,
                PROP_DOUBLE_CLICK_TIME,
            ],
        );
        handle
            .insert_source(source, |(property_name, value), _, state| {
                state.handle_ui_settings_property_changed(&property_name, value);
            })
            .expect("Failed to insert xfconf UI settings source");

        settings
    }

    pub fn icon_theme_name(&self) -> String {
        self.0
            .get_property::<String>(PROP_ICON_THEME_NAME)
            .unwrap_or_else(|| FALLBACK_ICON_THEME_NAME.to_owned())
    }

    pub fn hint_style(&self) -> cairo::HintStyle {
        let hinting = self.0.get_property::<i32>(PROP_FONT_HINTING_ENABLED);
        let hint_style = self.0.get_property::<String>(PROP_FONT_HINT_STYLE);
        parse_hint_style(hinting, hint_style)
    }

    pub fn antialias(&self) -> cairo::Antialias {
        let antialias = self.0.get_property::<i32>(PROP_FONT_ANTIALIAS_ENABLED);
        let subpixel_order = parse_subpixel_order(self.0.get_property::<String>(PROP_FONT_SUBPIXEL_ORDER));
        parse_antialias(antialias, subpixel_order)
    }

    pub fn subpixel_order(&self) -> cairo::SubpixelOrder {
        parse_subpixel_order(self.0.get_property::<String>(PROP_FONT_SUBPIXEL_ORDER))
    }

    #[cfg(feature = "xwayland")]
    pub fn font_dpi(&self) -> i32 {
        self.0
            .get_property::<i32>(PROP_FONT_DPI)
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_FONT_DPI)
    }

    pub fn dnd_drag_threshold(&self) -> i32 {
        self.0
            .get_property::<i32>(PROP_DND_DRAG_THRESHOLD)
            .unwrap_or(DEFAULT_DND_DRAG_THRESHOLD)
    }

    pub fn double_click_distance(&self) -> f64 {
        self.0
            .get_property::<i32>(PROP_DOUBLE_CLICK_DISTANCE)
            .filter(|v| *v > 0)
            .map(|v| v as f64)
            .unwrap_or(DEFAULT_DOUBLE_CLICK_DISTANCE)
    }

    pub fn double_click_time(&self) -> Duration {
        self.0
            .get_property::<i32>(PROP_DOUBLE_CLICK_TIME)
            .filter(|v| *v > 0)
            .map(|v| Duration::from_millis(v as u64))
            .unwrap_or(DEFAULT_DOUBLE_CLICK_TIME)
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    fn handle_ui_settings_property_changed(&mut self, property_name: &str, value: glib::Value) {
        match property_name {
            PROP_ICON_THEME_NAME => {
                if let Ok(icon_theme_name) = value.get::<String>() {
                    self.core.icon_theme.set_icon_theme_name(&icon_theme_name);
                    self.update_window_decorations_icon_theme();
                }
            }

            PROP_FONT_HINTING_ENABLED | PROP_FONT_HINT_STYLE => {
                let hinting = self.core.ui_settings.0.get_property(PROP_FONT_HINTING_ENABLED);
                let hint_style = self.core.ui_settings.0.get_property::<String>(PROP_FONT_HINT_STYLE);
                let hint_style = parse_hint_style(hinting, hint_style);
                if hint_style != self.core.font_options.hint_style() {
                    self.core.font_options.set_hint_style(hint_style);
                    self.update_window_decorations_font_options();
                    #[cfg(feature = "xwayland")]
                    self.x11_update_xrm_xft();
                }
            }

            PROP_FONT_SUBPIXEL_ORDER => {
                let subpixel_order = parse_subpixel_order(value.get::<String>().ok());
                let antialias = self.core.ui_settings.0.get_property::<i32>(PROP_FONT_ANTIALIAS_ENABLED);
                let antialias = parse_antialias(antialias, subpixel_order);
                if subpixel_order != self.core.font_options.subpixel_order() || antialias != self.core.font_options.antialias() {
                    self.core.font_options.set_subpixel_order(subpixel_order);
                    self.core.font_options.set_antialias(antialias);
                    self.update_window_decorations_font_options();
                    #[cfg(feature = "xwayland")]
                    self.x11_update_xrm_xft();
                }
            }

            PROP_FONT_ANTIALIAS_ENABLED => {
                let subpixel_order = self.core.ui_settings.0.get_property::<String>(PROP_FONT_SUBPIXEL_ORDER);
                let subpixel_order = parse_subpixel_order(subpixel_order);
                let antialias = parse_antialias(value.get::<i32>().ok(), subpixel_order);
                if antialias != self.core.font_options.antialias() || subpixel_order != self.core.font_options.subpixel_order() {
                    self.core.font_options.set_antialias(antialias);
                    self.core.font_options.set_subpixel_order(subpixel_order);
                    self.update_window_decorations_font_options();
                    #[cfg(feature = "xwayland")]
                    self.x11_update_xrm_xft();
                }
            }

            #[cfg(feature = "xwayland")]
            PROP_FONT_DPI => {
                self.x11_update_xrm_xft();
            }

            PROP_DND_DRAG_THRESHOLD => {
                self.core.dnd_drag_threshold = value.get::<i32>().unwrap_or(DEFAULT_DND_DRAG_THRESHOLD);
            }

            PROP_DOUBLE_CLICK_DISTANCE => {
                self.core.double_click_distance = value
                    .get::<i32>()
                    .ok()
                    .filter(|v| *v > 0)
                    .map(|v| v as f64)
                    .unwrap_or(DEFAULT_DOUBLE_CLICK_DISTANCE);
            }

            PROP_DOUBLE_CLICK_TIME => {
                self.core.double_click_time = value
                    .get::<i32>()
                    .ok()
                    .filter(|v| *v > 0)
                    .map(|v| Duration::from_millis(v as u64))
                    .unwrap_or(DEFAULT_DOUBLE_CLICK_TIME);
            }

            _ => (),
        }
    }
}

fn parse_hint_style(hinting: Option<i32>, hint_style: Option<String>) -> cairo::HintStyle {
    match hinting {
        Some(0) => cairo::HintStyle::None,
        Some(1) => match hint_style.as_deref() {
            Some("hintnone") => cairo::HintStyle::None,
            Some("hintslight") => cairo::HintStyle::Slight,
            Some("hintmedium") => cairo::HintStyle::Medium,
            Some("hintfull") => cairo::HintStyle::Full,
            _ => cairo::HintStyle::Default,
        },
        _ => cairo::HintStyle::Default,
    }
}

fn parse_subpixel_order(value: Option<String>) -> cairo::SubpixelOrder {
    match value.as_deref() {
        Some("rgb") => cairo::SubpixelOrder::Rgb,
        Some("bgr") => cairo::SubpixelOrder::Bgr,
        Some("vrgb") => cairo::SubpixelOrder::Vrgb,
        Some("vbgr") => cairo::SubpixelOrder::Vbgr,
        _ => cairo::SubpixelOrder::Default,
    }
}

fn parse_antialias(antialias: Option<i32>, subpixel_order: cairo::SubpixelOrder) -> cairo::Antialias {
    match antialias {
        Some(0) => cairo::Antialias::None,
        Some(1) => {
            if subpixel_order != cairo::SubpixelOrder::Default {
                cairo::Antialias::Subpixel
            } else {
                cairo::Antialias::Gray
            }
        }
        _ => cairo::Antialias::Default,
    }
}
