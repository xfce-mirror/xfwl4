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
    cell::{Ref, RefCell},
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    rc::Rc,
    str::FromStr,
};

use anyhow::{Context, anyhow};
use gtk::gdk;
use smithay::reexports::calloop::{
    LoopHandle,
    channel::{self, Channel, Sender},
};
use xfconf::ChannelExtManual;

use crate::{
    backend::Backend,
    build_config::{BUILD_PKGDATADIR, BUILD_XFWM4_PKGDATADIR},
    core::{
        config::{
            XFWM4_CHANNEL_NAME,
            xfwl4_config_types::{
                ActivateAction, DoubleClickAction, EasyClickKey, PlacementMode, TitleAlignment, TitleShadow, TitlebarButtonLayout,
            },
        },
        state::Xfwl4State,
        util::{
            CalloopXfconfSource,
            rc::{self, RcSetting, RcValueType},
        },
    },
    ui::tabwin::TabwinMode,
};

#[derive(Debug)]
struct Xfwl4ConfigInner {
    channel: xfconf::Channel,
    ext_notifier: Sender<String>,
    settings: HashMap<String, RcSetting>,
    color_names: HashMap<&'static str, gdk::RGBA>,
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

    pub fn handle_xfconf_property_changed(&mut self, property_name: &str, value: glib::Value) {
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

    fn theme_path_internal(&self, theme_name: &str) -> Option<PathBuf> {
        let theme_dirs = {
            let mut dirs = vec![
                {
                    let mut home_dir = glib::home_dir();
                    home_dir.push(".themes");
                    home_dir
                },
                {
                    let mut dir = glib::user_data_dir();
                    dir.push("themes");
                    dir
                },
            ];
            dirs.extend(glib::system_data_dirs().into_iter().map(|mut dir| {
                dir.push("themes");
                dir
            }));
            dirs
        };

        theme_dirs.into_iter().find_map(|mut dir| {
            dir.push(theme_name);
            dir.push("xfwm4");
            dir.push("themerc");
            dir.is_file().then(|| {
                dir.pop();
                dir
            })
        })
    }
}

#[derive(Debug, Clone)]
pub struct Xfwl4Config {
    inner: Rc<RefCell<Xfwl4ConfigInner>>,
}

impl Xfwl4Config {
    pub fn new<BackendData: Backend + 'static>(
        handle: LoopHandle<'static, Xfwl4State<BackendData>>,
    ) -> anyhow::Result<(Self, Channel<String>)> {
        let (ext_notifier_tx, ext_notifier_rx) = channel::channel();
        let channel = xfconf::Channel::new(XFWM4_CHANNEL_NAME);

        let config = Self {
            inner: Rc::new(RefCell::new(Xfwl4ConfigInner {
                channel: channel.clone(),
                ext_notifier: ext_notifier_tx,
                settings: settings(),
                color_names: HashMap::new(),
                activate_action: Default::default(),
                button_layout: Default::default(),
                cycle_tabwin_mode: Default::default(),
                double_click_action: Default::default(),
                easy_click: Default::default(),
                placement_mode: Default::default(),
                title_alignment: Default::default(),
                title_shadow_active: Default::default(),
                title_shadow_inactive: Default::default(),
            })),
        };

        let source = CalloopXfconfSource::new(channel, []);
        handle
            .insert_source(source, |(property_name, value), _, state| {
                state
                    .core
                    .config
                    .inner
                    .borrow_mut()
                    .handle_xfconf_property_changed(&property_name, value);
            })
            .expect("Failed to register xfconf xfwm4 source with event loop");

        config.inner.borrow_mut().load_all(true)?;

        Ok((config, ext_notifier_rx))
    }

    pub(in crate::core) fn update_color_names(&self, color_names: HashMap<&'static str, gdk::RGBA>) -> bool {
        let mut inner = self.inner.borrow_mut();
        if inner.color_names != color_names {
            inner.color_names = color_names;
            true
        } else {
            false
        }
    }

