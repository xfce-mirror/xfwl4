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

use std::str::FromStr;

use anyhow::anyhow;
use smithay::{reexports::calloop::LoopHandle, xwayland::xwm::settings::Value as XwmValue};
use xfconf::ChannelExtManual;

use crate::{
    backend::Backend,
    core::{state::Xfwl4State, util::CalloopXfconfSource},
};

const XSETTINGS_CHANNEL_NAME: &str = "xsettings";
const XSETTINGS_FROM_XFCONF: &[XSetting] = &[
    XSetting::NetDoubleClickTime,
    XSetting::NetDoubleClickDistance,
    XSetting::NetDndDragThreshold,
    XSetting::NetCursorBlink,
    XSetting::NetCursorBlinkTime,
    XSetting::NetThemeName,
    XSetting::NetIconThemeName,
    XSetting::NetSoundThemeName,
    XSetting::NetEnableEventSounds,
    XSetting::NetEnableInputFeedbackSounds,
    XSetting::GtkCanChangeAccels,
    XSetting::GtkColorPalette,
    XSetting::GtkFontName,
    XSetting::GtkMonospaceFontName,
    XSetting::GtkIconSizes,
    XSetting::GtkKeyThemeName,
    XSetting::GtkToolbarStyle,
    XSetting::GtkToolbarIconSize,
    XSetting::GtkImPreeditStyle,
    XSetting::GtkImStatusStyle,
    XSetting::GtkMenuImages,
    XSetting::GtkButtonImages,
    XSetting::GtkMenuBarAccel,
    XSetting::GtkCursorThemeName,
    XSetting::GtkDecorationLayout,
    XSetting::GtkDialogsUseHeader,
    XSetting::GtkModules,
    XSetting::GtkTitlebarMiddleClick,
    XSetting::XftAntialias,
    XSetting::XftHinting,
    XSetting::XftHintStyle,
    XSetting::XftRgba,
    // These should explicitly not be set blindly from xfconf as they need special handling that
    // depends on xfwl4's output configuration.
    //XSetting::GdkUnscaledDPI,
    //XSetting::GdkWindowScalingFactor,
    //XSetting::GtkCursorThemeSize,
    //XSetting::XftDpi,
];
const DEFAULT_CURSOR_THEME_SIZE: i32 = 24;

pub struct XSettingsManager(xfconf::Channel);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XSetting {
    NetDoubleClickTime,
    NetDoubleClickDistance,
    NetDndDragThreshold,
    NetCursorBlink,
    NetCursorBlinkTime,
    NetThemeName,
    NetIconThemeName,
    NetSoundThemeName,
    NetEnableEventSounds,
    NetEnableInputFeedbackSounds,
    GtkCanChangeAccels,
    GtkColorPalette,
    GtkFontName,
    GtkMonospaceFontName,
    GtkIconSizes,
    GtkKeyThemeName,
    GtkToolbarStyle,
    GtkToolbarIconSize,
    GtkImPreeditStyle,
    GtkImStatusStyle,
    GtkMenuImages,
    GtkButtonImages,
    GtkMenuBarAccel,
    GtkCursorThemeName,
    GtkCursorThemeSize,
    GtkDecorationLayout,
    GtkDialogsUseHeader,
    GtkModules,
    GtkTitlebarMiddleClick,
    XftAntialias,
    XftHinting,
    XftHintStyle,
    XftRgba,
    XftDpi,
    GdkUnscaledDPI,
    GdkWindowScalingFactor,
}

