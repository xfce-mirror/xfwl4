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
//
// Based on xfwm4/src/tabwin.c, which is:
//
// Copyright (C) 2002-2015 Olivier Fourdan

use anyhow::anyhow;
use glib::subclass::types::ObjectSubclassIsExt;
use gtk::{
    IconLookupFlags, gdk_pixbuf,
    glib::{self, Object},
    traits::IconThemeExt,
};
use smithay::reexports::{calloop::channel, wayland_server::backend::ObjectId};

use crate::{ui::FromUiMessage, util::ImageData};

pub(super) const TABWIN_WIDGET_NAME: &str = "xfwm-tabwin";
pub(super) const TABWIN_DEFAULT_CSS: &str = r#"#xfwm-tabwin {
  padding: 4px;
  border-radius: 10px;
  border: 1px solid @theme_selected_bg_color;
  background-color: @theme_bg_color;
}"#;
pub const TABWIN_WINDOW_TITLE: &str = "Tabwin";

const WIN_ICON_SIZE: u32 = 48;
pub const WIN_PREVIEW_SIZE: u32 = 6 * WIN_ICON_SIZE;
const LISTVIEW_WIN_ICON_SIZE: u32 = WIN_ICON_SIZE / 2;
const WIN_ICON_BORDER: u32 = 5;
const WIN_MAX_RATIO: f64 = 0.8;
const LABEL_HEIGHT: u32 = 30;

#[derive(Debug)]
pub enum TabwinAction {
    HoverWindow(ObjectId),
    WindowSelected(ObjectId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, glib::Enum)]
#[enum_type(name = "TabwinMode")]
pub enum TabwinMode {
    #[default]
    Grid,
    List,
}

impl TryFrom<i32> for TabwinMode {
    type Error = anyhow::Error;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Grid),
            1 => Ok(Self::List),
            invalid => Err(anyhow!("Invalid tabwin mode '{invalid}'")),
        }
    }
}

#[derive(Debug)]
pub struct TabwinClient {
    pub id: ObjectId,
    pub app_name: Option<String>,
    pub title: String,
    pub preview_icon: Option<ImageData>,
    pub app_icon: Option<ImageData>,
    pub is_minimized: bool,
}

glib::wrapper! {
    pub struct Tabwin(ObjectSubclass<imp::Tabwin>)
        @extends gtk::Window, gtk::Container, gtk::Widget;
}

impl Tabwin {
    pub fn new(
        mode: TabwinMode,
        clients: Vec<TabwinClient>,
        initial_selection: ObjectId,
        from_ui_tx: channel::Sender<FromUiMessage>,
        style_provider: Option<&gtk::CssProvider>,
    ) -> Self {
        let tabwin: Self = Object::builder()
            // Widget
            .property("app-paintable", true)
            // Window
            .property("type", gtk::WindowType::Popup)
            .property("title", TABWIN_WINDOW_TITLE)
            .property("decorated", false)
            .property("default-width", 0)
            .property("default-height", 0)
            // Tabwin
            .property("mode", mode)
            .property("fallback-style-provider", style_provider.cloned())
            .build();

        tabwin.imp().init_clients(clients, initial_selection);
        tabwin.imp().from_ui_tx.replace(Some(from_ui_tx));
        tabwin
    }

    pub fn selected(&self) -> Option<ObjectId> {
        self.imp().selected()
    }

    pub fn select_next(&self) -> Option<ObjectId> {
        self.imp().select_next()
    }

    pub fn select_previous(&self) -> Option<ObjectId> {
        self.imp().select_previous()
    }

    pub fn append_client(&self, _client: TabwinClient) {}

    pub fn remove_client(&self, _object_id: ObjectId) {}
}

mod imp {
    use std::{
        cell::{Cell, RefCell},
        ffi::CStr,
    };

    use gtk::{
        ffi::GTK_STYLE_PROPERTY_BORDER_RADIUS,
        gdk::{self, prelude::GdkPixbufExt, traits::MonitorExt},
        gdk_pixbuf,
        glib::{self, prelude::*},
        subclass::prelude::*,
        traits::{BoxExt, ContainerExt, GridExt, GtkWindowExt, LabelExt, StyleContextExt, WidgetExt},
    };
    use indexmap::IndexMap;
    use smithay::reexports::{calloop::channel, wayland_server::backend::ObjectId};

    use crate::ui::{
        FromUiMessage,
        util::{WidgetClassSubclassExtExt, WidgetExtExt},
    };

