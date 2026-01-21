use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

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

#[derive(Debug)]
pub enum ToUiMessage {
    WaylandDisplayReady,
    ShowWindowMenu(WindowMenuState),
    ShowTabwin(Vec<TabwinWindow>),
    Quit,
}

#[derive(Debug)]
pub enum FromUiMessage {
    DefaultMainContextClaimed,
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
    // Claim the global default main context so it gets created and no other thread creates it.
    let default_context = glib::MainContext::default();

    from_ui_tx.send(FromUiMessage::DefaultMainContextClaimed).unwrap();

    let wayland_display_ready = Arc::new(AtomicBool::new(false));
    let gtk_inited = Arc::new(AtomicBool::new(false));

    let message_source_id = to_ui_rx.attach(Some(&default_context), {
        let wayland_display_ready = Arc::clone(&wayland_display_ready);
        let gtk_inited = Arc::clone(&gtk_inited);

        move |message| {
            let wayland_display_ready = Arc::clone(&wayland_display_ready);
            handle_ui_message(message, from_ui_tx.clone(), wayland_display_ready, Arc::clone(&gtk_inited))
        }
    });

    while !wayland_display_ready.load(Ordering::SeqCst) {
        default_context.iteration(true);
    }

    gtk::gdk::set_allowed_backends("wayland");
    gtk::init()?;
    gtk_inited.store(true, Ordering::SeqCst);

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

fn handle_ui_message(
    message: ToUiMessage,
    _from_ui_tx: channel::Sender<FromUiMessage>,
    wayland_display_ready: Arc<AtomicBool>,
    gtk_inited: Arc<AtomicBool>,
) -> ControlFlow {
    match message {
        ToUiMessage::WaylandDisplayReady => {
            wayland_display_ready.store(true, Ordering::SeqCst);
            ControlFlow::Continue
        }
        ToUiMessage::ShowWindowMenu(state) => {
            window_menu::pop_up(state);
            ControlFlow::Continue
        }
        ToUiMessage::ShowTabwin(_windows) => ControlFlow::Continue,
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
