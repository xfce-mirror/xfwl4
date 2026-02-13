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

use std::{
    cell::RefCell,
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    rc::Rc,
    str::FromStr,
};

use anyhow::{Context, anyhow};
use smithay::reexports::calloop::{
    LoopHandle,
    channel::{self, Channel, Sender},
};
use xfconf::ChannelExtManual;

use crate::{
    Xfwl4State,
    backend::Backend,
    build_config::{BUILD_PKGDATADIR, BUILD_XFWM4_PKGDATADIR},
    config::{
        XFWM4_CHANNEL_NAME,
        xfwl4_config_types::{
            ActivateAction, DoubleClickAction, EasyClickKey, PlacementMode, TitleAlignment, TitleShadow, TitlebarButtonLayout,
        },
    },
    ui::tabwin::TabwinMode,
    util::{
        CalloopXfconfSource,
        rc::{self, RcColor, RcSetting, RcValueType},
    },
};

#[derive(Debug)]
struct Xfwl4ConfigInner {
    channel: xfconf::Channel,
    ext_notifier: Sender<String>,
    settings: HashMap<String, RcSetting>,
    activate_action: ActivateAction,
    button_layout: TitlebarButtonLayout,
    cycle_tabwin_mode: TabwinMode,
    double_click_action: DoubleClickAction,
    easy_click: EasyClickKey,
    placement_mode: PlacementMode,
    title_alignment: TitleAlignment,
    title_shadow_active: TitleShadow,
    title_shadow_inactive: TitleShadow,
}

impl Xfwl4ConfigInner {
    fn theme(&self) -> String {
        self.settings
            .get("theme")
            .and_then(|s| s.as_string())
            .unwrap_or_else(|| "Default".to_owned())
    }

    fn update_cached_value(&mut self, name: &str) {
        fn fetch_str_value<T>(inner: &mut Xfwl4ConfigInner, name: &str) -> T
        where
            T: FromStr + Default,
            T::Err: fmt::Display,
        {
            inner
                .settings
                .get(name)
                .and_then(|s| s.as_string())
                .and_then(|s| {
                    s.parse::<T>()
                        .inspect_err(|err| tracing::warn!("Failed to parse {name}: {err}"))
                        .ok()
                })
                .unwrap_or_default()
        }

        match name {
            "activate_action" => {
                self.activate_action = fetch_str_value(self, "activate_action");
            }
            "button_layout" => {
                self.button_layout = fetch_str_value(self, "button_layout");
            }
            "cycle_tabwin_mode" => {
                self.cycle_tabwin_mode = self
                    .settings
                    .get("cycle_tabwin_mode")
                    .and_then(|s| s.as_i32())
                    .and_then(|i| {
                        TabwinMode::try_from(i)
                            .inspect_err(|err| tracing::warn!("Failed to parse cycle_tabwin_mode: {err}"))
                            .ok()
                    })
                    .unwrap_or_default();
            }
            "double_click_action" => {
                self.double_click_action = fetch_str_value(self, "double_click_action");
            }
            "easy_click" => {
                self.easy_click = fetch_str_value(self, "easy_click");
            }
            "placement_mode" => {
                self.placement_mode = fetch_str_value(self, "placement_mode");
            }
            "title_alignment" => {
                self.title_alignment = fetch_str_value(self, "title_alignment");
            }
            "title_shadow_active" => {
                self.title_shadow_active = fetch_str_value(self, "title_shadow_active");
            }
            "title_shadow_inactive" => {
                self.title_shadow_inactive = fetch_str_value(self, "title_shadow_inactive");
            }
            _ => {}
        }
    }

    fn update_cached_values(&mut self) {
        self.update_cached_value("activate_action");
        self.update_cached_value("button_layout");
        self.update_cached_value("cycle_tabwin_mode");
        self.update_cached_value("double_click_action");
        self.update_cached_value("easy_click");
        self.update_cached_value("placement_mode");
        self.update_cached_value("title_alignment");
        self.update_cached_value("title_shadow_active");
        self.update_cached_value("title_shadow_inactive");
    }

