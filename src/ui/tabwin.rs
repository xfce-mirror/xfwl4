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

use gtk::glib::{self, Object};

const TABWIN_WIDGET_NAME: &str = "xfwm-tabwin";
const TABWIN_DEFAULT_CSS: &[u8] = br#"#xfwm-tabwin {
  padding: 4px;
  border-radius: 10px;
  border: 1px solid @theme_selected_bg_color;
  background-color: @theme_bg_color;
}"#;

const WIN_ICON_SIZE: u32 = 48;
const WIN_PREVIEW_SIZE: u32 = 6 * WIN_ICON_SIZE;
const LISTVIEW_WIN_ICON_SIZE: u32 = WIN_ICON_SIZE / 2;
const WIN_ICON_BORDER: u32 = 5;
const WIN_MAX_RATIO: f64 = 0.8;

#[derive(Debug)]
pub enum TabwinAction {
    Foo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, glib::Enum)]
#[enum_type(name = "TabwinMode")]
pub enum TabwinMode {
    #[default]
    Grid,
    List,
}

#[derive(Debug)]
pub struct TabwinWindow {
    // cairo::Surface and gdk_pixbuf::Pixbuf cannot be made Send, so send the raw bytes
    pub icon_bytes: Vec<u8>,
    pub name: String,
}

glib::wrapper! {
    pub struct Tabwin(ObjectSubclass<imp::Tabwin>)
        @extends gtk::Window, gtk::Container, gtk::Widget;
}

impl Tabwin {
    pub fn new() -> Self {
        Object::builder()
            // Widget
            .property("name", TABWIN_WIDGET_NAME)
            .property("app-paintable", true)
            // Window
            .property("type", gtk::WindowType::Toplevel)
            .property("default-width", 1)
            .property("default-height", 1)
            .property("position", gtk::WindowPosition::None)
            .build()
    }
}

mod imp {
    use std::cell::{Cell, RefCell};

    use gtk::{
        ffi::GTK_STYLE_PROPERTY_BORDER_RADIUS,
        glib::{self, prelude::*},
        subclass::prelude::*,
        traits::{BoxExt, ContainerExt, CssProviderExt, LabelExt, StyleContextExt, WidgetExt},
    };

    use super::TabwinMode;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::Tabwin)]
    pub struct Tabwin {
        #[property(get)]
        mode: Cell<TabwinMode>,

        label: RefCell<gtk::Label>,
        container: RefCell<gtk::Grid>,
        size: RefCell<(u32, u32)>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Tabwin {
        const NAME: &'static str = "Xfwl4Tabwin";
        type Type = super::Tabwin;
        type ParentType = gtk::Window;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Tabwin {
        fn constructed(&self) {
            let instance = self.obj();

            self.apply_default_theme();
            instance.realize();

            let ctx = instance.style_context();
            let border_radius = ctx.property::<i32>(&String::from_utf8(GTK_STYLE_PROPERTY_BORDER_RADIUS.into()).unwrap());
            let border = ctx.border(gtk::StateFlags::NORMAL);
            let padding = ctx.padding(gtk::StateFlags::NORMAL);
            instance.set_border_width(
                (border_radius as u32)
                    + border.left.max(border.right.max(border.top.max(border.bottom))) as u32
                    + padding.left.max(padding.right.max(padding.top.max(padding.bottom))) as u32,
            );

            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 3);
            instance.add(&vbox);

            let css_class = match self.mode.get() {
                TabwinMode::Grid => {
                    let label = gtk::Label::new(Some(""));
                    label.set_use_markup(true);
                    label.set_justify(gtk::Justification::Center);
                    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    vbox.pack_end(&label, true, true, 0);
                    self.label.replace(label);

                    "tabwin-app-grid"
                }
                TabwinMode::List => "tabwin-app-list",
            };
            ctx.add_class(css_class);

            let window_list = self.create_window_list();
            vbox.pack_start(&window_list, true, true, 0);
            self.container.replace(window_list);

            instance.connect_configure_event(|tabwin, event| tabwin.imp().handle_configure(event));
            instance.connect_draw(|tabwin, ctx| tabwin.imp().handle_draw(ctx));

            instance.show_all();
        }
    }

    impl WidgetImpl for Tabwin {}

    impl ContainerImpl for Tabwin {}

    impl BinImpl for Tabwin {}

    impl WindowImpl for Tabwin {}

    impl Tabwin {
        fn apply_default_theme(&self) {
            // XXX: I can't use LazyLock/OnceLock, because they require the type param to implement
            // XXX: Send, which gtk::CssProvider can't, because it contains a raw pointer.
            // TODO: Likely what I'll do is init the provider inside the UI thread once and pass it
            // in when creating the tabwin.
            let settings = gtk::Settings::default().unwrap();
            #[allow(irrefutable_let_patterns)]
            if let Some(theme) = settings
                .has_property("theme", Some(glib::Type::STRING))
                .then(|| settings.property::<String>("theme"))
                && let Some(theme_provider) = gtk::CssProvider::named(&theme, None)
                && let css = theme_provider.to_str()
                && !css.contains(&format!("#{}", super::TABWIN_WIDGET_NAME))
            {
                let provider = gtk::CssProvider::new();
                provider.load_from_data(super::TABWIN_DEFAULT_CSS).unwrap();
                self.obj()
                    .style_context()
                    .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
            }
        }

        fn build_icon_list(&self) -> IconListData {
            todo!()
        }

        fn create_window_list(&self) -> gtk::Grid {
            let grid = gtk::Grid::builder()
                .row_homogeneous(true)
                .row_spacing(4)
                .column_homogeneous(true)
                .column_spacing(4)
                .build();

            grid
        }

        fn handle_configure(&self, event: &gtk::gdk::EventConfigure) -> bool {
            self.size.replace(event.size());
            false
        }

        fn handle_draw(&self, cr: &gtk::cairo::Context) -> glib::Propagation {
            let instance = self.obj();

            let allocation = instance.allocation();
            let ctx = instance.style_context();

            gtk::render_background(&ctx, cr, 0., 0., allocation.width() as f64, allocation.height() as f64);
            gtk::render_frame(&ctx, cr, 0., 0., allocation.width() as f64, allocation.height() as f64);

            glib::Propagation::Stop
        }
    }

    struct IconListData {
        icons: Vec<gtk::gdk_pixbuf::Pixbuf>,
        icon_size: u32,
        grid_rows: u32,
        grid_cols: u32,
    }
}

#[cfg(test)]
mod test {
    use super::{TABWIN_DEFAULT_CSS, TABWIN_WIDGET_NAME};

    #[test]
    pub fn ensure_tabwin_css_name_correct() {
        let s = String::from_utf8(TABWIN_DEFAULT_CSS.into()).unwrap();
        let leading = format!("#{TABWIN_WIDGET_NAME}");
        assert!(s.starts_with(&leading));
    }
}