    pub(in crate::core) fn color_names(&self) -> Ref<'_, HashMap<&str, gdk::RGBA>> {
        Ref::map(self.inner.borrow(), |inner| &inner.color_names)
    }

    pub fn active_border_color(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("active_border_color")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn active_color_1(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("active_color_1")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn active_color_2(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("active_color_2")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn active_hilight_1(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("active_hilight_1")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn active_hilight_2(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("active_hilight_2")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn active_mid_1(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("active_mid_1")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn active_mid_2(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("active_mid_2")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn active_shadow_1(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("active_shadow_1")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn active_shadow_2(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("active_shadow_2")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn active_text_color(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("active_text_color")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn active_text_color_2(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("active_text_color_2")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn active_text_shadow_color(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("active_text_shadow_color")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn activate_action(&self) -> ActivateAction {
        self.inner.borrow().activate_action
    }

    pub fn borderless_maximize(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("borderless_maximize")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn box_move(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("box_move")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn box_resize(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("box_resize")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn button_layout(&self) -> TitlebarButtonLayout {
        self.inner.borrow().button_layout.clone()
    }

    pub fn button_offset(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("button_offset")
            .and_then(|s| s.as_i32())
            .unwrap_or(0)
    }

    pub fn button_spacing(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("button_spacing")
            .and_then(|s| s.as_i32())
            .unwrap_or(0)
    }

    pub fn click_to_focus(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("click_to_focus")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn cycle_apps_only(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("cycle_apps_only")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn cycle_draw_frame(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("cycle_draw_frame")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn cycle_raise(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("cycle_raise")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn cycle_hidden(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("cycle_hidden")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn cycle_minimized(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("cycle_minimized")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn cycle_minimum(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("cycle_minimum")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn cycle_preview(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("cycle_preview")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn cycle_tabwin_mode(&self) -> TabwinMode {
        self.inner.borrow().cycle_tabwin_mode
    }

    pub fn cycle_workspaces(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("cycle_workspaces")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn double_click_action(&self) -> DoubleClickAction {
        self.inner.borrow().double_click_action
    }

    pub fn double_click_distance(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("double_click_distance")
            .and_then(|s| s.as_i32())
            .unwrap_or(5)
    }

    pub fn double_click_time(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("double_click_time")
            .and_then(|s| s.as_i32())
            .unwrap_or(250)
    }

    pub fn easy_click(&self) -> EasyClickKey {
        self.inner.borrow().easy_click
    }

    pub fn focus_delay(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("focus_delay")
            .and_then(|s| s.as_i32())
            .unwrap_or(250)
    }

    pub fn focus_hint(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("focus_hint")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn focus_new(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("focus_new")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn frame_border_top(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("frame_border_top")
            .and_then(|s| s.as_i32())
            .unwrap_or(0)
    }

    pub fn frame_opacity(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("frame_opacity")
            .and_then(|s| s.as_i32())
            .unwrap_or(100)
    }

    pub fn full_width_title(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("full_width_title")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn horiz_scroll_opacity(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("horiz_scroll_opacity")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn inactive_opacity(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("inactive_opacity")
            .and_then(|s| s.as_i32())
            .unwrap_or(100)
    }

    pub fn inactive_border_color(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("inactive_border_color")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn inactive_color_1(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("inactive_color_1")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn inactive_color_2(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("inactive_color_2")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn inactive_hilight_1(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("inactive_hilight_1")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn inactive_hilight_2(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("inactive_hilight_2")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn inactive_mid_1(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("inactive_mid_1")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn inactive_mid_2(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("inactive_mid_2")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn inactive_shadow_1(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("inactive_shadow_1")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn inactive_shadow_2(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("inactive_shadow_2")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn inactive_text_color(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("inactive_text_color")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn inactive_text_color_2(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("inactive_text_color_2")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn inactive_text_shadow_color(&self) -> Option<gdk::RGBA> {
        let inner = self.inner.borrow();
        inner
            .settings
            .get("inactive_text_shadow_color")
            .and_then(|s| s.as_color_resolved(&inner.color_names))
    }

    pub fn maximized_offset(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("maximized_offset")
            .and_then(|s| s.as_i32())
            .unwrap_or(0)
    }

    pub fn margin_bottom(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("margin_bottom")
            .and_then(|s| s.as_i32())
            .unwrap_or(0)
    }

    pub fn margin_left(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("margin_left")
            .and_then(|s| s.as_i32())
            .unwrap_or(0)
    }

    pub fn margin_right(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("margin_right")
            .and_then(|s| s.as_i32())
            .unwrap_or(0)
    }

    pub fn margin_top(&self) -> i32 {
        self.inner.borrow().settings.get("margin_top").and_then(|s| s.as_i32()).unwrap_or(0)
    }

    pub fn mousewheel_rollup(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("mousewheel_rollup")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn move_opacity(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("move_opacity")
            .and_then(|s| s.as_i32())
            .unwrap_or(100)
    }

    pub fn placement_mode(&self) -> PlacementMode {
        self.inner.borrow().placement_mode
    }

    pub fn placement_ratio(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("placement_ratio")
            .and_then(|s| s.as_i32())
            .unwrap_or(20)
    }

    pub fn popup_opacity(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("popup_opacity")
            .and_then(|s| s.as_i32())
            .unwrap_or(100)
    }

    pub fn prevent_focus_stealing(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("prevent_focus_stealing")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn raise_delay(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("raise_delay")
            .and_then(|s| s.as_i32())
            .unwrap_or(250)
    }

    pub fn raise_on_click(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("raise_on_click")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn raise_on_focus(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("raise_on_focus")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn raise_with_any_button(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("raise_with_any_button")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn repeat_urgent_blink(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("repeat_urgent_blink")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn resize_opacity(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("resize_opacity")
            .and_then(|s| s.as_i32())
            .unwrap_or(100)
    }

    pub fn scroll_workspaces(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("scroll_workspaces")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn shadow_delta_height(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("shadow_delta_height")
            .and_then(|s| s.as_i32())
            .unwrap_or(0)
    }

    pub fn shadow_delta_width(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("shadow_delta_width")
            .and_then(|s| s.as_i32())
            .unwrap_or(0)
    }

    pub fn shadow_delta_x(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("shadow_delta_x")
            .and_then(|s| s.as_i32())
            .unwrap_or(0)
    }

    pub fn shadow_delta_y(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("shadow_delta_y")
            .and_then(|s| s.as_i32())
            .unwrap_or(-3)
    }

    pub fn shadow_opacity(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("shadow_opacity")
            .and_then(|s| s.as_i32())
            .unwrap_or(50)
    }

    pub fn show_app_icon(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("show_app_icon")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn show_dock_shadow(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("show_dock_shadow")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn show_frame_shadow(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("show_frame_shadow")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn show_popup_shadow(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("show_popup_shadow")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn snap_resist(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("snap_resist")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn snap_to_border(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("snap_to_border")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn snap_to_windows(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("snap_to_windows")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn snap_width(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("snap_width")
            .and_then(|s| s.as_i32())
            .unwrap_or(10)
    }

    pub fn theme(&self) -> String {
        self.inner.borrow().theme()
    }

    pub fn theme_path(&self) -> Option<PathBuf> {
        let inner = self.inner.borrow();
        let theme_name = inner.theme();
        inner.theme_path_internal(&theme_name).or_else(|| {
            tracing::warn!("Theme {theme_name} does not exist; falling back to 'Default'");
            inner.theme_path_internal("Default")
        })
    }

    pub fn tile_on_move(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("tile_on_move")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn title_alignment(&self) -> TitleAlignment {
        self.inner.borrow().title_alignment
    }

    pub fn title_font(&self) -> String {
        self.inner
            .borrow()
            .settings
            .get("title_font")
            .and_then(|s| s.as_string())
            .unwrap_or_else(|| "Sans Bold 9".to_owned())
    }

    pub fn title_horizontal_offset(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("title_horizontal_offset")
            .and_then(|s| s.as_i32())
            .unwrap_or(0)
    }

    pub fn titleless_maximize(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("titleless_maximize")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn title_shadow_active(&self) -> TitleShadow {
        self.inner.borrow().title_shadow_active
    }

    pub fn title_shadow_inactive(&self) -> TitleShadow {
        self.inner.borrow().title_shadow_inactive
    }

    pub fn title_vertical_offset_active(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("title_vertical_offset_active")
            .and_then(|s| s.as_i32())
            .unwrap_or(0)
    }

    pub fn title_vertical_offset_inactive(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("title_vertical_offset_inactive")
            .and_then(|s| s.as_i32())
            .unwrap_or(0)
    }

    pub fn toggle_workspaces(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("toggle_workspaces")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn unredirect_overlays(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("unredirect_overlays")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn urgent_blink(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("urgent_blink")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn use_compositing(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("use_compositing")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    // XXX: not using this
    pub fn vblank_mode(&self) -> String {
        self.inner
            .borrow()
            .settings
            .get("vblank_mode")
            .and_then(|s| s.as_string())
            .unwrap_or_else(|| "auto".to_owned())
    }

    pub fn workspace_count(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("workspace_count")
            .and_then(|s| s.as_i32())
            .unwrap_or(4)
    }

    pub fn wrap_cycle(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("wrap_cycle")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn wrap_layout(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("wrap_layout")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn wrap_resistance(&self) -> i32 {
        self.inner
            .borrow()
            .settings
            .get("wrap_resistance")
            .and_then(|s| s.as_i32())
            .unwrap_or(10)
    }

    pub fn wrap_windows(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("wrap_windows")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn wrap_workspaces(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("wrap_workspaces")
            .and_then(|s| s.as_bool())
            .unwrap_or(false)
    }

    pub fn zoom_desktop(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("zoom_desktop")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn zoom_pointer(&self) -> bool {
        self.inner
            .borrow()
            .settings
            .get("zoom_pointer")
            .and_then(|s| s.as_bool())
            .unwrap_or(true)
    }

    pub fn is_decoration_setting(&self, property_name: &str) -> bool {
        self.inner
            .borrow()
            .settings
            .get(property_name)
            .map(|s| s.is_decoration_setting())
            .unwrap_or(false)
    }
}

fn settings() -> HashMap<String, RcSetting> {
    [
        RcSetting::new("activate_action", RcValueType::String, true, false, true),
        RcSetting::new("active_border_color", RcValueType::Color, false, true, false),
        RcSetting::new("active_color_1", RcValueType::Color, false, true, false),
        RcSetting::new("active_color_2", RcValueType::Color, false, true, false),
        RcSetting::new("active_hilight_1", RcValueType::Color, false, true, false),
        RcSetting::new("active_hilight_2", RcValueType::Color, false, true, false),
        RcSetting::new("active_mid_1", RcValueType::Color, false, true, false),
        RcSetting::new("active_mid_2", RcValueType::Color, false, true, false),
        RcSetting::new("active_shadow_1", RcValueType::Color, false, true, false),
        RcSetting::new("active_shadow_2", RcValueType::Color, false, true, false),
        RcSetting::new("active_text_color", RcValueType::Color, false, true, false),
        RcSetting::new("active_text_color_2", RcValueType::Color, false, true, false),
        RcSetting::new("active_text_shadow_color", RcValueType::Color, false, true, false),
        RcSetting::new("borderless_maximize", RcValueType::Bool, true, true, true),
        RcSetting::new("box_move", RcValueType::Bool, true, false, true),
        RcSetting::new("box_resize", RcValueType::Bool, true, false, true),
        RcSetting::new("button_layout", RcValueType::String, true, true, true),
        RcSetting::new("button_offset", RcValueType::Int, true, true, true),
        RcSetting::new("button_spacing", RcValueType::Int, true, true, true),
        RcSetting::new("click_to_focus", RcValueType::Bool, true, false, true),
        RcSetting::new("cycle_apps_only", RcValueType::Bool, true, false, true),
        RcSetting::new("cycle_draw_frame", RcValueType::Bool, true, false, true),
        RcSetting::new("cycle_hidden", RcValueType::Bool, true, false, true),
        RcSetting::new("cycle_minimized", RcValueType::Bool, true, false, true),
        RcSetting::new("cycle_minimum", RcValueType::Bool, true, false, true),
        RcSetting::new("cycle_preview", RcValueType::Bool, true, false, true),
        RcSetting::new("cycle_raise", RcValueType::Bool, true, false, true),
        RcSetting::new("cycle_tabwin_mode", RcValueType::Int, true, false, false),
        RcSetting::new("cycle_workspaces", RcValueType::Bool, true, false, true),
        RcSetting::new("double_click_action", RcValueType::String, true, false, true),
        RcSetting::new("double_click_distance", RcValueType::Int, true, false, true),
        RcSetting::new("double_click_time", RcValueType::Int, true, false, true),
        RcSetting::new("easy_click", RcValueType::String, true, false, true),
        RcSetting::new("focus_delay", RcValueType::Int, true, false, true),
        RcSetting::new("focus_hint", RcValueType::Bool, true, false, true),
        RcSetting::new("focus_new", RcValueType::Bool, true, false, true),
        RcSetting::new("frame_border_top", RcValueType::Int, true, true, true),
        RcSetting::new("frame_opacity", RcValueType::Int, true, false, true),
        RcSetting::new("full_width_title", RcValueType::Bool, true, true, true),
        RcSetting::new("horiz_scroll_opacity", RcValueType::Bool, true, false, false),
        RcSetting::new("inactive_border_color", RcValueType::Color, false, true, false),
        RcSetting::new("inactive_color_1", RcValueType::Color, false, true, false),
        RcSetting::new("inactive_color_2", RcValueType::Color, false, true, false),
        RcSetting::new("inactive_hilight_1", RcValueType::Color, false, true, false),
        RcSetting::new("inactive_hilight_2", RcValueType::Color, false, true, false),
        RcSetting::new("inactive_mid_1", RcValueType::Color, false, true, false),
        RcSetting::new("inactive_mid_2", RcValueType::Color, false, true, false),
        RcSetting::new("inactive_opacity", RcValueType::Int, true, false, true),
        RcSetting::new("inactive_shadow_1", RcValueType::Color, false, true, false),
        RcSetting::new("inactive_shadow_2", RcValueType::Color, false, true, false),
        RcSetting::new("inactive_text_color", RcValueType::Color, false, true, false),
        RcSetting::new("inactive_text_color_2", RcValueType::Color, false, true, false),
        RcSetting::new("inactive_text_shadow_color", RcValueType::Color, false, true, false),
        RcSetting::new("margin_bottom", RcValueType::Int, true, false, false),
        RcSetting::new("margin_left", RcValueType::Int, true, false, false),
        RcSetting::new("margin_right", RcValueType::Int, true, false, false),
        RcSetting::new("margin_top", RcValueType::Int, true, false, false),
        RcSetting::new("maximized_offset", RcValueType::Int, true, true, true),
        RcSetting::new("mousewheel_rollup", RcValueType::Bool, true, false, false),
        RcSetting::new("move_opacity", RcValueType::Int, true, false, true),
        RcSetting::new("placement_mode", RcValueType::String, true, false, true),
        RcSetting::new("placement_ratio", RcValueType::Int, true, false, true),
        RcSetting::new("popup_opacity", RcValueType::Int, true, false, true),
        RcSetting::new("prevent_focus_stealing", RcValueType::Bool, true, false, true),
        RcSetting::new("raise_delay", RcValueType::Int, true, false, true),
        RcSetting::new("raise_on_click", RcValueType::Bool, true, false, true),
        RcSetting::new("raise_on_focus", RcValueType::Bool, true, false, true),
        RcSetting::new("raise_with_any_button", RcValueType::Bool, true, false, true),
        RcSetting::new("repeat_urgent_blink", RcValueType::Bool, true, false, true),
        RcSetting::new("resize_opacity", RcValueType::Int, true, false, true),
        RcSetting::new("scroll_workspaces", RcValueType::Bool, true, false, true),
        RcSetting::new("shadow_delta_height", RcValueType::Int, true, false, true),
        RcSetting::new("shadow_delta_width", RcValueType::Int, true, false, true),
        RcSetting::new("shadow_delta_x", RcValueType::Int, true, false, true),
        RcSetting::new("shadow_delta_y", RcValueType::Int, true, false, true),
        RcSetting::new("shadow_opacity", RcValueType::Int, true, false, true),
        RcSetting::new("show_app_icon", RcValueType::Bool, true, false, true),
        RcSetting::new("show_dock_shadow", RcValueType::Bool, true, false, true),
        RcSetting::new("show_frame_shadow", RcValueType::Bool, true, false, true),
        RcSetting::new("show_popup_shadow", RcValueType::Bool, true, false, true),
        RcSetting::new("snap_resist", RcValueType::Bool, true, false, true),
        RcSetting::new("snap_to_border", RcValueType::Bool, true, false, true),
        RcSetting::new("snap_to_windows", RcValueType::Bool, true, false, true),
        RcSetting::new("snap_width", RcValueType::Int, true, false, true),
        RcSetting::new("theme", RcValueType::String, true, false, true),
        RcSetting::new("tile_on_move", RcValueType::Bool, true, false, true),
        RcSetting::new("title_alignment", RcValueType::String, true, true, true),
        RcSetting::new("title_font", RcValueType::String, true, true, false),
        RcSetting::new("title_horizontal_offset", RcValueType::Int, true, true, true),
        RcSetting::new("title_shadow_active", RcValueType::String, true, true, true),
        RcSetting::new("title_shadow_inactive", RcValueType::String, true, true, true),
        RcSetting::new("title_vertical_offset_active", RcValueType::Int, true, true, true),
        RcSetting::new("title_vertical_offset_inactive", RcValueType::Int, true, true, true),
        RcSetting::new("titleless_maximize", RcValueType::Bool, true, false, true),
        RcSetting::new("toggle_workspaces", RcValueType::Bool, true, false, true),
        RcSetting::new("unredirect_overlays", RcValueType::Bool, true, false, true),
        RcSetting::new("urgent_blink", RcValueType::Bool, true, false, true),
        RcSetting::new("use_compositing", RcValueType::Bool, true, false, true),
        RcSetting::new("vblank_mode", RcValueType::String, true, false, false),
        RcSetting::new("workspace_count", RcValueType::Int, true, false, true),
        RcSetting::new("wrap_cycle", RcValueType::Bool, true, false, true),
        RcSetting::new("wrap_layout", RcValueType::Bool, true, false, true),
        RcSetting::new("wrap_resistance", RcValueType::Int, true, false, true),
        RcSetting::new("wrap_windows", RcValueType::Bool, true, false, true),
        RcSetting::new("wrap_workspaces", RcValueType::Bool, true, false, true),
        RcSetting::new("zoom_desktop", RcValueType::Bool, true, false, true),
        RcSetting::new("zoom_pointer", RcValueType::Bool, true, false, true),
    ]
    .into_iter()
    .map(|setting| (setting.name.to_owned(), setting))
    .collect()
}
