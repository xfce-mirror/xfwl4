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

use std::collections::HashSet;

use anyhow::anyhow;
use glib::{StaticType, subclass::types::ObjectSubclassIsExt};
use gtk::{
    gdk::{self, ModifierType, keys::Key as GdkKey, traits::MonitorExt},
    gdk_pixbuf,
    glib::{self, Object},
    prelude::WidgetExtManual,
    traits::{GtkWindowExt, WidgetExt},
};
use smithay::reexports::{calloop::channel, wayland_server::backend::ObjectId};

use crate::{
    core::util::{ImageData, icon_theme::IconTheme},
    ui::{
        FromUiMessage, ShortcutKey,
        util::{WidgetExtExt, style_property_value_for_type},
    },
};

pub(super) const TABWIN_WIDGET_NAME: &str = "xfwm-tabwin";
pub(super) const TABWIN_DEFAULT_CSS: &str = r#"#xfwm-tabwin {
  padding: 4px;
  border-radius: 10px;
  border: 1px solid @theme_selected_bg_color;
  background-color: @theme_bg_color;
}"#;
pub const TABWIN_WINDOW_TITLE: &str = "Tabwin";

const WIN_ICON_SIZE: i32 = 48;
pub const WIN_PREVIEW_SIZE: i32 = 6 * WIN_ICON_SIZE;
const LISTVIEW_WIN_ICON_SIZE: i32 = WIN_ICON_SIZE / 2;
const WIN_ICON_BORDER: u32 = 5;
const WIN_MAX_RATIO: f64 = 0.8;
const LABEL_HEIGHT: u32 = 30;

#[derive(Debug)]
pub enum TabwinAction {
    HoverWindow(ObjectId),
    Finished(Option<ObjectId>),
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

#[derive(Debug)]
pub struct TabwinConfig {
    pub mode: TabwinMode,
    pub cycle_preview: bool,
    pub window_opacity: f64,
    pub clients: Vec<TabwinClient>,
    pub initial_selection: ObjectId,
    pub next_shortcut: ShortcutKey,
    pub prev_shortcut: ShortcutKey,
    pub up_shortcut: ShortcutKey,
    pub down_shortcut: ShortcutKey,
    pub left_shortcut: ShortcutKey,
    pub right_shortcut: ShortcutKey,
    pub cancel_shortcut: ShortcutKey,
}

struct TabwinMetrics {
    cycle_preview: bool,
    icon_size: u32,
    label_height: u32,
    grid_cols: u32,
    grid_rows: u32,
}

glib::wrapper! {
    pub struct Tabwin(ObjectSubclass<imp::Tabwin>)
        @extends gtk::Window, gtk::Container, gtk::Widget;
}

impl Tabwin {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mode: TabwinMode,
        cycle_preview: bool,
        clients: Vec<TabwinClient>,
        initial_selection: ObjectId,
        from_ui_tx: channel::Sender<FromUiMessage>,
        style_provider: Option<&gtk::CssProvider>,
        next_shortcut: ShortcutKey,
        prev_shortcut: ShortcutKey,
        up_shortcut: ShortcutKey,
        down_shortcut: ShortcutKey,
        left_shortcut: ShortcutKey,
        right_shortcut: ShortcutKey,
        cancel_shortcut: ShortcutKey,
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
            .property("cycle-preview", cycle_preview)
            .property("fallback-style-provider", style_provider.cloned())
            .build();
        tabwin.imp().from_ui_tx.replace(Some(from_ui_tx.clone()));

        let next_prev_modifiers = (next_shortcut.modifiers | prev_shortcut.modifiers) & !cancel_shortcut.modifiers;
        let next_prev_minus_up_modifiers = (next_shortcut.modifiers | prev_shortcut.modifiers) & !up_shortcut.modifiers;
        let next_prev_minus_down_modifiers = (next_shortcut.modifiers | prev_shortcut.modifiers) & !down_shortcut.modifiers;
        let next_prev_minus_left_modifiers = (next_shortcut.modifiers | prev_shortcut.modifiers) & !left_shortcut.modifiers;
        let next_prev_minus_right_modifiers = (next_shortcut.modifiers | prev_shortcut.modifiers) & !right_shortcut.modifiers;
        let next_prev_minus_cancel_modifiers = (next_shortcut.modifiers | prev_shortcut.modifiers) & !cancel_shortcut.modifiers;

