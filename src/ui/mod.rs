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
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use glib::{ControlFlow, Receiver};
use gtk::traits::{ContainerExt, GtkWindowExt, WidgetExt};
use smithay::reexports::{
    calloop::channel::{self, Sender},
    wayland_server::backend::ObjectId,
};
use tracing::{error, warn};

use crate::ui::{
    tabwin::{TABWIN_DEFAULT_CSS, TABWIN_WIDGET_NAME, Tabwin, TabwinAction, TabwinClient, TabwinConfig, TabwinMode},
    window_menu::{WindowMenuAction, WindowMenuState},
};

mod gtk_settings;
pub mod tabwin;
mod theme;
mod util;
pub mod window_menu;

pub use gtk_settings::{FontSettings, PointerBehavior};

#[derive(Debug)]
pub enum ToUiMessage {
    WaylandDisplayReady,
    ProvideIconSizes(IconSizeHints),
    PrepareWindowMenu(Sender<()>, WindowMenuState),
    ShowTabwin(TabwinConfig),
    TabwinNext,
    TabwinPrevious,
    FinshTabwin,
    CancelTabwin,
    TabwinWindowAdded(TabwinClient),
    TabwinWindowRemoved(ObjectId),
    Quit,
}

#[derive(Debug)]
pub struct IconSizeHints {
    pub tabwin_mode: TabwinMode,
    pub tabwin_cycle_preview: bool,
}

#[derive(Debug)]
struct UiThreadState {
    from_ui_tx: channel::Sender<FromUiMessage>,

    tabwin: RefCell<Option<Tabwin>>,
    tabwin_style_provider: RefCell<Option<gtk::CssProvider>>,

    window_menu_anchor: RefCell<Option<gtk::Window>>,
}