    fn load_all(&mut self, fail_on_defaults_error: bool) -> anyhow::Result<()> {
        if let Err(err) = self.load_defaults() {
            if fail_on_defaults_error {
                Err(err)
            } else {
                tracing::warn!("Failed to reload defaults: {err}");
                Ok(())
            }?
        }

        self.load_from_xfconf();

        let theme_name = self.theme();
        if let Err(err) = self.load_from_theme(&theme_name) {
            if theme_name != "Default" {
                tracing::warn!("Failed to load theme {theme_name}; falling back to Default: {err}");
                self.load_from_theme("Default").context("Failed to load Default theme")?;
                self.settings.get_mut("theme").iter_mut().for_each(|setting| {
                    let _ = setting.set_from_str("Default");
                });
                Ok(())
            } else {
                Err(err)
            }?
        }

        self.update_cached_values();

        Ok(())
    }

    fn load_from_rcfile<P: AsRef<Path>>(&mut self, path: P, allow_value_errors: bool) -> anyhow::Result<()> {
        rc::parse(path, &mut self.settings, allow_value_errors)
    }

    fn load_defaults(&mut self) -> anyhow::Result<()> {
        let xfwl4_path = PathBuf::from(format!("{BUILD_PKGDATADIR}/defaults"));
        let xfwm4_path = PathBuf::from(format!("{BUILD_XFWM4_PKGDATADIR}/defaults"));
        let path = if xfwl4_path.exists() {
            Ok(xfwl4_path)
        } else if xfwm4_path.exists() {
            Ok(xfwm4_path)
        } else {
            Err(anyhow!("No default settings file found"))
        }?;

        self.load_from_rcfile(path, false)
    }

    fn load_from_xfconf(&mut self) {
        for (name, setting) in self.settings.iter_mut().filter(|(_, setting)| setting.in_xfconf()) {
            let name = format!("/general/{name}");
            if let Some(value) = self.channel.get_property_value(&name)
                && let Err(err) = setting.set_from_xfconf(value)
            {
                tracing::warn!("{err}");
            }
        }
    }

    fn load_from_theme(&mut self, theme_name: &str) -> anyhow::Result<()> {
        if let Some(themerc) = self.theme_path_internal(theme_name).map(|mut dir| {
            dir.push("themerc");
            dir
        }) {
            self.load_from_rcfile(themerc, true)
        } else {
            Err(anyhow!("Failed to find theme named {theme_name}"))
        }
    }

    fn handle_xfconf_property_changed(&mut self, property_name: &str, value: glib::Value) {
        let name_short = property_name.chars().skip("/general/".len()).collect::<String>();
        if let Some(setting) = self.settings.get_mut(&name_short) {
            if setting.in_xfconf() {
                if let Err(err) = setting.set_from_xfconf(value) {
                    tracing::warn!("Got property '{property_name}' from xfconf but the type was incorrect: {err}");
                } else if name_short == "theme" {
                    if let Err(err) = self.load_all(false) {
                        tracing::error!("Failed to reload config after theme change: {err}");
                    }
                } else {
                    self.update_cached_value(&name_short);
                }

                let _ = self.ext_notifier.send(property_name.to_owned());
            } else {
                tracing::info!(
                    "Got a property-change notification for '{property_name}', but that setting is not supposed to be in xfconf"
                );
            }
        }
    }
}

