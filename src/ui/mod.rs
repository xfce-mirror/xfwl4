use std::thread::{self, JoinHandle};

use glib::{ControlFlow, Receiver};
use gtk::traits::{ContainerExt, WidgetExt};
use smithay::reexports::calloop::channel;
use tracing::error;

use crate::ui::{
    tabwin::{TabwinAction, TabwinWindow},
    window_menu::{WindowMenuAction, WindowMenuState},
};

pub mod tabwin;
pub mod window_menu;

pub enum ToUiMessage {
    ShowWindowMenu(WindowMenuState),
    ShowTabwin(Vec<TabwinWindow>),
    Quit,
}

pub enum FromUiMessage {
    WindowMenuAction(WindowMenuAction),
    TabwinAction(TabwinAction),
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
    gtk::gdk::set_allowed_backends("wayland");
    gtk::init()?;

    let message_source_id = to_ui_rx.attach(None, move |message| handle_ui_message(message, from_ui_tx.clone()));

    let window = gtk::Window::builder()
        .title("Hello test!")
        .default_width(200)
        .default_height(200)
        .build();

    let label = gtk::Label::builder().label("This is a test!").build();
    window.add(&label);

    window.show_all();

    gtk::main();

    message_source_id.remove();

    Ok(())
}

fn handle_ui_message(message: ToUiMessage, _from_ui_tx: channel::Sender<FromUiMessage>) -> ControlFlow {
    match message {
        ToUiMessage::ShowWindowMenu(state) => {
            window_menu::pop_up(state);
            ControlFlow::Continue
        }
        ToUiMessage::ShowTabwin(_windows) => ControlFlow::Continue,
        ToUiMessage::Quit => {
            let level = gtk::main_level();
            for _ in 0..level {
                gtk::main_quit();
            }
            ControlFlow::Break
        }
    }
}