    use super::{
        LABEL_HEIGHT, LISTVIEW_WIN_ICON_SIZE, TABWIN_WIDGET_NAME, TabwinClient, TabwinMode, WIN_ICON_BORDER, WIN_ICON_SIZE, WIN_MAX_RATIO,
        WIN_PREVIEW_SIZE, build_app_icon, build_client_icon,
    };

    struct IconListClient {
        app_name: Option<String>,
        title: String,
        icon: gdk_pixbuf::Pixbuf,
    }

    struct IconListWidget {
        app_name: Option<String>,
        title: String,
        window_button: gtk::Button,
        button_box: gtk::Box,
        label: gtk::Label,
    }

    struct IconListData {
        icons: IndexMap<ObjectId, IconListClient>,
        icon_size: u32,
        label_height: u32,
        grid_cols: u32,
        grid_rows: u32,
    }

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::Tabwin)]
    pub struct Tabwin {
        #[property(name = "fallback-style-provider", construct_only, get)]
        fallback_style_provider: RefCell<Option<gtk::CssProvider>>,
        #[property(name = "monitor", construct_only, get)]
        monitor: RefCell<Option<gdk::Monitor>>,
        #[property(name = "mode", construct_only, get, builder(TabwinMode::default()))]
        mode: Cell<TabwinMode>,
        #[property(name = "cycle-preview", construct_only, get, default = true)]
        cycle_preview: Cell<bool>,

        pub(super) from_ui_tx: RefCell<Option<channel::Sender<FromUiMessage>>>,
        client_widgets: RefCell<IndexMap<ObjectId, IconListWidget>>,
        selected_client: RefCell<Option<ObjectId>>,

        top_vbox: RefCell<gtk::Box>,
        label: RefCell<gtk::Label>,
        container: RefCell<gtk::Grid>,
        size: RefCell<(u32, u32)>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Tabwin {
        const NAME: &'static str = "Xfwl4Tabwin";
        type Type = super::Tabwin;
        type ParentType = gtk::Window;

        fn class_init(klass: &mut Self::Class) {
            klass.install_style_property_from_pspec(
                glib::ParamSpecInt::builder("preview-size")
                    .nick("preview size")
                    .blurb("size of the app preview")
                    .minimum(1)
                    .default_value(WIN_PREVIEW_SIZE as i32)
                    .read_only()
                    .build(),
            );

            klass.install_style_property_from_pspec(
                glib::ParamSpecInt::builder("icon-size")
                    .nick("icon size")
                    .blurb("size of the app icon")
                    .minimum(1)
                    .default_value(WIN_ICON_SIZE as i32)
                    .read_only()
                    .build(),
            );

            klass.install_style_property_from_pspec(
                glib::ParamSpecInt::builder("listview-icon-size")
                    .nick("listview icon size")
                    .blurb("ize of the app icon in listview mode")
                    .minimum(1)
                    .default_value(LISTVIEW_WIN_ICON_SIZE as i32)
                    .read_only()
                    .build(),
            );
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for Tabwin {
        fn constructed(&self) {
            self.parent_constructed();

            let instance = self.obj();
            instance.set_position(gtk::WindowPosition::None);
            instance.set_widget_name(TABWIN_WIDGET_NAME);

            if let Some(fallback_style_provider) = self.fallback_style_provider.borrow().as_ref() {
                instance
                    .style_context()
                    .add_provider(fallback_style_provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
            }

            instance.realize();

            let ctx = instance.style_context();

            let border_radius_prop = CStr::from_bytes_with_nul(GTK_STYLE_PROPERTY_BORDER_RADIUS)
                .expect("strings from gtk should be valid")
                .to_string_lossy();
            let border_radius = ctx
                .style_property_for_state(&border_radius_prop, gtk::StateFlags::NORMAL)
                .get::<i32>()
                .unwrap_or(0);
            let border = ctx.border(gtk::StateFlags::NORMAL);
            let padding = ctx.padding(gtk::StateFlags::NORMAL);
            let border_width = (border_radius as u32)
                + border.left.max(border.right.max(border.top.max(border.bottom))) as u32
                + padding.left.max(padding.right.max(padding.top.max(padding.bottom))) as u32;
            instance.set_border_width(border_width);

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

            self.top_vbox.replace(vbox);

            instance.connect_configure_event(|tabwin, event| tabwin.imp().handle_configure(event));
            instance.connect_draw(|tabwin, ctx| tabwin.imp().handle_draw(ctx));
        }
    }

    impl WidgetImpl for Tabwin {}

    impl ContainerImpl for Tabwin {}

    impl BinImpl for Tabwin {}

    impl WindowImpl for Tabwin {}

    impl Tabwin {
        fn monitor(&self) -> gdk::Monitor {
            self.monitor
                .borrow()
                .as_ref()
                .map(Clone::clone)
                .or_else(|| gdk::Display::default().and_then(|display| display.monitor(0)))
                .expect("there should be at least one monitor")
        }

        pub(super) fn init_clients(&self, clients: Vec<TabwinClient>, initial_selection: ObjectId) {
            let (window_list, button_widgets) = self.create_window_list(clients);
            self.top_vbox.borrow().pack_start(&window_list, true, true, 0);
            self.container.replace(window_list);
            self.client_widgets.replace(button_widgets);

            self.set_selected(initial_selection);
        }

        fn create_window_list(&self, clients: Vec<TabwinClient>) -> (gtk::Grid, IndexMap<ObjectId, IconListWidget>) {
            let instance = self.obj();
            instance.realize();

            let grid = gtk::Grid::builder()
                .row_homogeneous(true)
                .row_spacing(4)
                .column_homogeneous(true)
                .column_spacing(4)
                .build();

            let icon_list_data = self.build_icon_list(clients);
            let monitor = self.monitor();
            let scale = monitor.scale_factor();
            let size_request = icon_list_data.icon_size + icon_list_data.label_height + 2 * WIN_ICON_BORDER;

            let mut pack_pos: u32 = 0;
            #[allow(clippy::mutable_key_type)]
            let widgets = icon_list_data
                .icons
                .into_iter()
                .map(|(id, icon_list_client)| {
                    let IconListClient { app_name, title, icon } = icon_list_client;

                    let window_button = gtk::Button::builder().relief(gtk::ReliefStyle::None).build();

                    if self.mode.get() == TabwinMode::Grid {
                        window_button.connect_enter_notify_event({
                            let myself = instance.to_owned();
                            let id = id.clone();
                            move |_button, _event| {
                                let imp = myself.imp();

                                if let Some(widget_data) = imp.client_widgets.borrow().get(&id) {
                                    widget_data.label.set_label(widget_data.app_name.as_deref().unwrap_or("..."));
                                    myself.imp().label.borrow().set_label(&widget_data.title);
                                }
                                glib::Propagation::Proceed
                            }
                        });

                        window_button.connect_leave_notify_event({
                            let myself = instance.to_owned();
                            let id = id.clone();
                            move |_button, _event| {
                                let imp = myself.imp();

                                if let Some(widget_data) = imp.client_widgets.borrow().get(&id) {
                                    widget_data.label.set_label("");
                                }

                                if let Some(selected) = imp.selected_client.borrow().as_ref()
                                    && let Some(widget_data) = imp.client_widgets.borrow().get(selected)
                                {
                                    myself.imp().label.borrow().set_label(&widget_data.title);
                                }

                                glib::Propagation::Proceed
                            }
                        });
                    }

                    let icon_surface = icon
                        .create_surface(scale, instance.window().as_ref())
                        .expect("failed to create cairo surface from pixbuf");
                    let icon_image = gtk::Image::builder().surface(&icon_surface).build();

                    let (button_box, button_label, left_attach, top_attach) = match self.mode.get() {
                        TabwinMode::Grid => {
                            window_button.set_size_request(size_request as i32, size_request as i32);

                            let button_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(0).build();
                            let button_label = gtk::Label::builder().label("").xalign(0.5).yalign(1.).build();

                            icon_image.set_halign(gtk::Align::Center);
                            icon_image.set_valign(gtk::Align::End);

                            button_box.pack_start(&icon_image, true, true, 0);

                            (
                                button_box,
                                button_label,
                                pack_pos % icon_list_data.grid_cols,
                                pack_pos / icon_list_data.grid_cols,
                            )
                        }

                        TabwinMode::List => {
                            let label_width = monitor.geometry().width() as f64 / (icon_list_data.grid_cols as f64 + 1.);

                            window_button.set_size_request(
                                label_width as i32,
                                if icon_list_data.icon_size < icon_list_data.label_height {
                                    icon_list_data.label_height as i32 + 8
                                } else {
                                    icon_list_data.icon_size as i32 + 8
                                },
                            );

                            let button_box = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(6).build();
                            let button_label = gtk::Label::builder().label(&*title).xalign(0.).yalign(0.5).build();

                            icon_image.set_halign(gtk::Align::Center);
                            icon_image.set_valign(gtk::Align::Center);

                            button_box.pack_start(&icon_image, false, false, 0);

                            (
                                button_box,
                                button_label,
                                pack_pos / icon_list_data.grid_rows,
                                pack_pos % icon_list_data.grid_rows,
                            )
                        }
                    };

                    window_button.add(&button_box);

                    button_label.set_justify(gtk::Justification::Center);
                    button_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    button_box.pack_start(&button_label, true, true, 0);

                    grid.attach(&window_button, left_attach as i32, top_attach as i32, 1, 1);

                    pack_pos += 1;

                    (
                        id,
                        IconListWidget {
                            app_name,
                            title,
                            window_button,
                            button_box,
                            label: button_label,
                        },
                    )
                })
                .collect();

            (grid, widgets)
        }

        fn build_icon_list(&self, clients: Vec<TabwinClient>) -> IconListData {
            let instance = self.obj();

            let monitor = self.monitor();
            let monitor_width = monitor.geometry().width();
            let monitor_height = monitor.geometry().height();

            let layout = instance.create_pango_layout(Some(""));
            let label_height = {
                let height = layout.pixel_size().1;
                if height <= 0 { LABEL_HEIGHT } else { height as u32 }
            };

            let mut cycle_preview = self.cycle_preview.get();

            let n_clients = clients.len();

            let (icon_size, grid_cols, grid_rows) = match self.mode.get() {
                TabwinMode::Grid => {
                    let calc_grid_size = move |size_request: u32| {
                        let grid_cols = (monitor_width as f64 * WIN_MAX_RATIO / size_request as f64).floor() as u32;
                        let grid_rows = (n_clients as f64 / grid_cols as f64).ceil() as u32;
                        (grid_cols, grid_rows)
                    };

                    let standard_icon_size = instance.style_property::<i32>("icon-size") as u32;

                    let mut icon_size = if cycle_preview {
                        instance.style_property::<i32>("preview-size") as u32
                    } else {
                        standard_icon_size
                    };

                    let mut size_request = icon_size + label_height + 2 * WIN_ICON_BORDER;
                    let (mut grid_cols, mut grid_rows) = calc_grid_size(size_request);

                    // Halve the icon size until everything fits
                    while (size_request * grid_rows + label_height) as f64 > monitor_height as f64 * WIN_MAX_RATIO {
                        icon_size /= 2;
                        if cycle_preview && icon_size <= standard_icon_size {
                            cycle_preview = false;
                            icon_size = standard_icon_size;
                        }

                        size_request = icon_size + label_height + 2 * WIN_ICON_BORDER;
                        (grid_cols, grid_rows) = calc_grid_size(size_request);

                        // Don't let the icons get too small
                        if icon_size < 8 {
                            icon_size = 8;
                            break;
                        }
                    }

                    (icon_size, grid_cols, grid_rows)
                }

                TabwinMode::List => {
                    let icon_size = instance.style_property::<i32>("listview-icon-size") as u32;
                    let grid_rows = ((monitor_height as f64 * WIN_MAX_RATIO) / (icon_size + 2 * WIN_ICON_BORDER) as f64).floor() as u32;
                    let grid_cols = (n_clients as f64 / grid_rows as f64).ceil() as u32;

                    (icon_size, grid_cols, grid_rows)
                }
            };

            let scaled_size = icon_size * monitor.scale_factor() as u32;

            let icons = clients
                .into_iter()
                .map(|client| {
                    let icon = if self.mode.get() == TabwinMode::Grid && cycle_preview {
                        build_client_icon(client.preview_icon, client.app_icon, scaled_size, scaled_size, client.is_minimized)
                    } else {
                        build_app_icon(client.app_icon, scaled_size, scaled_size).unwrap_or_else(|| {
                            let blank =
                                gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, scaled_size as i32, scaled_size as i32)
                                    .expect("failed to create empty pixbuf");
                            blank.fill(0x222222ff);
                            blank
                        })
                    };

                    (
                        client.id,
                        IconListClient {
                            app_name: client.app_name,
                            title: client.title,
                            icon,
                        },
                    )
                })
                .collect();

            IconListData {
                icons,
                icon_size,
                label_height,
                grid_cols,
                grid_rows,
            }
        }

        fn set_selected(&self, selected: ObjectId) {
            let update_labels = self.mode.get() == TabwinMode::Grid;

            if let Some(prev_selected) = self.selected_client.borrow_mut().take()
                && let Some(prev_widget_data) = self.client_widgets.borrow().get(&prev_selected)
            {
                if update_labels {
                    prev_widget_data.label.set_label("");
                }
                prev_widget_data.button_box.unset_state_flags(gtk::StateFlags::CHECKED);
            }

            if let Some(widget_data) = self.client_widgets.borrow().get(&selected) {
                widget_data.button_box.set_state_flags(gtk::StateFlags::CHECKED, false);
                if update_labels {
                    widget_data.label.set_label(widget_data.app_name.as_deref().unwrap_or("..."));
                    self.label.borrow().set_label(&widget_data.title);
                }
                widget_data.window_button.grab_focus();
            } else if update_labels {
                self.label.borrow().set_label("");
            }

            self.selected_client.replace(Some(selected));
        }

        pub(super) fn selected(&self) -> Option<ObjectId> {
            self.selected_client.borrow().clone()
        }

        pub(super) fn select_next(&self) -> Option<ObjectId> {
            let cur_selected = self.selected_client.borrow().clone();
            if let Some(cur_selected) = cur_selected {
                let client_widgets = self.client_widgets.borrow();
                if let Some(index) = client_widgets.get_index_of(&cur_selected)
                    && let Some(next) = client_widgets
                        .get_index(index + 1)
                        .or_else(|| client_widgets.first())
                        .map(|(k, _)| k.clone())
                {
                    drop(client_widgets);
                    self.set_selected(next.clone());
                    return Some(next);
                }
            }

            None
        }

        pub(super) fn select_previous(&self) -> Option<ObjectId> {
            let cur_selected = self.selected_client.borrow().clone();
            if let Some(cur_selected) = cur_selected {
                let client_widgets = self.client_widgets.borrow();
                if let Some(index) = client_widgets.get_index_of(&cur_selected) {
                    let previous = if index > 0 {
                        client_widgets.get_index(index - 1)
                    } else {
                        client_widgets.last()
                    }
                    .map(|(k, _)| k.clone());

                    if let Some(previous) = previous {
                        drop(client_widgets);
                        self.set_selected(previous.clone());
                        return Some(previous);
                    }
                }
            }

            None
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

            glib::Propagation::Proceed
        }
    }
}

fn build_client_icon(
    preview_icon: Option<ImageData>,
    app_icon: Option<ImageData>,
    width: u32,
    height: u32,
    is_minimized: bool,
) -> gdk_pixbuf::Pixbuf {
    let icon_pixbuf =
        gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, width as i32, height as i32).expect("failed to create empty pixbuf");
    icon_pixbuf.fill(0);

    if let Some(app_content) = preview_icon
        .and_then(|icon| icon.load(width, height))
        .or_else(|| default_icon_at_size(width, height))
    {
        let aw = app_content.width();
        let ah = app_content.height();
        app_content.copy_area(0, 0, aw, ah, &icon_pixbuf, (width as i32 - aw) / 2, (height as i32 - ah) / 2);
    }

    let small_icon_size = (width / 4).min(height / 4).min(48);
    if let Some(small_icon) = build_app_icon(app_icon, small_icon_size, small_icon_size) {
        small_icon.composite(
            &icon_pixbuf,
            ((width - small_icon_size) / 2) as i32,
            (height - small_icon_size) as i32,
            small_icon_size as i32,
            small_icon_size as i32,
            (width - small_icon_size) as f64 / 2.,
            (height - small_icon_size) as f64,
            1.,
            1.,
            gdk_pixbuf::InterpType::Bilinear,
            0xff,
        );
    }

    if is_minimized {
        let saturated = gdk_pixbuf::Pixbuf::new(
            icon_pixbuf.colorspace(),
            icon_pixbuf.has_alpha(),
            icon_pixbuf.bits_per_sample(),
            icon_pixbuf.width(),
            icon_pixbuf.height(),
        )
        .expect("failed to create empty pixbuf");
        icon_pixbuf.saturate_and_pixelate(&saturated, 0.55, true);
        saturated
    } else {
        icon_pixbuf
    }
}

fn build_app_icon(app_icon: Option<ImageData>, final_width: u32, final_height: u32) -> Option<gdk_pixbuf::Pixbuf> {
    app_icon
        .and_then(|app_icon| app_icon.load(final_width, final_height))
        .or_else(|| default_icon_at_size(final_width, final_height))
}

fn default_icon_at_size(width: u32, height: u32) -> Option<gdk_pixbuf::Pixbuf> {
    let icon_theme = gtk::IconTheme::default().expect("failed to get default icon theme");

    let (width, height) = if width == 0 || height == 0 { (160, 160) } else { (width, height) };

    icon_theme
        .load_icon("xfwm4-default", width.max(height) as i32, IconLookupFlags::FORCE_SIZE)
        .ok()
        .flatten()
}

#[cfg(test)]
mod test {
    use super::{TABWIN_DEFAULT_CSS, TABWIN_WIDGET_NAME};

    #[test]
    pub fn ensure_tabwin_css_name_correct() {
        let leading = format!("#{TABWIN_WIDGET_NAME}");
        assert!(TABWIN_DEFAULT_CSS.starts_with(&leading));
    }
}
