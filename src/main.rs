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

use std::{os::fd::OwnedFd, time::Duration};

use anyhow::Context;
use gettextrs::{LocaleCategory, bind_textdomain_codeset, bindtextdomain, setlocale, textdomain};
use smithay::reexports::calloop::EventLoop;
use tracing::{error, info};
use xfwl4::{
    backend::{Backend, BackendType},
    build_config::{BUILD_LOCALEDIR, GETTEXT_PACKAGE},
    core::state::Xfwl4State,
};

use crate::app::{
    cli::{self, ChosenBackend},
    env,
};

mod app;

#[cfg(feature = "profile-with-tracy-mem")]
#[global_allocator]
static GLOBAL: profiling::tracy_client::ProfiledAllocator<std::alloc::System> =
    profiling::tracy_client::ProfiledAllocator::new(std::alloc::System, 10);

struct InitData<'l, BackendData: Backend + 'static> {
    state: Xfwl4State<BackendData>,
    event_loop: EventLoop<'l, Xfwl4State<BackendData>>,
    ui_process_notifier: OwnedFd,
    start_session: bool,
    #[cfg(feature = "udev")]
    notify_fd: Option<std::os::fd::RawFd>,
    #[cfg(feature = "xwayland")]
    xwayland_scale: f64,
}

fn main() {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_env("XFWL4_LOG") {
        tracing_subscriber::fmt().compact().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().compact().init();
    }

    if let Err(err) = run() {
        error!("{}", err);
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    setlocale(LocaleCategory::LcAll, "");
    bindtextdomain(GETTEXT_PACKAGE, BUILD_LOCALEDIR)?;
    bind_textdomain_codeset(GETTEXT_PACKAGE, "UTF-8")?;
    textdomain(GETTEXT_PACKAGE)?;

    let cli = cli::parse()?;

    // SAFETY: We are calling this from a (so far) single-threaded program.
    unsafe {
        env::init_environment()?;
    }

    #[cfg(feature = "udev")]
    // SAFETY: We are calling this from a (so far) single-threaded program.
    let notify_fd = unsafe { env::extract_notify_fd_from_env() }
        .inspect_err(|err| tracing::warn!("{err}"))
        .ok()
        .flatten();

    // SAFETY: We haven't spawned any threads yet.
    let (ui_process_pid, ui_process_notifier) = unsafe { xfwl4::ui::start_ui_process() }?;

    #[cfg(feature = "profile-with-tracy")]
    profiling::tracy_client::Client::start();

    profiling::register_thread!("Main Thread");

    #[cfg(feature = "profile-with-puffin")]
    let _server = puffin_http::Server::new(&format!("0.0.0.0:{}", puffin_http::DEFAULT_PORT)).unwrap();
    #[cfg(feature = "profile-with-puffin")]
    profiling::puffin::set_scopes_on(true);

    xfconf::init().context("xfconf initialization failed")?;

    let start_session = !cli.no_session;
    #[cfg(feature = "xwayland")]
    let xwayland_scale = cli.xwayland_scale;

    match cli.backend {
        ChosenBackend::Auto => unreachable!(),
        #[cfg(feature = "winit")]
        ChosenBackend::Winit => {
            tracing::info!("Starting xfwl4 with winit backend");
            let (event_loop, state) = xfwl4::backend::winit::init(ui_process_pid)?;
            let init_data = InitData {
                state,
                event_loop,
                ui_process_notifier,
                start_session,
                #[cfg(feature = "udev")]
                notify_fd,
                #[cfg(feature = "xwayland")]
                xwayland_scale,
            };
            run_main_loop(init_data)?;
        }
        #[cfg(feature = "udev")]
        ChosenBackend::Tty => {
            tracing::info!("Starting xfwl4 on a tty using udev");
            let (event_loop, state) = xfwl4::backend::udev::init(cli.into(), ui_process_pid)?;
            let init_data = InitData {
                state,
                event_loop,
                ui_process_notifier,
                start_session,
                #[cfg(feature = "udev")]
                notify_fd,
                #[cfg(feature = "xwayland")]
                xwayland_scale,
            };
            run_main_loop(init_data)?;
        }
        #[cfg(feature = "x11")]
        ChosenBackend::X11 => {
            tracing::info!("Starting xfwl4 with x11 backend");
            let (event_loop, state) = xfwl4::backend::x11::init(cli.into(), ui_process_pid)?;
            let init_data = InitData {
                state,
                event_loop,
                ui_process_notifier,
                start_session,
                #[cfg(feature = "udev")]
                notify_fd,
                #[cfg(feature = "xwayland")]
                xwayland_scale,
            };
            run_main_loop(init_data)?;
        }
    }

    // Annoyingly gtk_main() blocks in gdk_flush() before returning, but it will never make
    // progress because we aren't handling data on the Wayland socket anymore.
    //if let Err(err) = ui_thread_handle.join() {
    //    warn!("Failed to join UI thread: {err:?}");
    //}

    Ok(())
}

fn run_main_loop<BackendData: Backend + 'static>(init_data: InitData<'_, BackendData>) -> anyhow::Result<()> {
    let InitData {
        mut state,
        mut event_loop,
        ui_process_notifier,
        start_session,
        #[cfg(feature = "udev")]
        notify_fd,
        #[cfg(feature = "xwayland")]
        xwayland_scale,
    } = init_data;

    state.initialize_outputs();

    state.load_decoration_theme()?;

    if let Some(socket_name) = state.socket_name() {
        // SAFETY: This may not be safe, as other threads have been started, and we can't be sure
        // what they are doing.
        unsafe { std::env::set_var("WAYLAND_DISPLAY", socket_name) };
    }

    #[cfg(feature = "xwayland")]
    {
        let display_number = state.start_xwayland(xwayland_scale)?;
        // SAFETY: This may not be safe, as other threads have been started, and we can't be sure
        // what they are doing.
        unsafe {
            std::env::set_var("DISPLAY", format!(":{display_number}"));
        }
    }

    #[cfg(feature = "udev")]
    let xfce4_session = if state.backend_type() == BackendType::Tty {
        let xfce4_session = if start_session {
            use std::process::Command;

            env::import_environment();

            match Command::new("xfce4-session").spawn() {
                Err(err) => {
                    tracing::error!("Failed to start xfce4-session: {err}");
                    None
                }
                Ok(child) => Some(child),
            }
        } else {
            None
        };

        if let Some(notify_fd) = notify_fd {
            // SAFETY: This may not be safe, as we have to trust the parent process that the FD is
            // valid and open.
            unsafe {
                env::notify_fd(notify_fd);
            }
        }

        xfce4_session
    } else {
        None
    };

    if let Some(socket_name) = state.socket_name() {
        let mut name_buf = socket_name.as_bytes().to_vec();
        name_buf.push(b'\0');
        if let Err(err) = smithay::reexports::rustix::io::write(&ui_process_notifier, &name_buf) {
            tracing::error!("Failed to notify UI process: {err}");
        }
    }
    drop(ui_process_notifier);

    info!("Initialization completed, starting the main loop.");

    event_loop.run(Some(Duration::from_millis(16)), &mut state, |state| {
        state.refresh_and_flush_clients()
    })?;

    #[cfg(feature = "udev")]
    if let Some(xfce4_session) = xfce4_session {
        use smithay::reexports::rustix::process::{Pid, Signal, kill_process};
        let _ = kill_process(Pid::from_child(&xfce4_session), Signal::TERM);
    }

    Ok(())
}
