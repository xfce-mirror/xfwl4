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

use glib::{ObjectExt, Sender, SourceId, clone};
use gtk::traits::{BoxExt, ButtonExt, ContainerExt, GtkWindowExt, WidgetExt};

use crate::ui::compositor_ui_protocol::proto::xfwl4_ui_dialog_v1::Xfwl4UiDialogV1;

#[derive(Debug)]
pub struct DialogState {
    pub proxy: Xfwl4UiDialogV1,
    pub action_tx: Sender<String>,
    pub action_rx_id: SourceId,
    pub config: Option<DialogConfig>,
    pub dialog: Option<gtk::Window>,
    pub primary_button: Option<gtk::Button>,
}

#[derive(Debug)]
pub struct DialogConfig {
    pub title: String,
    pub primary_text: Option<String>,
    pub secondary_text: Option<String>,
    pub icon_name: Option<String>,
    pub cancel_button: DialogButton,
    pub additional_buttons: Vec<DialogButton>,
}

#[derive(Debug)]
pub struct DialogButton {
    pub text: String,
    pub action_id: String,
}

pub fn show_dialog(config: DialogConfig, action_tx: Sender<String>) -> gtk::Window {
    let window = gtk::Window::builder()
        .title(config.title)
        .type_(gtk::WindowType::Toplevel)
        .type_hint(gtk::gdk::WindowTypeHint::Dialog)
        .icon_name(config.icon_name.as_deref().unwrap_or("dialog-info"))
        .resizable(false)
        .skip_pager_hint(true)
        .skip_taskbar_hint(true)
        .window_position(gtk::WindowPosition::Center)
        .resizable(false)
        .build();

    let top_vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
    window.add(&top_vbox);

    let hbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .margin(8)
        .build();
    top_vbox.pack_start(&hbox, true, true, 0);

    if let Some(icon_name) = config.icon_name {
        let image = gtk::Image::from_icon_name(Some(&icon_name), gtk::IconSize::Dialog);
        hbox.pack_start(&image, false, false, 0);
    }

    let vbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .valign(gtk::Align::Start)
        .build();
    hbox.pack_start(&vbox, true, true, 0);

    if let Some(primary_text) = config.primary_text {
        let escaped = glib::markup_escape_text(&primary_text);
        let text = format!("<big><b>{escaped}</b></big>");
        let label = gtk::Label::builder().label(text).use_markup(true).halign(gtk::Align::Start).build();
        vbox.pack_start(&label, false, false, 0);
    }

    if let Some(secondary_text) = config.secondary_text {
        let label = gtk::Label::builder().label(secondary_text).halign(gtk::Align::Start).build();
        vbox.pack_start(&label, false, false, 0);
    }

    let button_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(0)
        .homogeneous(true)
        .build();
    top_vbox.pack_end(&button_box, false, false, 0);

    let cancel_action_id = Rc::new(RefCell::new(Some(config.cancel_button.action_id.clone())));
    let cancel_id = window.connect_delete_event(clone!(@strong action_tx, @strong cancel_action_id => move |_, _| {
        if let Some(cancel_action_id) = cancel_action_id.borrow_mut().take() {
            let _ = action_tx.send(cancel_action_id);
        }
        glib::Propagation::Proceed
    }));

    let cancel_id = Rc::new(RefCell::new(Some(cancel_id)));
    for DialogButton { text, action_id } in std::iter::once(config.cancel_button).chain(config.additional_buttons) {
        let button = gtk::Button::with_label(&text);
        button_box.pack_start(&button, true, true, 0);

        let action_id = Rc::new(RefCell::new(Some(action_id)));
        button.connect_clicked(clone!(@strong action_tx, @strong window, @strong cancel_id => move |_| {
            if let Some(cancel_id) = cancel_id.borrow_mut().take() {
                window.disconnect(cancel_id);
            }
            if let Some(action_id) = action_id.borrow_mut().take() {
                let _ = action_tx.send(action_id);
            }
        }));
    }

    window.show_all();
    if let Some(last_button) = button_box.children().last() {
        last_button.set_can_default(true);
        window.set_default(Some(last_button));
        last_button.grab_focus();
    }

    window
}
