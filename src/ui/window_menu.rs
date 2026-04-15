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

use std::cell::Cell;

use gettextrs::gettext;
use glib::clone;
use gtk::{
    cairo,
    gdk::Gravity,
    glib,
    prelude::WidgetExtManual,
    traits::{CheckMenuItemExt, GtkMenuExt, GtkMenuItemExt, MenuShellExt, WidgetExt},
};

use crate::ui::compositor_ui_protocol::proto::xfwl4_ui_window_menu_v1::{Direction, StackingState};

pub const WINDOW_MENU_TOPLEVEL_TITLE: &str = "WindowMenu";

#[derive(Debug, Clone, Copy, PartialEq, glib::Boxed)]
#[boxed_type(name = "WindowMenuAction")]
pub enum WindowMenuAction {
    ToggleMaximize,
    Minimize,
    MinimizeOtherWindows,
    Move,
    Resize,
    StackOnTop,
    StackNormal,
    StackBelow,
    ToggleShade,
    Fullscreen,
    ToggleSticky,
    MoveToWorkspace(u32),
    MoveToOutput(Direction),
    Close,
}

pub fn create_anchor_window() -> gtk::Window {
    let window = gtk::Window::builder()
        .type_(gtk::WindowType::Popup)
        .title(WINDOW_MENU_TOPLEVEL_TITLE)
        .default_width(1)
        .width_request(1)
        .default_height(1)
        .height_request(1)
        .decorated(false)
        .opacity(0.)
        .build();
    window.add_events(gtk::gdk::EventMask::BUTTON_PRESS_MASK | gtk::gdk::EventMask::BUTTON_RELEASE_MASK);
    window.realize();

    if let Some(gdk_window) = window.window() {
        let region = cairo::Region::create_rectangle(&cairo::RectangleInt::new(0, 0, 1, 1));
        gdk_window.input_shape_combine_region(&region, 0, 0);
    }

    window
}

