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
use smithay::{
    reexports::{
        calloop::channel::Sender,
        wayland_server::backend::{GlobalId, ObjectId},
    },
    utils::{Logical, Rectangle},
};

use crate::ui::FromUiMessage;

pub const WINDOW_MENU_TOPLEVEL_TITLE: &str = "WindowMenu";

#[derive(Debug, Clone, PartialEq)]
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
    MoveToOutput(Rectangle<i32, Logical>),
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StackingState {
    Normal,
    AlwaysOnTop,
    AlwaysBelow,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShadeState {
    Normal,
    Shaded,
    CannotShade,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MaximizeState {
    Normal,
    Maximized,
    CannotMaximize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FullscreenState {
    Normal,
    Fullscreen,
    CannotFullscreen,
}

#[derive(Debug, Clone)]
pub struct WindowMenuState {
    pub window_id: ObjectId,
    pub maximize_state: MaximizeState,
    pub can_minimize: bool,
    pub can_move: bool,
    pub can_resize: bool,
    pub stacking_state: StackingState,
    pub shade_state: ShadeState,
    pub fullscreen_state: FullscreenState,
    pub sticky: bool,
    pub can_move_workspaces: bool,
    pub current_workspace: u32,
    pub workspace_names: Vec<String>,
    pub current_monitor: Option<(GlobalId, Rectangle<i32, Logical>)>,
    pub monitors: Vec<(GlobalId, Rectangle<i32, Logical>)>,
    pub can_close: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Direction {
    Up,
    Down,
    Left,
    Right,
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

fn connect_radio_action<MI: GtkMenuItemExt + CheckMenuItemExt>(
    item: &MI,
    window_id: &ObjectId,
    action: WindowMenuAction,
    tx: &Sender<FromUiMessage>,
) {
    let data = Cell::new(Some((window_id.clone(), action)));
    item.connect_activate(clone!(@strong tx => move |item| {
        if item.is_active()
            && let Some((window_id, action)) = data.take()
        {
            let _ = tx.send(FromUiMessage::WindowMenuAction(window_id, action));
        }
    }));
}

fn connect_action<MI: GtkMenuItemExt>(item: &MI, window_id: &ObjectId, action: WindowMenuAction, tx: &Sender<FromUiMessage>) {
    let data = Cell::new(Some((window_id.clone(), action)));
    item.connect_activate(clone!(@strong tx => move |_| {
        if let Some((window_id, action)) = data.take() {
            let _ = tx.send(FromUiMessage::WindowMenuAction(window_id, action));
        }
    }));
}

pub fn create_menu(state: WindowMenuState, parent: &gtk::Window, tx: &Sender<FromUiMessage>) -> gtk::Menu {
    let menu = gtk::Menu::builder().attach_widget(parent).reserve_toggle_size(true).build();

    let maximize = gtk::MenuItem::builder()
        .label(match state.maximize_state {
            MaximizeState::Normal | MaximizeState::CannotMaximize => gettext("Ma_ximize"),
            MaximizeState::Maximized => gettext("Unma_ximize"),
        })
        .use_underline(true)
        .sensitive(state.maximize_state != MaximizeState::CannotMaximize)
        .build();
    menu.append(&maximize);
    connect_action(&maximize, &state.window_id, WindowMenuAction::ToggleMaximize, tx);

    let minimize = gtk::MenuItem::builder()
        .label(gettext("Mi_nimize"))
        .use_underline(true)
        .sensitive(state.can_minimize)
        .build();
    menu.append(&minimize);
    connect_action(&minimize, &state.window_id, WindowMenuAction::Minimize, tx);

    let minimize_other = gtk::MenuItem::builder()
        .label(gettext("Minimize _Other Windows"))
        .use_underline(true)
        .build();
    menu.append(&minimize_other);
    connect_action(&minimize_other, &state.window_id, WindowMenuAction::MinimizeOtherWindows, tx);

    let move_mi = gtk::MenuItem::builder()
        .label(gettext("_Move"))
        .use_underline(true)
        .sensitive(state.can_move)
        .build();
    menu.append(&move_mi);
    connect_action(&move_mi, &state.window_id, WindowMenuAction::Move, tx);

    let resize = gtk::MenuItem::builder()
        .label(gettext("_Resize"))
        .use_underline(true)
        .sensitive(state.can_resize)
        .build();
    menu.append(&resize);
    connect_action(&resize, &state.window_id, WindowMenuAction::Resize, tx);

    menu.append(&gtk::SeparatorMenuItem::new());

    let stack_top = gtk::RadioMenuItem::builder()
        .label(gettext("Always on _Top"))
        .use_underline(true)
        .active(state.stacking_state == StackingState::AlwaysOnTop)
        .build();
    menu.append(&stack_top);
    connect_radio_action(&stack_top, &state.window_id, WindowMenuAction::StackOnTop, tx);

    let stack_normal = gtk::RadioMenuItem::from_widget(&stack_top);
    stack_normal.set_label(&gettext("_Same as Other Windows"));
    stack_normal.set_use_underline(true);
    stack_normal.set_active(state.stacking_state == StackingState::Normal);
    menu.append(&stack_normal);
    connect_radio_action(&stack_normal, &state.window_id, WindowMenuAction::StackNormal, tx);

    let stack_below = gtk::RadioMenuItem::from_widget(&stack_top);
    stack_below.set_label(&gettext("Always _Below Other Windows"));
    stack_below.set_use_underline(true);
    stack_below.set_active(state.stacking_state == StackingState::AlwaysBelow);
    menu.append(&stack_below);
    connect_radio_action(&stack_below, &state.window_id, WindowMenuAction::StackBelow, tx);

    menu.append(&gtk::SeparatorMenuItem::new());

    let shade = gtk::MenuItem::builder()
        .label(match state.shade_state {
            ShadeState::Normal | ShadeState::CannotShade => gettext("Roll Window Up"),
            ShadeState::Shaded => gettext("Roll Window Down"),
        })
        .sensitive(state.shade_state != ShadeState::CannotShade)
        .build();
    menu.append(&shade);
    connect_action(&shade, &state.window_id, WindowMenuAction::ToggleShade, tx);

    let fullscreen = gtk::MenuItem::builder()
        .label(match state.fullscreen_state {
            FullscreenState::Normal | FullscreenState::CannotFullscreen => gettext("_Fullscreen"),
            FullscreenState::Fullscreen => gettext("Un_fullscreen"),
        })
        .use_underline(true)
        .sensitive(state.fullscreen_state != FullscreenState::CannotFullscreen)
        .build();
    menu.append(&fullscreen);
    connect_action(&fullscreen, &state.window_id, WindowMenuAction::Fullscreen, tx);

    // TODO: "Context _Help" (maybe?  kinda obsolete?)

    menu.append(&gtk::SeparatorMenuItem::new());

    let sticky = gtk::CheckMenuItem::builder()
        .label(gettext("Always on _Visible Workspace"))
        .use_underline(true)
        .active(state.sticky)
        .build();
    menu.append(&sticky);
    connect_action(&sticky, &state.window_id, WindowMenuAction::ToggleSticky, tx);

    let move_workspace = gtk::MenuItem::builder()
        .label(gettext("Move to Another _Workspace"))
        .use_underline(true)
        .sensitive(state.workspace_names.len() > 1 && state.can_move_workspaces)
        .build();
    menu.append(&move_workspace);

    let move_ws_menu = gtk::Menu::new();
    move_workspace.set_submenu(Some(&move_ws_menu));

    for (i, name) in state.workspace_names.into_iter().enumerate() {
        let move_to_ws = gtk::MenuItem::builder()
            .label(name)
            .sensitive(i != state.current_workspace as usize)
            .build();
        move_ws_menu.append(&move_to_ws);
        connect_action(&move_to_ws, &state.window_id, WindowMenuAction::MoveToWorkspace(i as u32), tx);
    }

    if state.monitors.len() > 1
        && let Some(current_monitor) = state.current_monitor
    {
        let directions = [
            (gettext("Monitor Left"), Direction::Left),
            (gettext("Monitor Right"), Direction::Right),
            (gettext("Monitor Up"), Direction::Up),
            (gettext("Monitor Down"), Direction::Down),
        ];

        let monitor_move_items = directions
            .into_iter()
            .flat_map(|(label, direction)| {
                adjacent_monitor_in_direction(&current_monitor, &state.monitors, direction).map(|output_rect| {
                    let item = gtk::MenuItem::builder().label(label).build();
                    connect_action(&item, &state.window_id, WindowMenuAction::MoveToOutput(output_rect), tx);
                    item
                })
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
    }

    menu.append(&gtk::SeparatorMenuItem::new());

    let close = gtk::MenuItem::builder()
        .label(gettext("_Close"))
        .use_underline(true)
        .sensitive(state.can_close)
        .build();
    menu.append(&close);
    connect_action(&close, &state.window_id, WindowMenuAction::Close, tx);

    let button_press_id = parent.connect_button_press_event(clone!(@strong menu => move |window, event| {
        if event.button() == gtk::gdk::BUTTON_SECONDARY {
            menu.popup_at_widget(window, Gravity::NorthWest, Gravity::NorthWest, Some(event));
        }

        glib::Propagation::Proceed
    }));

    let button_press_id = Cell::new(Some(button_press_id));
    menu.connect_deactivate(clone!(@strong parent, @strong tx => move |menu| {
        if let Some(button_press_id) = button_press_id.take() {
            glib::signal_handler_disconnect(&parent, button_press_id);
        }

        // Even though we don't keep a reference to the menu anywhere, I think the anchor GtkWindow
        // keeps a reference, so we need to destroy it on our own.  But we have to do it in an idle
        // function, because GtkMenu sends the GtkMenuItem::activate and ::cancel signals *after*
        // the GtkMenuShell::deactivate signal.  If we destroy it now, we'll never get the menu
        // item signal.
        glib::idle_add_local_once(clone!(@strong menu, @strong tx => move || {
            unsafe { menu.destroy() }
            let _ = tx.send(FromUiMessage::WindowMenuDismissed);
        }));
    }));

    menu.show_all();

    menu
}

fn adjacent_monitor_in_direction(
    cur: &(GlobalId, Rectangle<i32, Logical>),
    monitors: &[(GlobalId, Rectangle<i32, Logical>)],
    direction: Direction,
) -> Option<Rectangle<i32, Logical>> {
    let cur_rect = cur.1;
    monitors
        .iter()
        .filter(|(id, _)| *id != cur.0)
        .filter(|(_, rect)| {
            let (in_direction, has_overlap) = match direction {
                Direction::Left => (
                    rect.loc.x + rect.size.w <= cur_rect.loc.x,
                    rect.loc.y < cur_rect.loc.y + cur_rect.size.h && rect.loc.y + rect.size.h > cur_rect.loc.y,
                ),
                Direction::Right => (
                    rect.loc.x >= cur_rect.loc.x + cur_rect.size.w,
                    rect.loc.y < cur_rect.loc.y + cur_rect.size.h && rect.loc.y + rect.size.h > cur_rect.loc.y,
                ),
                Direction::Up => (
                    rect.loc.y + rect.size.h <= cur_rect.loc.y,
                    rect.loc.x < cur_rect.loc.x + cur_rect.size.w && rect.loc.x + rect.size.w > cur_rect.loc.x,
                ),
                Direction::Down => (
                    rect.loc.y >= cur_rect.loc.y + cur_rect.size.h,
                    rect.loc.x < cur_rect.loc.x + cur_rect.size.w && rect.loc.x + rect.size.w > cur_rect.loc.x,
                ),
            };
            in_direction && has_overlap
        })
        .min_by_key(|(_, rect)| match direction {
            Direction::Left => cur_rect.loc.x - (rect.loc.x + rect.size.w),
            Direction::Right => rect.loc.x - (cur_rect.loc.x + cur_rect.size.w),
            Direction::Up => cur_rect.loc.y - (rect.loc.y + rect.size.h),
            Direction::Down => rect.loc.y - (cur_rect.loc.y + cur_rect.size.h),
        })
        .map(|(_, rect)| *rect)
}