        tabwin.add_events(gdk::EventMask::KEY_PRESS_MASK | gdk::EventMask::KEY_RELEASE_MASK);
        tabwin.connect_key_press_event({
            let from_ui_tx = from_ui_tx.clone();
            move |tabwin, event| {
                let key = event.keyval();
                let mask = event.state() & !(ModifierType::LOCK_MASK | ModifierType::MOD4_MASK);

                if key == GdkKey::from(next_shortcut.keysym.raw()) && mask == next_shortcut.modifiers {
                    if let Some(selected) = tabwin.select_next() {
                        let _ = from_ui_tx.send(FromUiMessage::TabwinAction(TabwinAction::HoverWindow(selected)));
                    }
                    glib::Propagation::Stop
                } else if key == GdkKey::from(prev_shortcut.keysym.raw()) && mask == prev_shortcut.modifiers {
                    if let Some(selected) = tabwin.select_previous() {
                        let _ = from_ui_tx.send(FromUiMessage::TabwinAction(TabwinAction::HoverWindow(selected)));
                    }
                    glib::Propagation::Stop
                } else if key == GdkKey::from(up_shortcut.keysym.raw()) && (mask & !next_prev_minus_up_modifiers) == up_shortcut.modifiers {
                    if let Some(selected) = tabwin.select_up() {
                        let _ = from_ui_tx.send(FromUiMessage::TabwinAction(TabwinAction::HoverWindow(selected)));
                    }
                    glib::Propagation::Stop
                } else if key == GdkKey::from(down_shortcut.keysym.raw())
                    && (mask & !next_prev_minus_down_modifiers) == down_shortcut.modifiers
                {
                    if let Some(selected) = tabwin.select_down() {
                        let _ = from_ui_tx.send(FromUiMessage::TabwinAction(TabwinAction::HoverWindow(selected)));
                    }
                    glib::Propagation::Stop
                } else if key == GdkKey::from(left_shortcut.keysym.raw())
                    && (mask & !next_prev_minus_left_modifiers) == left_shortcut.modifiers
                {
                    if let Some(selected) = tabwin.select_left() {
                        let _ = from_ui_tx.send(FromUiMessage::TabwinAction(TabwinAction::HoverWindow(selected)));
                    }
                    glib::Propagation::Stop
                } else if key == GdkKey::from(right_shortcut.keysym.raw())
                    && (mask & !next_prev_minus_right_modifiers) == right_shortcut.modifiers
                {
                    if let Some(selected) = tabwin.select_right() {
                        let _ = from_ui_tx.send(FromUiMessage::TabwinAction(TabwinAction::HoverWindow(selected)));
                    }
                    glib::Propagation::Stop
                } else if key == GdkKey::from(cancel_shortcut.keysym.raw())
                    && (mask & !next_prev_minus_cancel_modifiers) == cancel_shortcut.modifiers
                {
                    let _ = from_ui_tx.send(FromUiMessage::TabwinAction(TabwinAction::Finished(None)));
                    tabwin.close();
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        });

        fn modifier_mask_for_keyval(keyval: GdkKey) -> gdk::ModifierType {
            use gdk::keys::constants::*;
            match keyval {
                val if val == Shift_L || val == Shift_R => gdk::ModifierType::SHIFT_MASK,
                val if val == Control_L || val == Control_R => gdk::ModifierType::CONTROL_MASK,
                val if val == Alt_L || val == Alt_R => gdk::ModifierType::MOD1_MASK,
                val if val == Super_L || val == Super_R => gdk::ModifierType::SUPER_MASK | gdk::ModifierType::MOD4_MASK,
                val if val == Hyper_L || val == Hyper_R => gdk::ModifierType::HYPER_MASK,
                val if val == Meta_L || val == Meta_R => gdk::ModifierType::META_MASK,
                val if val == Caps_Lock => gdk::ModifierType::LOCK_MASK,
                val if val == Num_Lock => gdk::ModifierType::MOD2_MASK,
                val if val == ISO_Level3_Shift => gdk::ModifierType::MOD5_MASK,
                _ => gdk::ModifierType::empty(),
            }
        }

        tabwin.connect_key_release_event(move |tabwin, event| {
            let state = event.state() & !modifier_mask_for_keyval(event.keyval());
            if (state & !next_prev_modifiers) == state {
                let _ = from_ui_tx.send(FromUiMessage::TabwinAction(TabwinAction::Finished(tabwin.imp().selected())));
                tabwin.close();
            }

            glib::Propagation::Proceed
        });

        tabwin.imp().init_clients(clients, initial_selection);
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

    pub fn select_up(&self) -> Option<ObjectId> {
        self.imp().select_up()
    }

    pub fn select_down(&self) -> Option<ObjectId> {
        self.imp().select_down()
    }

    pub fn select_left(&self) -> Option<ObjectId> {
        self.imp().select_left()
    }

    pub fn select_right(&self) -> Option<ObjectId> {
        self.imp().select_right()
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
        prelude::WidgetExtManual,
        subclass::prelude::*,
        traits::{BoxExt, ContainerExt, GridExt, GtkWindowExt, LabelExt, StyleContextExt, WidgetExt},
    };
    use indexmap::IndexMap;
    use smithay::reexports::{calloop::channel, wayland_server::backend::ObjectId};

    use crate::ui::{
        FromUiMessage,
        tabwin::{TabwinAction, TabwinMetrics, calculate_tabwin_metrics},
        util::WidgetClassSubclassExtExt,
    };

    use super::{
        LISTVIEW_WIN_ICON_SIZE, TABWIN_WIDGET_NAME, TabwinClient, TabwinMode, WIN_ICON_BORDER, WIN_ICON_SIZE, WIN_PREVIEW_SIZE,
        build_client_icon, load_icon,
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

        grid_cols: Cell<u32>,
        grid_rows: Cell<u32>,

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
                    .default_value(WIN_PREVIEW_SIZE)
                    .read_only()
                    .build(),
            );

            klass.install_style_property_from_pspec(
                glib::ParamSpecInt::builder("icon-size")
                    .nick("icon size")
                    .blurb("size of the app icon")
                    .minimum(1)
                    .default_value(WIN_ICON_SIZE)
                    .read_only()
                    .build(),
            );

            klass.install_style_property_from_pspec(
                glib::ParamSpecInt::builder("listview-icon-size")
                    .nick("listview icon size")
                    .blurb("ize of the app icon in listview mode")
                    .minimum(1)
                    .default_value(LISTVIEW_WIN_ICON_SIZE)
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
            self.grid_cols.replace(icon_list_data.grid_cols);
            self.grid_rows.replace(icon_list_data.grid_rows);

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
                    // Make it so the toplevel window gets all key events.
                    window_button.set_events(window_button.events() & !(gdk::EventMask::KEY_PRESS_MASK | gdk::EventMask::KEY_RELEASE_MASK));

                    if let Some(from_ui_tx) = self.from_ui_tx.borrow().as_ref().cloned() {
                        let id = RefCell::new(Some(id.clone()));
                        // We have to use 'button-press-event' here, because 'activate' will not
                        // fire if there are modifier keys held, which will often be the case with
                        // the tabwin.
                        window_button.connect_button_press_event(move |button, _| {
                            if let Some(window) = button.toplevel().and_then(|toplevel| toplevel.downcast::<gtk::Window>().ok())
                                && let Some(id) = id.take()
                            {
                                let _ = from_ui_tx.send(FromUiMessage::TabwinAction(TabwinAction::Finished(Some(id))));
                                window.close();
                                glib::Propagation::Stop
                            } else {
                                glib::Propagation::Proceed
                            }
                        });
                    }

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
            let scale = monitor.scale_factor();
            let n_clients = clients.len();
            let TabwinMetrics {
                cycle_preview,
                icon_size,
                label_height,
                grid_cols,
                grid_rows,
            } = calculate_tabwin_metrics(self.mode.get(), n_clients, self.cycle_preview.get(), &monitor, Some(&instance));

            let icons = clients
                .into_iter()
                .map(|client| {
                    let icon = if self.mode.get() == TabwinMode::Grid && cycle_preview {
                        build_client_icon(
                            client.preview_icon,
                            client.app_icon,
                            icon_size,
                            icon_size,
                            scale,
                            client.is_minimized,
                        )
                    } else {
                        load_icon(client.app_icon, icon_size, icon_size, scale).unwrap_or_else(|| {
                            let scaled_size = (icon_size * scale as u32) as i32;
                            let blank = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, scaled_size, scaled_size)
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
            self.select_sequential(|index, n_clients| (index + 1) % n_clients)
        }

        pub(super) fn select_previous(&self) -> Option<ObjectId> {
            self.select_sequential(|index, n_clients| if index == 0 { n_clients - 1 } else { index - 1 })
        }

        pub(super) fn select_up(&self) -> Option<ObjectId> {
            let dec = |i: usize, total: usize| if i == 0 { total - 1 } else { i - 1 };
            match self.mode.get() {
                TabwinMode::Grid => self.select_perpendicular(dec),
                TabwinMode::List => self.select_sequential(dec),
            }
        }

        pub(super) fn select_down(&self) -> Option<ObjectId> {
            let inc = |i: usize, total: usize| (i + 1) % total;
            match self.mode.get() {
                TabwinMode::Grid => self.select_perpendicular(inc),
                TabwinMode::List => self.select_sequential(inc),
            }
        }

        pub(super) fn select_left(&self) -> Option<ObjectId> {
            let dec = |i: usize, total: usize| if i == 0 { total - 1 } else { i - 1 };
            match self.mode.get() {
                TabwinMode::Grid => self.select_sequential(dec),
                TabwinMode::List => self.select_perpendicular(dec),
            }
        }

        pub(super) fn select_right(&self) -> Option<ObjectId> {
            let inc = |i: usize, total: usize| (i + 1) % total;
            match self.mode.get() {
                TabwinMode::Grid => self.select_sequential(inc),
                TabwinMode::List => self.select_perpendicular(inc),
            }
        }

        fn select_sequential(&self, advance: impl Fn(usize, usize) -> usize) -> Option<ObjectId> {
            let cur_selected = self.selected_client.borrow().clone();
            if let Some(cur_selected) = cur_selected
                && let client_widgets = self.client_widgets.borrow()
                && let Some(index) = client_widgets.get_index_of(&cur_selected)
                && let new_index = advance(index, client_widgets.len())
                && let Some((id, _)) = client_widgets.get_index(new_index)
            {
                let id = id.clone();
                drop(client_widgets);
                self.set_selected(id.clone());
                Some(id)
            } else {
                None
            }
        }

        fn select_perpendicular(&self, advance: impl Fn(usize, usize) -> usize) -> Option<ObjectId> {
            let cur_selected = self.selected_client.borrow().clone();
            if let Some(cur_selected) = cur_selected
                && let client_widgets = self.client_widgets.borrow()
                && let Some(index) = client_widgets.get_index_of(&cur_selected)
                && let new_index = {
                    let n_clients = client_widgets.len();
                    let (primary, secondary) = match self.mode.get() {
                        TabwinMode::Grid => (self.grid_cols.get() as usize, self.grid_rows.get() as usize),
                        TabwinMode::List => (self.grid_rows.get() as usize, self.grid_cols.get() as usize),
                    };
                    let total = primary * secondary;

                    let perp_index = (index % primary) * secondary + (index / primary);
                    let mut new_perp = perp_index;
                    loop {
                        new_perp = advance(new_perp, total);
                        let new_index = (new_perp % secondary) * primary + (new_perp / secondary);
                        if new_index < n_clients {
                            break new_index;
                        }
                    }
                }
                && let Some((id, _)) = client_widgets.get_index(new_index)
            {
                let id = id.clone();
                drop(client_widgets);
                self.set_selected(id.clone());
                Some(id)
            } else {
                None
            }
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
    scale: i32,
    is_minimized: bool,
) -> gdk_pixbuf::Pixbuf {
    let phys_width = width * scale as u32;
    let phys_height = height * scale as u32;
    let icon_pixbuf = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, phys_width as i32, phys_height as i32)
        .expect("failed to create empty pixbuf");
    icon_pixbuf.fill(0);

    if let Some(app_content) = load_icon(preview_icon, width, height, scale) {
        let aw = app_content.width();
        let ah = app_content.height();
        app_content.copy_area(
            0,
            0,
            aw,
            ah,
            &icon_pixbuf,
            (phys_width as i32 - aw) / 2,
            (phys_height as i32 - ah) / 2,
        );
    }

    let small_icon_size = (width / 4).min(height / 4).min(48 / scale as u32);
    if let Some(small_icon) = load_icon(app_icon, small_icon_size, small_icon_size, scale) {
        let phys_small = small_icon_size * scale as u32;
        small_icon.composite(
            &icon_pixbuf,
            ((phys_width - phys_small) / 2) as i32,
            (phys_height - phys_small) as i32,
            phys_small as i32,
            phys_small as i32,
            (phys_width - phys_small) as f64 / 2.,
            (phys_height - phys_small) as f64,
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

fn load_icon(icon: Option<ImageData>, final_width: u32, final_height: u32, scale: i32) -> Option<gdk_pixbuf::Pixbuf> {
    let icon_theme = gtk::IconTheme::default().expect("failed to get default icon theme");
    icon.and_then(|icon| icon.load(final_width, final_height, scale as f64, &icon_theme))
        .or_else(|| {
            icon_theme
                .load_icon("xfwm4-default", final_width.max(final_height) as i32, scale as f64)
                .ok()
        })
}

pub fn create(config: TabwinConfig, from_ui_tx: channel::Sender<FromUiMessage>, style_provider: Option<&gtk::CssProvider>) -> Tabwin {
    let tabwin = Tabwin::new(
        config.mode,
        config.cycle_preview,
        config.clients,
        config.initial_selection,
        from_ui_tx,
        style_provider,
        config.next_shortcut,
        config.prev_shortcut,
        config.up_shortcut,
        config.down_shortcut,
        config.left_shortcut,
        config.right_shortcut,
        config.cancel_shortcut,
    );
    tabwin.set_opacity(config.window_opacity);
    tabwin
}

/// Guess the possible icon sizes we'll need based on how the tabwin draws
///
/// This checks each monitor (since different monitors at different resolutions could hold more or
/// fewer icons at a particular size) and passes it a bunch of client counts, given the tabwin mode
/// and cycle_preview setting.
pub fn guess_icon_sizes(mode: TabwinMode, cycle_preview: bool) -> HashSet<i32> {
    gdk::Display::default()
        .map(|display| (0..display.n_monitors()).flat_map(|i| display.monitor(i)).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .flat_map(|monitor| {
            (1..=61)
                .step_by(4)
                .map(move |n_clients| calculate_tabwin_metrics(mode, n_clients, cycle_preview, &monitor, None).icon_size as i32)
        })
        .collect()
}

fn calculate_tabwin_metrics(
    mode: TabwinMode,
    n_clients: usize,
    mut cycle_preview: bool,
    monitor: &gdk::Monitor,
    tabwin: Option<&Tabwin>,
) -> TabwinMetrics {
    let monitor_width = monitor.geometry().width();
    let monitor_height = monitor.geometry().height();

    let label_height = tabwin
        .map(|tabwin| {
            let layout = tabwin.create_pango_layout(Some(""));
            layout.pixel_size().1
        })
        .filter(|height| *height > 0)
        .unwrap_or(LABEL_HEIGHT as i32) as u32;

    match mode {
        TabwinMode::Grid => {
            let calc_grid_size = move |size_request: u32| {
                let grid_cols = (monitor_width as f64 * WIN_MAX_RATIO / size_request as f64).floor() as u32;
                let grid_rows = (n_clients as f64 / grid_cols as f64).ceil() as u32;
                (grid_cols, grid_rows)
            };

            let standard_icon_size = tabwin
                .map(|tabwin| tabwin.style_property::<i32>("icon-size"))
                .or_else(|| style_property_value_for_type(Tabwin::static_type(), "icon_size"))
                .unwrap_or(WIN_ICON_SIZE) as u32;

            let mut icon_size = if cycle_preview {
                tabwin
                    .map(|tabwin| tabwin.style_property::<i32>("preview-size"))
                    .or_else(|| style_property_value_for_type(Tabwin::static_type(), "preview-size"))
                    .unwrap_or(WIN_PREVIEW_SIZE) as u32
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

            TabwinMetrics {
                cycle_preview,
                icon_size,
                label_height,
                grid_cols,
                grid_rows,
            }
        }

        TabwinMode::List => {
            let icon_size = tabwin
                .map(|tabwin| tabwin.style_property::<i32>("listview-icon-size"))
                .or_else(|| style_property_value_for_type(Tabwin::static_type(), "listview-icon-size"))
                .unwrap_or(LISTVIEW_WIN_ICON_SIZE) as u32;
            let grid_rows = ((monitor_height as f64 * WIN_MAX_RATIO) / (icon_size + 2 * WIN_ICON_BORDER) as f64).floor() as u32;
            let grid_cols = (n_clients as f64 / grid_rows as f64).ceil() as u32;

            TabwinMetrics {
                cycle_preview: false,
                icon_size,
                label_height,
                grid_cols,
                grid_rows,
            }
        }
    }
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