fn settings() -> HashMap<String, RcSetting> {
    [
        RcSetting::new("activate_action", RcValueType::String, true, true),
        RcSetting::new("active_border_color", RcValueType::Color, false, false),
        RcSetting::new("active_color_1", RcValueType::Color, false, false),
        RcSetting::new("active_color_2", RcValueType::Color, false, false),
        RcSetting::new("active_hilight_1", RcValueType::Color, false, false),
        RcSetting::new("active_hilight_2", RcValueType::Color, false, false),
        RcSetting::new("active_mid_1", RcValueType::Color, false, false),
        RcSetting::new("active_mid_2", RcValueType::Color, false, false),
        RcSetting::new("active_shadow_1", RcValueType::Color, false, false),
        RcSetting::new("active_shadow_2", RcValueType::Color, false, false),
        RcSetting::new("active_text_color", RcValueType::Color, false, false),
        RcSetting::new("active_text_color_2", RcValueType::Color, false, false),
        RcSetting::new("active_text_shadow_color", RcValueType::Color, false, false),
        RcSetting::new("borderless_maximize", RcValueType::Bool, true, true),
        RcSetting::new("box_move", RcValueType::Bool, true, true),
        RcSetting::new("box_resize", RcValueType::Bool, true, true),
        RcSetting::new("button_layout", RcValueType::String, true, true),
        RcSetting::new("button_offset", RcValueType::Int, true, true),
        RcSetting::new("button_spacing", RcValueType::Int, true, true),
        RcSetting::new("click_to_focus", RcValueType::Bool, true, true),
        RcSetting::new("cycle_apps_only", RcValueType::Bool, true, true),
        RcSetting::new("cycle_draw_frame", RcValueType::Bool, true, true),
        RcSetting::new("cycle_hidden", RcValueType::Bool, true, true),
        RcSetting::new("cycle_minimized", RcValueType::Bool, true, true),
        RcSetting::new("cycle_minimum", RcValueType::Bool, true, true),
        RcSetting::new("cycle_preview", RcValueType::Bool, true, true),
        RcSetting::new("cycle_raise", RcValueType::Bool, true, true),
        RcSetting::new("cycle_tabwin_mode", RcValueType::Int, true, false),
        RcSetting::new("cycle_workspaces", RcValueType::Bool, true, true),
        RcSetting::new("double_click_action", RcValueType::String, true, true),
        RcSetting::new("double_click_distance", RcValueType::Int, true, true),
        RcSetting::new("double_click_time", RcValueType::Int, true, true),
        RcSetting::new("easy_click", RcValueType::String, true, true),
        RcSetting::new("focus_delay", RcValueType::Int, true, true),
        RcSetting::new("focus_hint", RcValueType::Bool, true, true),
        RcSetting::new("focus_new", RcValueType::Bool, true, true),
        RcSetting::new("frame_border_top", RcValueType::Int, true, true),
        RcSetting::new("frame_opacity", RcValueType::Int, true, true),
        RcSetting::new("full_width_title", RcValueType::Bool, true, true),
        RcSetting::new("horiz_scroll_opacity", RcValueType::Bool, true, false),
        RcSetting::new("inactive_border_color", RcValueType::Color, false, false),
        RcSetting::new("inactive_color_1", RcValueType::Color, false, false),
        RcSetting::new("inactive_color_2", RcValueType::Color, false, false),
        RcSetting::new("inactive_hilight_1", RcValueType::Color, false, false),
        RcSetting::new("inactive_hilight_2", RcValueType::Color, false, false),
        RcSetting::new("inactive_mid_1", RcValueType::Color, false, false),
        RcSetting::new("inactive_mid_2", RcValueType::Color, false, false),
        RcSetting::new("inactive_opacity", RcValueType::Int, true, true),
        RcSetting::new("inactive_shadow_1", RcValueType::Color, false, false),
        RcSetting::new("inactive_shadow_2", RcValueType::Color, false, false),
        RcSetting::new("inactive_text_color", RcValueType::Color, false, false),
        RcSetting::new("inactive_text_color_2", RcValueType::Color, false, false),
        RcSetting::new("inactive_text_shadow_color", RcValueType::Color, false, false),
        RcSetting::new("margin_bottom", RcValueType::Int, true, false),
        RcSetting::new("margin_left", RcValueType::Int, true, false),
        RcSetting::new("margin_right", RcValueType::Int, true, false),
        RcSetting::new("margin_top", RcValueType::Int, true, false),
        RcSetting::new("maximized_offset", RcValueType::Int, true, true),
        RcSetting::new("mousewheel_rollup", RcValueType::Bool, true, false),
        RcSetting::new("move_opacity", RcValueType::Int, true, true),
        RcSetting::new("placement_mode", RcValueType::String, true, true),
        RcSetting::new("placement_ratio", RcValueType::Int, true, true),
        RcSetting::new("popup_opacity", RcValueType::Int, true, true),
        RcSetting::new("prevent_focus_stealing", RcValueType::Bool, true, true),
        RcSetting::new("raise_delay", RcValueType::Int, true, true),
        RcSetting::new("raise_on_click", RcValueType::Bool, true, true),
        RcSetting::new("raise_on_focus", RcValueType::Bool, true, true),
        RcSetting::new("raise_with_any_button", RcValueType::Bool, true, true),
        RcSetting::new("repeat_urgent_blink", RcValueType::Bool, true, true),
        RcSetting::new("resize_opacity", RcValueType::Int, true, true),
        RcSetting::new("scroll_workspaces", RcValueType::Bool, true, true),
        RcSetting::new("shadow_delta_height", RcValueType::Int, true, true),
        RcSetting::new("shadow_delta_width", RcValueType::Int, true, true),
        RcSetting::new("shadow_delta_x", RcValueType::Int, true, true),
        RcSetting::new("shadow_delta_y", RcValueType::Int, true, true),
        RcSetting::new("shadow_opacity", RcValueType::Int, true, true),
        RcSetting::new("show_app_icon", RcValueType::Bool, true, true),
        RcSetting::new("show_dock_shadow", RcValueType::Bool, true, true),
        RcSetting::new("show_frame_shadow", RcValueType::Bool, true, true),
        RcSetting::new("show_popup_shadow", RcValueType::Bool, true, true),
        RcSetting::new("snap_resist", RcValueType::Bool, true, true),
        RcSetting::new("snap_to_border", RcValueType::Bool, true, true),
        RcSetting::new("snap_to_windows", RcValueType::Bool, true, true),
        RcSetting::new("snap_width", RcValueType::Int, true, true),
        RcSetting::new("theme", RcValueType::String, true, true),
        RcSetting::new("tile_on_move", RcValueType::Bool, true, true),
        RcSetting::new("title_alignment", RcValueType::String, true, true),
        RcSetting::new("title_font", RcValueType::String, true, false),
        RcSetting::new("title_horizontal_offset", RcValueType::Int, true, true),
        RcSetting::new("title_shadow_active", RcValueType::String, true, true),
        RcSetting::new("title_shadow_inactive", RcValueType::String, true, true),
        RcSetting::new("title_vertical_offset_active", RcValueType::Int, true, true),
        RcSetting::new("title_vertical_offset_inactive", RcValueType::Int, true, true),
        RcSetting::new("titleless_maximize", RcValueType::Bool, true, true),
        RcSetting::new("toggle_workspaces", RcValueType::Bool, true, true),
        RcSetting::new("unredirect_overlays", RcValueType::Bool, true, true),
        RcSetting::new("urgent_blink", RcValueType::Bool, true, true),
        RcSetting::new("use_compositing", RcValueType::Bool, true, true),
        RcSetting::new("vblank_mode", RcValueType::String, true, false),
        RcSetting::new("workspace_count", RcValueType::Int, true, true),
        RcSetting::new("wrap_cycle", RcValueType::Bool, true, true),
        RcSetting::new("wrap_layout", RcValueType::Bool, true, true),
        RcSetting::new("wrap_resistance", RcValueType::Int, true, true),
        RcSetting::new("wrap_windows", RcValueType::Bool, true, true),
        RcSetting::new("wrap_workspaces", RcValueType::Bool, true, true),
        RcSetting::new("zoom_desktop", RcValueType::Bool, true, true),
        RcSetting::new("zoom_pointer", RcValueType::Bool, true, true),
    ]
    .into_iter()
    .map(|setting| (setting.name.to_owned(), setting))
    .collect()
}