#[derive(Debug)]
pub enum FromUiMessage {
    DefaultMainContextClaimed,
    IconThemeChanged(String),
    IconSizes(HashSet<i32>),
    WindowMenuAction(ObjectId, WindowMenuAction),
    WindowMenuDismissed,
    TabwinAction(TabwinAction),
    ThemeColorsChanged(HashMap<&'static str, gtk::gdk::RGBA>),
    FontSettingsChanged(FontSettings),
    PointerBehaviorSettingsChanged(PointerBehavior),
}

pub fn launch_ui_thread(to_ui_rx: Receiver<ToUiMessage>, from_ui_tx: channel::Sender<FromUiMessage>) -> JoinHandle<()> {
    thread::spawn(move || {
        if let Err(err) = thread_fn(to_ui_rx, from_ui_tx) {
            error!("Failed to run UI thread: {err}");
            std::process::exit(1);
        }
    })
}

fn thread_fn(to_ui_rx: Receiver<ToUiMessage>, from_ui_tx: channel::Sender<FromUiMessage>) -> anyhow::Result<()> {
    // Claim the global default main context so it gets created and no other thread creates it.
    let default_context = glib::MainContext::default();

    from_ui_tx.send(FromUiMessage::DefaultMainContextClaimed).unwrap();

    let wayland_display_ready = Arc::new(AtomicBool::new(false));
    let gtk_inited = Arc::new(AtomicBool::new(false));

    let state = Rc::new(UiThreadState {
        from_ui_tx: from_ui_tx.clone(),
        tabwin: RefCell::new(None),
        tabwin_style_provider: RefCell::new(None),
        window_menu_anchor: RefCell::new(None),
    });

    let message_source_id = to_ui_rx.attach(Some(&default_context), {
        let wayland_display_ready = Arc::clone(&wayland_display_ready);
        let gtk_inited = Arc::clone(&gtk_inited);
        let state = Rc::clone(&state);

        move |message| {
            let wayland_display_ready = Arc::clone(&wayland_display_ready);
            handle_ui_message(message, Rc::clone(&state), wayland_display_ready, Arc::clone(&gtk_inited))
        }
    });

    while !wayland_display_ready.load(Ordering::SeqCst) {
        default_context.iteration(true);
    }

    gtk::gdk::set_allowed_backends("wayland");
    gtk::init()?;
    gtk_inited.store(true, Ordering::SeqCst);

    let settings_notifiers = gtk_settings::init_notifiers(Rc::clone(&state), from_ui_tx);

    let window_menu_anchor = window_menu::create_anchor_window();
    window_menu_anchor.show_all();
    state.window_menu_anchor.borrow_mut().replace(window_menu_anchor);

    let window = gtk::Window::builder()
        .title("Hello test!")
        .default_width(200)
        .default_height(200)
        .build();

    let label = gtk::Label::builder().label("This is a test!").build();
    window.add(&label);

    window.show_all();

    gtk::main();

    let settings = gtk::Settings::default().unwrap();
    for id in settings_notifiers {
        glib::signal_handler_disconnect(&settings, id);
    }
    message_source_id.remove();

    Ok(())
}

fn handle_ui_message(
    message: ToUiMessage,
    state: Rc<UiThreadState>,
    wayland_display_ready: Arc<AtomicBool>,
    gtk_inited: Arc<AtomicBool>,
) -> ControlFlow {
    match message {
        ToUiMessage::WaylandDisplayReady => {
            wayland_display_ready.store(true, Ordering::SeqCst);
            ControlFlow::Continue
        }

        ToUiMessage::ProvideIconSizes(icon_size_hints) => {
            let tabwin_sizes = tabwin::guess_icon_sizes(icon_size_hints.tabwin_mode, icon_size_hints.tabwin_cycle_preview);
            let _ = state.from_ui_tx.send(FromUiMessage::IconSizes(tabwin_sizes));
            ControlFlow::Continue
        }

        ToUiMessage::PrepareWindowMenu(ready_tx, window_menu_state) => {
            if let Some(parent) = state.window_menu_anchor.borrow().clone() {
                window_menu::create_menu(window_menu_state, &parent, &state.from_ui_tx);
                let _ = ready_tx.send(());
            }
            ControlFlow::Continue
        }

        ToUiMessage::ShowTabwin(config) => {
            let tabwin_showing = state.tabwin.borrow().is_some();
            if !tabwin_showing {
                let tabwin = tabwin::create(config, state.from_ui_tx.clone(), state.tabwin_style_provider.borrow().as_ref());
                tabwin.connect_destroy_event({
                    let state = Rc::clone(&state);
                    move |_, _| {
                        state.tabwin.replace(None);
                        glib::Propagation::Proceed
                    }
                });
                tabwin.show_all();

                *state.tabwin.borrow_mut() = Some(tabwin);
            } else {
                warn!("Tabwin already visible");
            }
            ControlFlow::Continue
        }

        ToUiMessage::TabwinNext => {
            if let Some(tabwin) = state.tabwin.borrow().as_ref()
                && let Some(selected) = tabwin.select_next()
            {
                let _ = state
                    .from_ui_tx
                    .send(FromUiMessage::TabwinAction(TabwinAction::HoverWindow(selected)));
            }
            ControlFlow::Continue
        }

        ToUiMessage::TabwinPrevious => {
            if let Some(tabwin) = state.tabwin.borrow().as_ref()
                && let Some(selected) = tabwin.select_previous()
            {
                let _ = state
                    .from_ui_tx
                    .send(FromUiMessage::TabwinAction(TabwinAction::HoverWindow(selected)));
            }
            ControlFlow::Continue
        }

        ToUiMessage::FinshTabwin => {
            if let Some(tabwin) = state.tabwin.take() {
                let selected = tabwin.selected();
                tabwin.close();

                if let Some(selected) = selected {
                    let _ = state
                        .from_ui_tx
                        .send(FromUiMessage::TabwinAction(TabwinAction::WindowSelected(selected)));
                }
            }
            ControlFlow::Continue
        }

        ToUiMessage::CancelTabwin => {
            if let Some(tabwin) = state.tabwin.take() {
                tabwin.close();
            }
            ControlFlow::Continue
        }

        ToUiMessage::TabwinWindowAdded(client) => {
            if let Some(tabwin) = state.tabwin.borrow().as_ref() {
                tabwin.append_client(client);
            }
            ControlFlow::Continue
        }

        ToUiMessage::TabwinWindowRemoved(id) => {
            if let Some(tabwin) = state.tabwin.borrow().as_ref() {
                tabwin.remove_client(id);
            }
            ControlFlow::Continue
        }

        ToUiMessage::Quit => {
            if gtk_inited.load(Ordering::SeqCst) {
                let level = gtk::main_level();
                for _ in 0..level {
                    gtk::main_quit();
                }
            }
            ControlFlow::Break
        }
    }
}
