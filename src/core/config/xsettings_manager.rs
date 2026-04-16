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
use smithay::{
    reexports::calloop::LoopHandle,
    xwayland::{X11Wm, xwm::settings::Value as XwmValue},
};
use xfconf::ChannelExtManual;

use crate::{
    backend::Backend,
    core::{state::Xfwl4State, util::CalloopXfconfSource},
};

const XSETTINGS_CHANNEL_NAME: &str = "xsettings";
const XSETTINGS: &[XSetting] = &[
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
    XSetting::GtkCursorThemeSize,
    XSetting::GtkDecorationLayout,
    XSetting::GtkDialogsUseHeader,
    XSetting::GtkModules,
    XSetting::GtkTitlebarMiddleClick,
    XSetting::XftAntialias,
    XSetting::XftHinting,
    XSetting::XftHintStyle,
    XSetting::XftRgba,
    XSetting::XftDpi,
    XSetting::GdkWindowScalingFactor,
];

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
            | Self::GtkCursorThemeSize
            | Self::GtkDialogsUseHeader
            | Self::XftAntialias
            | Self::XftHinting
            | Self::GdkWindowScalingFactor => glib_value_to_i32(value).map(XwmValue::Integer),

            Self::XftDpi => glib_value_to_i32(value).map(|dpi| XwmValue::Integer(dpi * 1024)),

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
        }
    }
}

impl FromStr for XSetting {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        XSETTINGS
            .iter()
            .copied()
            .find(|xsetting| xsetting.xfconf_property_name() == s)
            .ok_or_else(|| anyhow!("Unknown xfconf property '{s}' for XSetting"))
    }
}

impl XSettingsManager {
    pub fn new<BackendData: Backend + 'static>(handle: LoopHandle<'_, Xfwl4State<BackendData>>) -> Self {
        let channel = xfconf::Channel::new(XSETTINGS_CHANNEL_NAME);
        let property_names = XSETTINGS.iter().map(|xsetting| xsetting.xfconf_property_name());
        let source = CalloopXfconfSource::new(channel.clone(), property_names);

        handle
            .insert_source(source, |(property_name, property_value), _, state| {
                if let Some(xw) = state.core.xwayland.as_mut()
                    && let Err(err) = property_name.parse::<XSetting>().and_then(|xsetting| {
                        xsetting
                            .to_xwm_value(property_value)
                            .ok_or_else(|| anyhow!("failed to convert xsetting value"))
                            .and_then(|value| {
                                xw.xwm
                                    .set_xsettings([(xsetting.name().to_owned(), value)].into_iter())
                                    .map_err(anyhow::Error::from)
                            })
                    })
                {
                    tracing::warn!("Failed to set xsetting from '{property_name}': {err}");
                }
            })
            .expect("Failed to register xsettings source with main loop");

        Self(channel)
    }

    pub fn init_xsettings(&self, xwm: &mut X11Wm) {
        let xsettings = XSETTINGS.iter().flat_map(|xsetting| {
            self.0
                .get_property_value(xsetting.xfconf_property_name())
                .and_then(|value| xsetting.to_xwm_value(value))
                .map(|value| (xsetting.name().to_owned(), value))
        });

        if let Err(err) = xwm.set_xsettings(xsettings) {
            tracing::warn!("Failed to set initial xsettings: {err}");
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