impl XSetting {
    pub fn name(&self) -> &'static str {
        self.xfconf_property_name().strip_prefix('/').unwrap()
    }

    pub fn xfconf_property_name(&self) -> &'static str {
        match self {
            Self::NetDoubleClickTime => "/Net/DoubleClickTime",
            Self::NetDoubleClickDistance => "/Net/DoubleClickDistance",
            Self::NetDndDragThreshold => "/Net/DndDragThreshold",
            Self::NetCursorBlink => "/Net/CursorBlink",
            Self::NetCursorBlinkTime => "/Net/CursorBlinkTime",
            Self::NetThemeName => "/Net/ThemeName",
            Self::NetIconThemeName => "/Net/IconThemeName",
            Self::NetSoundThemeName => "/Net/SoundThemeName",
            Self::NetEnableEventSounds => "/Net/EnableEventSounds",
            Self::NetEnableInputFeedbackSounds => "/Net/EnableInputFeedbackSounds",
            Self::GtkCanChangeAccels => "/Gtk/CanChangeAccels",
            Self::GtkColorPalette => "/Gtk/ColorPalette",
            Self::GtkFontName => "/Gtk/FontName",
            Self::GtkMonospaceFontName => "/Gtk/MonospaceFontName",
            Self::GtkIconSizes => "/Gtk/IconSizes",
            Self::GtkKeyThemeName => "/Gtk/KeyThemeName",
            Self::GtkToolbarStyle => "/Gtk/ToolbarStyle",
            Self::GtkToolbarIconSize => "/Gtk/ToolbarIconSize",
            Self::GtkImPreeditStyle => "/Gtk/IMPreeditStyle",
            Self::GtkImStatusStyle => "/Gtk/IMStatusStyle",
            Self::GtkMenuImages => "/Gtk/MenuImages",
            Self::GtkButtonImages => "/Gtk/ButtonImages",
            Self::GtkMenuBarAccel => "/Gtk/MenuBarAccel",
            Self::GtkCursorThemeName => "/Gtk/CursorThemeName",
            Self::GtkCursorThemeSize => "/Gtk/CursorThemeSize",
            Self::GtkDecorationLayout => "/Gtk/DecorationLayout",
            Self::GtkDialogsUseHeader => "/Gtk/DialogsUseHeader",
            Self::GtkModules => "/Gtk/Modules",
            Self::GtkTitlebarMiddleClick => "/Gtk/TitlebarMiddleClick",
            Self::XftAntialias => "/Xft/Antialias",
            Self::XftHinting => "/Xft/Hinting",
            Self::XftHintStyle => "/Xft/HintStyle",
            Self::XftRgba => "/Xft/RGBA",
            Self::XftDpi => "/Xft/DPI",
            Self::GdkUnscaledDPI => "/Gdk/UnscaledDPI",
            Self::GdkWindowScalingFactor => "/Gdk/WindowScalingFactor",
        }
    }

    pub fn to_xwm_value(self, value: glib::Value) -> Option<XwmValue> {
        match self {
            Self::NetDoubleClickTime
            | Self::NetDoubleClickDistance
            | Self::NetDndDragThreshold
            | Self::NetCursorBlink
            | Self::NetCursorBlinkTime
            | Self::NetEnableEventSounds
            | Self::NetEnableInputFeedbackSounds
            | Self::GtkCanChangeAccels
            | Self::GtkToolbarIconSize
            | Self::GtkMenuImages
            | Self::GtkButtonImages
            | Self::GtkDialogsUseHeader
            | Self::XftAntialias
            | Self::XftHinting => glib_value_to_i32(value).map(XwmValue::Integer),

            Self::NetThemeName
            | Self::NetIconThemeName
            | Self::NetSoundThemeName
            | Self::GtkColorPalette
            | Self::GtkFontName
            | Self::GtkMonospaceFontName
            | Self::GtkIconSizes
            | Self::GtkKeyThemeName
            | Self::GtkToolbarStyle
            | Self::GtkImPreeditStyle
            | Self::GtkImStatusStyle
            | Self::GtkMenuBarAccel
            | Self::GtkCursorThemeName
            | Self::GtkDecorationLayout
            | Self::GtkModules
            | Self::GtkTitlebarMiddleClick
            | Self::XftHintStyle
            | Self::XftRgba => value.get::<String>().ok().map(XwmValue::String),

            // Shouldn't be updated from xfconf.
            Self::GdkUnscaledDPI => None,
            Self::GdkWindowScalingFactor => None,
            Self::GtkCursorThemeSize => None,
            Self::XftDpi => None,
        }
    }
}

impl FromStr for XSetting {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        XSETTINGS_FROM_XFCONF
            .iter()
            .copied()
            .find(|xsetting| xsetting.xfconf_property_name() == s)
            .ok_or_else(|| anyhow!("Unknown xfconf property '{s}' for XSetting"))
    }
}

