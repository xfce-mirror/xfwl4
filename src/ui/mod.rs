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
    ffi::OsStr,
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    rc::Rc,
};

use anyhow::anyhow;
use gtk::traits::WidgetExt;
use smithay::reexports::rustix::{self, process::Pid};
use wayland_client::protocol::wl_registry::WlRegistry;

use crate::{
    core::config::ShortcutKey,
    ui::{
        compositor_ui_protocol::{TabwinState, WindowMenuState, proto::xfwl4_ui_manager_v1::Xfwl4UiManagerV1},
        gtk_settings_sync::GtkSettingsSync,
        tabwin::{TABWIN_DEFAULT_CSS, TABWIN_WIDGET_NAME, Tabwin},
        wayland_client_gsource::WaylandClientSource,
    },
};

mod compositor_ui_protocol;
mod gtk_settings;
mod gtk_settings_sync;
pub mod tabwin;
mod theme;
mod util;
mod wayland_client_gsource;
pub mod window_menu;

#[derive(Debug, Default)]
struct UiProcessState {
    source: Option<WaylandClientSource>,
    registry: Option<WlRegistry>,
    ui_manager: Option<Xfwl4UiManagerV1>,

    tabwin_state: Option<TabwinState>,
    tabwin: Option<Tabwin>,
    tabwin_style_provider: Option<gtk::CssProvider>,

    window_menu_anchor: gtk::Window,
    window_menu_state: Option<WindowMenuState>,
    window_menu: Option<gtk::Menu>,
}

/// # Safety
///
/// Must be started before the app spawns any other threads.
pub unsafe fn start_ui_process() -> anyhow::Result<(Pid, OwnedFd)> {
    let (rx, tx) = rustix::pipe::pipe()?;
    tracing::debug!("pipe created: rx={}, tx={}", rx.as_raw_fd(), tx.as_raw_fd());

    // SAFETY: We're starting in a single-threaded program.
    match unsafe { libc::fork() } {
        -1 => Err(anyhow!("fork() for UI process failed")),
        0 => {
            drop(tx);

            let keep = [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO, rx.as_raw_fd()];
            for fd in 3..1024 {
                if !keep.contains(&fd) {
                    // SAFETY: All our FDs are > -1, and so are valid, even if they aren't open.
                    unsafe { libc::close(fd) };
                }
            }

            if let Err(err) = ui_main(rx) {
                tracing::error!("Failed to initialize UI process: {err}");
                if cfg!(feature = "profile-with-tracy") {
                    unsafe { libc::_exit(1) };
                } else {
                    std::process::exit(1);
                }
            } else {
                tracing::debug!("UI thread quitting");
                if cfg!(feature = "profile-with-tracy") {
                    unsafe { libc::_exit(0) };
                } else {
                    std::process::exit(0);
                }
            }
        }
        pid => Pid::from_raw(pid)
            .map(|pid| (pid, tx))
            .ok_or_else(|| anyhow!("UI child PID is somehow invalid")),
    }
}

fn ui_main(rx: OwnedFd) -> anyhow::Result<()> {
    // Wait until the main process sends the socket name followed by a nul byte.
    let mut buf = [0u8; 256];
    let mut total_read = 0;
    loop {
        let read = smithay::reexports::rustix::io::read(&rx, &mut buf[total_read..])?;
        total_read += read;
        if read == 0 || buf[..total_read].ends_with(b"\0") {
            break;
        }
    }
    drop(rx);

    if total_read == 0 || !buf[..total_read].ends_with(b"\0") {
        return Err(anyhow!("Got bad socket name from main process"));
    }

    let socket_name = OsStr::from_bytes(&buf[..(total_read - 1)]);
    tracing::debug!("Main process is ready on display {socket_name:?}");

    // SAFETY: No other threads have started yet.
    unsafe { std::env::set_var("WAYLAND_DISPLAY", socket_name) };

    gtk::gdk::set_allowed_backends("wayland");
    gtk::init()?;

    xfconf::init()?;

    let window_menu_anchor = window_menu::create_anchor_window();
    window_menu_anchor.show_all();

    let state = UiProcessState {
        source: None,
        registry: None,
        ui_manager: None,
        tabwin_state: None,
        tabwin: None,
        tabwin_style_provider: None,
        window_menu_anchor,
        window_menu_state: None,
        window_menu: None,
    };

    let display_name = gtk::gdk::Display::default().unwrap().name();
    let state = compositor_ui_protocol::connect(&display_name, state)?;

    let _settings_sync = GtkSettingsSync::new();
    let settings_notifiers = gtk_settings::init_notifiers(Rc::clone(&state));

    gtk::main();

    if let Some(source) = state.borrow_mut().source.take() {
        source.destroy();
    }

    let settings = gtk::Settings::default().unwrap();
    for id in settings_notifiers {
        glib::signal_handler_disconnect(&settings, id);
    }

    Ok(())
}
