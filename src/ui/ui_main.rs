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

use std::{cell::RefCell, rc::Rc};

use gtk::{
    cairo, pango,
    prelude::{GtkSettingsExt, IconThemeExt, WidgetExt},
};

use crate::{
    ui::{
        UiProcessState,
        compositor_ui_protocol::{self},
        gtk_settings,
        gtk_settings_sync::GtkSettingsSync,
        window_menu,
    },
    util::icon::FALLBACK_ICON_NAME,
};

pub fn ui_main() -> anyhow::Result<()> {
    gtk::gdk::set_allowed_backends("wayland");
    gtk::init()?;

    xfconf::init()?;

    let window_menu_anchor = window_menu::create_anchor_window();
    window_menu_anchor.show_all();

    let state = UiProcessState {
        source: None,
        registry: None,
        ui_manager: None,
        tabwin_state: None,
        tabwin_style_provider: None,
        window_menu_anchor,
        window_menu_state: None,
        window_menu: Rc::new(RefCell::new(None)),
        dialog_states: Vec::new(),
    };

    let display_name = gtk::gdk::Display::default().unwrap().name();
    let state = compositor_ui_protocol::connect(&display_name, state)?;

    let _settings_sync = GtkSettingsSync::new();
    let settings_notifiers = gtk_settings::init_notifiers(Rc::clone(&state));

    glib::idle_add_local_once(|| {
        // Force GtkIconTheme and librsvg to do its init stuff so we don't take that hit the first
        // time we are trying to pop up the tabwin.
        let icon_theme = gtk::IconTheme::default().expect("icon theme not present");
        let _ = icon_theme.load_icon(FALLBACK_ICON_NAME, 64, gtk::IconLookupFlags::FORCE_SVG);
        let _ = icon_theme.load_icon(FALLBACK_ICON_NAME, 64, gtk::IconLookupFlags::NO_SVG);

        // And force a first text rendering to do the global fontconfig init and load the default
        // font.
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 150, 40).expect("create small cairo surface");
        let cr = cairo::Context::new(&surface).expect("create cairo context");
        let layout = pangocairo::functions::create_layout(&cr);
        if let Some(font_name) = gtk::Settings::default().and_then(|settings| settings.gtk_font_name()) {
            layout.set_font_description(Some(&pango::FontDescription::from_string(font_name.as_str())));
        }
        layout.set_text("pangocairo warmup");
        pangocairo::functions::show_layout(&cr, &layout);
        surface.flush();
    });

    gtk::main();

    if let Some(source) = state.borrow_mut().source.take() {
        source.destroy();
    }

    let settings = gtk::Settings::default().unwrap();
    for id in settings_notifiers {
        glib::signal_handler_disconnect(&settings, id);
    }

    Ok(())
}
