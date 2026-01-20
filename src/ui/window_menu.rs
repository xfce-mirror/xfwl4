use glib::clone;
use gtk::{
    gdk::{Gravity, traits::SeatExt},
    glib,
    traits::{CheckMenuItemExt, GtkMenuExt, GtkMenuItemExt, GtkWindowExt, MenuShellExt, WidgetExt},
};

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

pub fn pop_up(state: WindowMenuState) {
    let window = gtk::Window::builder()
        .title("Window Menu")
        .default_width(1)
        .default_height(1)
        .width_request(1)
        .height_request(1)
        .decorated(false)
        .accept_focus(false)
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

    window.show_all();
    menu.show_all();

    let mut event = gtk::gdk::Event::new(gtk::gdk::EventType::ButtonPress);
    event.set_screen(gtk::gdk::Screen::default().as_ref());
    let pointer = gtk::gdk::Display::default()
        .and_then(|display| display.default_seat())
        .and_then(|seat| seat.pointer());
    event.set_device(pointer.as_ref());

    /*
    let eventp = ToGlibPtrMut::<'_, *mut gtk::gdk::ffi::GdkEvent>::to_glib_none_mut(&mut event);
    let event_button = unsafe { &mut (*eventp.0).button };
    event_button.window = window.window().unwrap().to_glib_full();
    event_button.button gtk::gdk::ffi::GDK_BUTTON_PRIMARY;
    event_button.
    */

    menu.connect_selection_done(clone!(@strong window => move|_| {
        window.close();
    }));

    menu.popup_at_widget(&window, Gravity::Center, Gravity::NorthWest, Some(&event));
}
