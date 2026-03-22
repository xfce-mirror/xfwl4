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

//! The UI thread supervision scheme
//!
//! 1.   As one of the first things it does, the main thread calls start().
//! 2.   start() creates two pipes and then forks, returning the write end of one pipe and the read
//!      end of another to the main thread.
//! 3.   The new child is the supervisor process.  It immediately sits and blocks on the read end
//!      one of the pipes it sent back to the main thread.
//! 4.   The main thread finishes all its initialization, and inserts the read end of the pipe it
//!      shares with the supervisor process into its main loop.  When it's ready to enter its main
//!      loop, it writes the value of WAYLAND_DISPLAY, followed by a NUL byte, to the write end of
//!      one of the pipes the supervisor process gave it.
//! 5.   The supervisor process, having been blocking on that pipe, reads the data the main thread
//!      sent it.  It then sets WAYLAND_DISPLAY in its environment.
//! 6.   The supervisor process creates another pipe, and forks again.
//! 7.   The grandchild is the UI process.  It takes the read end of the pipe the supervisor just
//!      created, and blocks on it.
//! 8.   The supervisor takes the UI process's PID, and writes it to the pipe it shares with the
//!      main thread. Then it blocks on the read end of the pipe it shares with the main thread.
//! 9.   The main thread's main loop will see the data on the pipe, read the PID, and store it
//!      somewhere.  It then writes a single NUL byte to the pipe it shares with the supervisor.
//! 10.  The supervisor reads that zero, and then writes a single NUL byte to the pipe it shares
//!      with the UI process.
//! 11.  The UI process sees the NUL byte, initializes itself, and enters its own main loop,
//!      connecting to the main thread over its shared private Wayland protocol.
//! 12.  At this point, we are fully initialized, and the compositor in the main process can accept
//!      regular Wayland client connections and instruct the UI process to do UI things for it.
//! 13.  The supervisor process now calls waitpid(), and blocks indefinitely.
//!      1. If the UI process exits normally (exit code 0), the supervisor will return from
//!         waitpid(), see this orderly exit, and do an orderly exit of its own, because that means
//!         the compositor has told the UI thread to quit, because it is itself shutting down.
//!      2. If the UI process exits poorly (exit code not-0, or a signal), it loops back to step 6.
//!
//! Whew, that was a lot.  The reasons for this are several:
//!
//! * The UI thread may crash.  UI toolkits sometimes do that, even if the code using it is
//!   perfect.  The code using it almost certainly isn't perfect, though, and it may crash or run
//!   into some sort of fatal error.
//! * So we need to be able to restart it.  Unfortunately, the main thread cannot restart it,
//!   because once the main thread is fully running, it has cluttered its address space with
//!   things a new child process would inherit.  More importantly, it will have spawned threads,
//!   and forking (without immediately exec'ing) in a multi-threaded environment is entirely
//!   unsafe and will often lead to deadlocks.
//! * But a process in the middle, that never spawns threads, and does nothing more than create
//!   pipes, read from and write to them, and call waitpid()... why, it can fork as many times as
//!   wants, whenever it wants.
//! * That process in the middle is also hopefully so dead-simple that it's near impossible that
//!   it could crash.  Hopefully.
//!
//! So there we have it.  Quite complicated, but it will have to do.
use std::os::fd::{AsRawFd, OwnedFd};

use anyhow::anyhow;
use smithay::reexports::rustix;
use wayland_client::protocol::wl_registry::WlRegistry;

use crate::{
    core::config::ShortcutKey,
    ui::{
        compositor_ui_protocol::{TabwinState, WindowMenuState, proto::xfwl4_ui_manager_v1::Xfwl4UiManagerV1},
        supervisor::run_supervisor,
        tabwin::{TABWIN_DEFAULT_CSS, TABWIN_WIDGET_NAME, Tabwin},
        wayland_client_gsource::WaylandClientSource,
    },
    util::io::close_all_fds,
};

mod compositor_ui_protocol;
mod gtk_settings;
mod gtk_settings_sync;
mod supervisor;
pub mod tabwin;
mod theme;
mod ui_main;
mod util;
mod wayland_client_gsource;
pub mod window_menu;

pub struct MainComms {
    pub to_supervisor: OwnedFd,
    pub from_supervisor: OwnedFd,
}

struct SupervisorComms {
    pub to_main: OwnedFd,
    pub from_main: OwnedFd,
}

#[derive(Debug, Default)]
pub struct UiProcessState {
    source: Option<WaylandClientSource>,
    registry: Option<WlRegistry>,
    pub ui_manager: Option<Xfwl4UiManagerV1>,

    pub tabwin_state: Option<TabwinState>,
    pub tabwin: Option<Tabwin>,
    pub tabwin_style_provider: Option<gtk::CssProvider>,

    pub window_menu_anchor: gtk::Window,
    pub window_menu_state: Option<WindowMenuState>,
    pub window_menu: Option<gtk::Menu>,
}

/// # Safety
///
/// This must be called before any threads have been started.
pub unsafe fn start() -> anyhow::Result<MainComms> {
    let (main_comms, supervisor_comms) = build_comms()?;

    tracing::trace!("Starting UI supervisor");
    // SAFETY: We're starting in a single-threaded process.
    match unsafe { libc::fork() } {
        -1 => Err(anyhow!("fork() for UI supervisor process failed")),

        0 => {
            drop(main_comms);

            let except = &[supervisor_comms.from_main.as_raw_fd(), supervisor_comms.to_main.as_raw_fd()];
            close_all_fds(except);

            if let Err(err) = run_supervisor(supervisor_comms) {
                tracing::error!("Failed to initialize UI supervisor process: {err}");
                do_exit(1);
            } else {
                do_exit(0);
            }
        }

        supervisor_pid => {
            tracing::info!("UI supervisor process started as PID {supervisor_pid}");
            Ok(main_comms)
        }
    }
}

fn build_comms() -> std::io::Result<(MainComms, SupervisorComms)> {
    // Pipe for supervisor -> main process comms.
    let (supervisor_from_main_rx, main_to_supervisor_tx) = rustix::pipe::pipe()?;
    // Pipe for main process -> supervisor comms.
    let (main_from_supervisor_rx, supervisor_to_main_tx) = rustix::pipe::pipe()?;

    let main = MainComms {
        to_supervisor: main_to_supervisor_tx,
        from_supervisor: main_from_supervisor_rx,
    };
    let supervisor = SupervisorComms {
        to_main: supervisor_to_main_tx,
        from_main: supervisor_from_main_rx,
    };

    Ok((main, supervisor))
}

// This is here because sub-processes that do not exec() will inherit some tracy stuff that it sets
// up before main() is called (sigh), so we need to use _exit(), which will not run atexit
// handlers.  If we don't, exit will hang as tracy tries to join a nonexistent thread.
fn do_exit(code: libc::c_int) -> ! {
    if cfg!(feature = "profile-with-tracy") {
        unsafe { libc::_exit(code) };
    } else {
        std::process::exit(code);
    }
}
