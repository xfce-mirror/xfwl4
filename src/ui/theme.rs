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

use std::{collections::HashMap, ffi::CStr};

use gtk::{
    gdk,
    traits::{StyleContextExt, WidgetExt},
};

use crate::util::Hlsa;

const LIGHTNESS_MULT: f64 = 1.3;
const DARKNESS_MULT: f64 = 0.7;

#[derive(Debug, Clone, Copy, PartialEq)]
enum StyleName {
    Fg,
    Bg,
    Light,
    Dark,
    Mid,
}

impl StyleName {
    fn property_name(&self) -> String {
        match self {
            Self::Fg => CStr::from_bytes_with_nul(gtk::ffi::GTK_STYLE_PROPERTY_COLOR)
                .expect("strings from gtk should be valid")
                .to_str()
                .unwrap()
                .to_owned(),
            Self::Bg | Self::Light | Self::Dark | Self::Mid => CStr::from_bytes_with_nul(gtk::ffi::GTK_STYLE_PROPERTY_BACKGROUND_COLOR)
                .expect("strings from gtk should be valid")
                .to_str()
                .unwrap()
                .to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum StyleState {
    Normal,
    #[allow(dead_code)]
    Active,
    #[allow(dead_code)]
    Prelight,
    Selected,
    Insensitive,
}

impl StyleState {
    fn flags(&self) -> gtk::StateFlags {
        match self {
            Self::Normal => gtk::StateFlags::NORMAL,
            Self::Active => gtk::StateFlags::ACTIVE,
            Self::Prelight => gtk::StateFlags::PRELIGHT,
            Self::Selected => gtk::StateFlags::SELECTED,
            Self::Insensitive => gtk::StateFlags::INSENSITIVE,
        }
    }
}

pub fn fetch_theme_colors() -> HashMap<&'static str, gdk::RGBA> {
    const COLOR_NAMES: &[(&str, StyleName, StyleState)] = &[
        ("active_text_color", StyleName::Fg, StyleState::Selected),
        ("inactive_text_color", StyleName::Fg, StyleState::Insensitive),
        ("active_text_shadow_color", StyleName::Dark, StyleState::Selected),
        ("inactive_text_shadow_color", StyleName::Dark, StyleState::Insensitive),
        ("active_border_color", StyleName::Fg, StyleState::Normal),
        ("inactive_border_color", StyleName::Fg, StyleState::Normal),
        ("active_color_1", StyleName::Bg, StyleState::Selected),
        ("active_hilight_1", StyleName::Light, StyleState::Selected),
        ("active_shadow_1", StyleName::Dark, StyleState::Selected),
        ("active_mid_1", StyleName::Mid, StyleState::Selected),
        ("active_text_color_2", StyleName::Fg, StyleState::Normal),
        ("active_color_2", StyleName::Bg, StyleState::Normal),
        ("active_hilight_2", StyleName::Light, StyleState::Normal),
        ("active_shadow_2", StyleName::Dark, StyleState::Normal),
        ("active_mid_2", StyleName::Mid, StyleState::Normal),
        ("inactive_color_1", StyleName::Bg, StyleState::Insensitive),
        ("inactive_hilight_1", StyleName::Light, StyleState::Insensitive),
        ("inactive_shadow_1", StyleName::Dark, StyleState::Insensitive),
        ("inactive_mid_1", StyleName::Mid, StyleState::Insensitive),
        ("inactive_text_color_2", StyleName::Fg, StyleState::Insensitive),
        ("inactive_color_2", StyleName::Bg, StyleState::Normal),
        ("inactive_hilight_2", StyleName::Light, StyleState::Normal),
        ("inactive_shadow_2", StyleName::Dark, StyleState::Normal),
        ("inactive_mid_2", StyleName::Mid, StyleState::Normal),
    ];

    let win = gtk::Window::new(gtk::WindowType::Toplevel);
    win.realize();

    let ctx = win.style_context();
    ctx.add_class("gtkstyle-fallback");

    COLOR_NAMES
        .iter()
        .flat_map(|(name_str, name, state)| {
            let value = ctx.style_property_for_state(&name.property_name(), state.flags());
            value
                .get::<gdk::RGBA>()
                .ok()
                .map(|rgba| match name {
                    StyleName::Light => gdk::RGBA::from(Hlsa::from(rgba).shade(LIGHTNESS_MULT)),
                    StyleName::Dark => gdk::RGBA::from(Hlsa::from(rgba).shade(DARKNESS_MULT)),
                    StyleName::Mid => {
                        let light = gdk::RGBA::from(Hlsa::from(rgba).shade(LIGHTNESS_MULT));
                        let dark = gdk::RGBA::from(Hlsa::from(rgba).shade(DARKNESS_MULT));
                        gdk::RGBA::new(
                            (light.red() + dark.red()) / 2.,
                            (light.green() + dark.green()) / 2.,
                            (light.blue() + dark.blue()) / 2.,
                            light.alpha(),
                        )
                    }
                    StyleName::Fg | StyleName::Bg => rgba,
                })
                .map(|rgba| (*name_str, rgba))
        })
        .collect()
}
