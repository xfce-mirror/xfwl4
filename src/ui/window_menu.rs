use std::sync::LazyLock;

use glib::clone;
use gtk::{
    gdk::Gravity,
    glib,
    prelude::WidgetExtManual,
    traits::{CheckMenuItemExt, GtkMenuExt, GtkMenuItemExt, GtkWindowExt, MenuShellExt, WidgetExt},
};
use regex::Regex;
use smithay::utils::{Logical, Point};

pub const WINDOW_MENU_TOPLEVEL_TITLE_PREFIX: &str = "WindowMenu-";

#[derive(Debug)]
pub enum WindowMenuAction {
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StackingState {
    Normal,
    AlwaysOnTop,
    AlwaysBelow,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RolledState {
    Normal,
    RolledUp,
    CannotRoll,
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
    pub location: Point<i32, Logical>,
    pub maximize_state: MaximizeState,
    pub can_minimize: bool,
    pub can_move: bool,
    pub can_resize: bool,
    pub stacking_state: StackingState,
    pub rolled_state: RolledState,
    pub fullscreen_state: FullscreenState,
    pub pinned: bool,
    pub can_move_workspaces: bool,
    pub current_workspace: u32,
    pub workspace_names: Vec<String>,
    pub can_close: bool,
}

pub fn parse_title(title: &str) -> Option<Point<i32, Logical>> {
    static TITLE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        let re_str = format!("{WINDOW_MENU_TOPLEVEL_TITLE_PREFIX}{}", r"\((?<x_coord>\d+),(?<y_coord>\d+)\)");
        regex::Regex::new(&re_str).unwrap()
    });

    tracing::debug!("parsing possible window menu title {title}");

    TITLE_REGEX.captures(title).and_then(|captures| {
        let x = captures.name("x_coord").and_then(|x| x.as_str().parse::<i32>().ok());
        let y = captures.name("y_coord").and_then(|y| y.as_str().parse::<i32>().ok());
        tracing::debug!("got captures, x?{}, y?{}", x.is_some(), y.is_some());

        match (x, y) {
            (Some(x), Some(y)) => Some(Point::new(x, y)),
            _ => None,
        }
    })
}

pub fn pop_up(state: WindowMenuState) {
    let window = gtk::Window::builder()
        .type_(gtk::WindowType::Popup)
        .title(format!(
            "{WINDOW_MENU_TOPLEVEL_TITLE_PREFIX}({},{})",
            state.location.x, state.location.y
        ))
        .default_width(1)
        .default_height(1)
        .decorated(false)
        .opacity(0.)
        .build();

    let menu = gtk::Menu::builder().attach_widget(&window).reserve_toggle_size(true).build();

    let maximize = gtk::MenuItem::builder()
        .label(match state.maximize_state {
            MaximizeState::Normal | MaximizeState::CannotMaximize => "Ma_ximize",
            MaximizeState::Maximized => "Unma_ximize",
        })
        .use_underline(true)
        .sensitive(state.maximize_state != MaximizeState::CannotMaximize)
        .build();
    menu.append(&maximize);

    let minimize = gtk::MenuItem::builder()
        .label("Mi_nimize")
        .use_underline(true)
        .sensitive(state.can_minimize)
        .build();
    menu.append(&minimize);

    let minimize_other = gtk::MenuItem::builder()
        .label("Minimize _Other Windows")
        .use_underline(true)
        .build();
    menu.append(&minimize_other);

    let move_mi = gtk::MenuItem::builder()
        .label("_Move")
        .use_underline(true)
        .sensitive(state.can_move)
        .build();
    menu.append(&move_mi);

    let resize_mi = gtk::MenuItem::builder()
        .label("_Resize")
        .use_underline(true)
        .sensitive(state.can_resize)
        .build();
    menu.append(&resize_mi);

    menu.append(&gtk::SeparatorMenuItem::new());

    let stack_top = gtk::RadioMenuItem::builder()
        .label("Always on _Top")
        .use_underline(true)
        .active(state.stacking_state == StackingState::AlwaysOnTop)
        .build();
    menu.append(&stack_top);

    let stack_normal = gtk::RadioMenuItem::from_widget(&stack_top);
    stack_normal.set_label("_Same as Other Windows");
    stack_normal.set_use_underline(true);
    stack_normal.set_active(state.stacking_state == StackingState::Normal);
    menu.append(&stack_normal);

    let stack_below = gtk::RadioMenuItem::from_widget(&stack_top);
    stack_below.set_label("_Same as Other Windows");
    stack_below.set_use_underline(true);
    stack_below.set_active(state.stacking_state == StackingState::AlwaysBelow);
    menu.append(&stack_below);

    menu.append(&gtk::SeparatorMenuItem::new());

    let roll = gtk::MenuItem::builder()
        .label(match state.rolled_state {
            RolledState::Normal | RolledState::CannotRoll => "Roll Window Up",
            RolledState::RolledUp => "Roll Window Down",
        })
        .sensitive(state.rolled_state != RolledState::CannotRoll)
        .build();
    menu.append(&roll);

    let fullscreen = gtk::MenuItem::builder()
        .label(match state.fullscreen_state {
            FullscreenState::Normal | FullscreenState::CannotFullscreen => "_Fullscreen",
            FullscreenState::Fullscreen => "Un_fullscreen",
        })
        .use_underline(true)
        .sensitive(state.fullscreen_state != FullscreenState::CannotFullscreen)
        .build();
    menu.append(&fullscreen);

    menu.append(&gtk::SeparatorMenuItem::new());

    let pinned = gtk::CheckMenuItem::builder()
        .label("Always on _Visible Workspace")
        .use_underline(true)
        .active(state.pinned)
        .build();
    menu.append(&pinned);

    // TODO: "Move to Another _Workspace"
    // TODO: "Move to Another Monitor"

    menu.append(&gtk::SeparatorMenuItem::new());

    let close = gtk::MenuItem::builder().label("_Close").use_underline(true).build();
    menu.append(&close);

    menu.connect_deactivate(clone!(@strong window => move|menu| {
        tracing::debug!("menu deactivate");
        menu.popdown();
        unsafe { menu.destroy(); }
        window.close();
        unsafe { window.destroy(); }
    }));
    menu.connect_destroy(|_| tracing::debug!("menu destroyed"));

    menu.show_all();

    window.add_events(gtk::gdk::EventMask::BUTTON_PRESS_MASK | gtk::gdk::EventMask::BUTTON_RELEASE_MASK);
    window.connect_button_press_event(move |window, event| {
        if event.button() == gtk::gdk::BUTTON_SECONDARY {
            tracing::debug!("popping up menu");
            //menu.popup_at_pointer(Some(&event));

            if let Some(rect_window) = window.window() {
                let rect = gtk::gdk::Rectangle::new(state.location.x, state.location.y, 1, 1);
                menu.popup_at_rect(&rect_window, &rect, Gravity::NorthWest, Gravity::NorthWest, Some(event));
            }

            //menu.popup_at_widget(&window, Gravity::NorthWest, Gravity::NorthWest, Some(&event));
        }

        glib::Propagation::Proceed
    });
    window.connect_destroy(|_| tracing::debug!("window destroyed"));

    window.show();
}