impl XSettingsManager {
    pub fn new<BackendData: Backend + 'static>(handle: LoopHandle<'_, Xfwl4State<BackendData>>) -> Self {
        let channel = xfconf::Channel::new(XSETTINGS_CHANNEL_NAME);
        let property_names = XSETTINGS_FROM_XFCONF.iter().map(|xsetting| xsetting.xfconf_property_name());
        let source = CalloopXfconfSource::new(channel.clone(), property_names);

        handle
            .insert_source(source, |(property_name, property_value), _, state| {
                if let Err(err) = property_name.parse::<XSetting>().and_then(|xsetting| match xsetting {
                    XSetting::XftDpi => state.x11_update_dpi(),
                    XSetting::GtkCursorThemeSize => state.x11_update_cursor_theme_size(),

                    xsetting if XSETTINGS_FROM_XFCONF.contains(&xsetting) => {
                        if let Some(xw) = state.core.xwayland.as_mut() {
                            xsetting
                                .to_xwm_value(property_value)
                                .ok_or_else(|| anyhow!("failed to convert xsetting value"))
                                .and_then(|value| xw.update_xsetting(xsetting.name(), value))
                        } else {
                            Ok(())
                        }
                    }

                    _ => Ok(()),
                }) {
                    tracing::warn!("Failed to set xsetting from '{property_name}': {err}");
                }
            })
            .expect("Failed to register xsettings source with main loop");

        Self(channel)
    }

    pub fn xsettings_for_scale(&self, scale: f64, base_dpi: i32) -> Vec<(String, XwmValue)> {
        let int_scale = scale.ceil().max(1.) as i32;
        self.xsettings_for_dpi(scale, base_dpi)
            .into_iter()
            .chain([self.xsetting_for_cursor_theme_size(scale)])
            .chain([(XSetting::GdkWindowScalingFactor.name().to_owned(), int_scale.into())])
            .collect()
    }

    pub fn xsettings_for_dpi(&self, scale: f64, base_dpi: i32) -> Vec<(String, XwmValue)> {
        let int_scale = scale.ceil().max(1.) as i32;
        let base_dpi = base_dpi as f64;
        let dpi = (base_dpi * scale).round() as i32;
        let unscaled_dpi = ((base_dpi * scale) / int_scale as f64).round() as i32;
        [(XSetting::XftDpi, dpi), (XSetting::GdkUnscaledDPI, unscaled_dpi)]
            .into_iter()
            .map(|(name, value)| (name.name().to_owned(), value.into()))
            .collect()
    }

    pub fn xsetting_for_cursor_theme_size(&self, scale: f64) -> (String, XwmValue) {
        let base_cursor_size = self
            .0
            .get_property(XSetting::GtkCursorThemeSize.xfconf_property_name())
            .and_then(glib_value_to_i32)
            .unwrap_or(DEFAULT_CURSOR_THEME_SIZE);
        let cursor_size = (base_cursor_size as f64 * scale).round() as i32;
        (XSetting::GtkCursorThemeSize.name().to_owned(), cursor_size.into())
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub(in crate::core) fn x11_init_xsettings(&mut self) {
        if let Some(xw) = self.core.xwayland.as_mut() {
            let xsettings = XSETTINGS_FROM_XFCONF
                .iter()
                .flat_map(|xsetting| {
                    xw.xsettings_manager()
                        .0
                        .get_property_value(xsetting.xfconf_property_name())
                        .and_then(|value| xsetting.to_xwm_value(value))
                        .map(|value| (xsetting.name().to_owned(), value))
                })
                .collect::<Vec<_>>();
            if let Err(err) = xw.set_xsettings(xsettings.into_iter()) {
                tracing::warn!("Failed to initialize XSETTINGS: {err}");
            }
        }
    }
}

fn glib_value_to_i32(value: glib::Value) -> Option<i32> {
    value
        .get::<i32>()
        .ok()
        .or_else(|| value.get::<u32>().ok().and_then(|v| i32::try_from(v).ok()))
        .or_else(|| value.get::<bool>().ok().map(i32::from))
}