#[allow(clippy::too_many_arguments)]
pub fn create_menu<F1, F2>(
    maximized: Option<bool>,
    can_minimize: bool,
    can_move: bool,
    can_resize: bool,
    stacking_state: StackingState,
    shaded: Option<bool>,
    fullscreen: Option<bool>,
    sticky: bool,
    current_workspace: Option<u32>,
    workspace_names: Vec<String>,
    adjacent_outputs: Vec<Direction>,
    can_close: bool,
    parent: &gtk::Window,
    action_callback: F1,
    dismissed_callback: F2,
) -> gtk::Menu
where
    F1: Fn(WindowMenuAction) + Clone + 'static,
    F2: Fn() + Clone + 'static,
{
    let menu = gtk::Menu::builder().attach_widget(parent).reserve_toggle_size(true).build();

    let maximize = gtk::MenuItem::builder()
        .label(match maximized {
            Some(false) | None => gettext("Ma_ximize"),
            Some(true) => gettext("Unma_ximize"),
        })
        .use_underline(true)
        .sensitive(maximized.is_some())
        .build();
    menu.append(&maximize);
    maximize.connect_activate(clone!(@strong action_callback => move|_| action_callback(WindowMenuAction::ToggleMaximize)));

    let minimize = gtk::MenuItem::builder()
        .label(gettext("Mi_nimize"))
        .use_underline(true)
        .sensitive(can_minimize)
        .build();
    menu.append(&minimize);
    minimize.connect_activate(clone!(@strong action_callback => move |_| action_callback(WindowMenuAction::Minimize)));

    let minimize_other = gtk::MenuItem::builder()
        .label(gettext("Minimize _Other Windows"))
        .use_underline(true)
        .build();
    menu.append(&minimize_other);
    minimize_other.connect_activate(clone!(@strong action_callback => move |_| action_callback(WindowMenuAction::MinimizeOtherWindows)));

    let move_mi = gtk::MenuItem::builder()
        .label(gettext("_Move"))
        .use_underline(true)
        .sensitive(can_move)
        .build();
    menu.append(&move_mi);
    move_mi.connect_activate(clone!(@strong action_callback => move |_| action_callback(WindowMenuAction::Move)));

    let resize = gtk::MenuItem::builder()
        .label(gettext("_Resize"))
        .use_underline(true)
        .sensitive(can_resize)
        .build();
    menu.append(&resize);
    resize.connect_activate(clone!(@strong action_callback => move |_| action_callback(WindowMenuAction::Resize)));

    menu.append(&gtk::SeparatorMenuItem::new());

    let stack_top = gtk::RadioMenuItem::builder()
        .label(gettext("Always on _Top"))
        .use_underline(true)
        .active(stacking_state == StackingState::AlwaysOnTop)
        .build();
    menu.append(&stack_top);
    stack_top.connect_activate(
        clone!(@strong action_callback => move |item| if item.is_active() { action_callback(WindowMenuAction::StackOnTop); }),
    );

    let stack_normal = gtk::RadioMenuItem::from_widget(&stack_top);
    stack_normal.set_label(&gettext("_Same as Other Windows"));
    stack_normal.set_use_underline(true);
    stack_normal.set_active(stacking_state == StackingState::Normal);
    menu.append(&stack_normal);
    stack_normal.connect_activate(
        clone!(@strong action_callback => move |item| if item.is_active() { action_callback(WindowMenuAction::StackNormal); }),
    );

    let stack_below = gtk::RadioMenuItem::from_widget(&stack_top);
    stack_below.set_label(&gettext("Always _Below Other Windows"));
    stack_below.set_use_underline(true);
    stack_below.set_active(stacking_state == StackingState::AlwaysBelow);
    menu.append(&stack_below);
    stack_below.connect_activate(
        clone!(@strong action_callback => move |item| if item.is_active() { action_callback(WindowMenuAction::StackBelow); }),
    );

    menu.append(&gtk::SeparatorMenuItem::new());

    let shade = gtk::MenuItem::builder()
        .label(match shaded {
            Some(false) | None => gettext("Roll Window Up"),
            Some(true) => gettext("Roll Window Down"),
        })
        .sensitive(shaded.is_some())
        .build();
    menu.append(&shade);
    shade.connect_activate(clone!(@strong action_callback => move |_| action_callback(WindowMenuAction::ToggleShade)));

    let fullscreen = gtk::MenuItem::builder()
        .label(match fullscreen {
            Some(false) | None => gettext("_Fullscreen"),
            Some(true) => gettext("Un_fullscreen"),
        })
        .use_underline(true)
        .sensitive(fullscreen.is_some())
        .build();
    menu.append(&fullscreen);
    fullscreen.connect_activate(clone!(@strong action_callback => move |_| action_callback(WindowMenuAction::Fullscreen)));

    menu.append(&gtk::SeparatorMenuItem::new());

    let sticky = gtk::CheckMenuItem::builder()
        .label(gettext("Always on _Visible Workspace"))
        .use_underline(true)
        .active(sticky)
        .build();
    menu.append(&sticky);
    sticky.connect_activate(clone!(@strong action_callback => move |_| action_callback(WindowMenuAction::ToggleSticky)));

    let move_workspace = gtk::MenuItem::builder()
        .label(gettext("Move to Another _Workspace"))
        .use_underline(true)
        .sensitive(!workspace_names.is_empty())
        .build();
    menu.append(&move_workspace);

    let move_ws_menu = gtk::Menu::new();
    move_workspace.set_submenu(Some(&move_ws_menu));

    for (i, name) in workspace_names.into_iter().enumerate() {
        let move_to_ws = gtk::MenuItem::builder()
            .label(name)
            .sensitive(current_workspace.is_none_or(|cur_ws| cur_ws as usize != i))
            .build();
        move_ws_menu.append(&move_to_ws);
        move_to_ws
            .connect_activate(clone!(@strong action_callback => move |_| action_callback(WindowMenuAction::MoveToWorkspace(i as u32))));
    }

    let monitor_move_items = adjacent_outputs
        .into_iter()
        .map(|direction| {
            let label = match direction {
                Direction::Left => gettext("Monitor Left"),
                Direction::Right => gettext("Monitor Right"),
                Direction::Up => gettext("Monitor Up"),
                Direction::Down => gettext("Monitor Down"),
            };
            let item = gtk::MenuItem::builder().label(&label).build();
            item.connect_activate(clone!(@strong action_callback => move |_| action_callback(WindowMenuAction::MoveToOutput(direction))));
            item
        })
        .collect::<Vec<_>>();
    if !monitor_move_items.is_empty() {
        let move_monitor = gtk::MenuItem::builder().label("Move to Another Monitor").build();
        menu.append(&move_monitor);

        let move_monitor_menu = gtk::Menu::new();
        move_monitor.set_submenu(Some(&move_monitor_menu));

        for item in monitor_move_items {
            move_monitor_menu.append(&item);
        }
    }

    menu.append(&gtk::SeparatorMenuItem::new());

    let close = gtk::MenuItem::builder()
        .label(gettext("_Close"))
        .use_underline(true)
        .sensitive(can_close)
        .build();
    menu.append(&close);
    close.connect_activate(clone!(@strong action_callback => move |_| action_callback(WindowMenuAction::Close)));

    let button_press_id = parent.connect_button_press_event(clone!(@strong menu => move |window, event| {
        if event.button() == gtk::gdk::BUTTON_SECONDARY {
            menu.popup_at_widget(window, Gravity::NorthWest, Gravity::NorthWest, Some(event));
        }

        glib::Propagation::Proceed
    }));

    let button_press_id = Cell::new(Some(button_press_id));
    menu.connect_deactivate(clone!(@strong parent, @strong dismissed_callback => move |menu| {
        if let Some(button_press_id) = button_press_id.take() {
            glib::signal_handler_disconnect(&parent, button_press_id);
        }

        // Even though we don't keep a reference to the menu anywhere, I think the anchor GtkWindow
        // keeps a reference, so we need to destroy it on our own.  But we have to do it in an idle
        // function, because GtkMenu sends the GtkMenuItem::activate and ::cancel signals *after*
        // the GtkMenuShell::deactivate signal.  If we destroy it now, we'll never get the menu
        // item signal.
        glib::idle_add_local_once(clone!(@strong menu, @strong dismissed_callback => move || {
            unsafe { menu.destroy() }
            dismissed_callback();
        }));
    }));

    menu.show_all();

    menu
}
