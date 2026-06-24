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

use std::{ffi::OsString, process::Child, time::Duration};

use anyhow::{Context, anyhow};
use gettextrs::{LocaleCategory, bind_textdomain_codeset, bindtextdomain, setlocale, textdomain};
use smithay::reexports::calloop::{
    EventLoop,
    timer::{TimeoutAction, Timer},
};
use tracing::{error, info};
use xfwl4::{
    backend::{Backend, BackendType},
    build_config::{BUILD_LOCALEDIR, GETTEXT_PACKAGE},
    core::state::Xfwl4State,
    ui::MainComms,
};

use crate::app::{
    cli::{self, ChosenBackend},
    env, session,
};

mod app;

#[cfg(feature = "profile-with-tracy-mem")]
#[global_allocator]
static GLOBAL: profiling::tracy_client::ProfiledAllocator<std::alloc::System> =
    profiling::tracy_client::ProfiledAllocator::new(std::alloc::System, 10);

struct InitData<'l, BackendData: Backend + 'static> {
    state: Xfwl4State<BackendData>,
    event_loop: EventLoop<'l, Xfwl4State<BackendData>>,
    main_comms: MainComms,
    session_command: Option<(OsString, Vec<OsString>)>,
    dbus_daemon_child: Option<Child>,
    #[cfg(feature = "udev")]
    notify_fd: Option<std::os::fd::RawFd>,
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
    let session_command = cli.session_command()?;

    // SAFETY: We are calling this from a (so far) single-threaded program.
    unsafe {
        env::init_environment()?;
    }

    // SAFETY: We are calling this from a (so far) single-threaded program.
    let dbus_daemon_child = unsafe { env::ensure_dbus_session_daemon() }?;

    #[cfg(feature = "udev")]
    // SAFETY: We are calling this from a (so far) single-threaded program.
    let notify_fd = unsafe { env::extract_notify_fd_from_env() }
        .inspect_err(|err| tracing::warn!("{err}"))
        .ok()
        .flatten();

    // SAFETY: We haven't spawned any threads yet.
    let main_comms = unsafe { xfwl4::ui::start() }?;

    #[cfg(feature = "profile-with-tracy")]
    profiling::tracy_client::Client::start();

    profiling::register_thread!("Main Thread");

    #[cfg(feature = "profile-with-puffin")]
    let _server = puffin_http::Server::new(&format!("0.0.0.0:{}", puffin_http::DEFAULT_PORT)).unwrap();
    #[cfg(feature = "profile-with-puffin")]
    profiling::puffin::set_scopes_on(true);

    xfconf::init().context("xfconf initialization failed")?;

    match cli.backend {
        ChosenBackend::Auto => unreachable!(),
        #[cfg(feature = "winit")]
        ChosenBackend::Winit => {
            tracing::info!("Starting xfwl4 with winit backend");
            let (event_loop, state) = xfwl4::backend::winit::init()?;
            let init_data = InitData {
                state,
                event_loop,
                main_comms,
                session_command,
                dbus_daemon_child,
                #[cfg(feature = "udev")]
                notify_fd,
            };
            run_main_loop(init_data)?;
        }
        #[cfg(feature = "udev")]
        ChosenBackend::Tty => {
            tracing::info!("Starting xfwl4 on a tty using udev");
            let (event_loop, state) = xfwl4::backend::udev::init(cli.into())?;
            let init_data = InitData {
                state,
                event_loop,
                main_comms,
                session_command,
                dbus_daemon_child,
                #[cfg(feature = "udev")]
                notify_fd,
            };
            run_main_loop(init_data)?;
        }
        #[cfg(feature = "x11")]
        ChosenBackend::X11 => {
            tracing::info!("Starting xfwl4 with x11 backend");
            let (event_loop, state) = xfwl4::backend::x11::init(cli.into())?;
            let init_data = InitData {
                state,
                event_loop,
                main_comms,
                session_command,
                dbus_daemon_child,
                #[cfg(feature = "udev")]
                notify_fd,
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
        main_comms,
        session_command,
        dbus_daemon_child,
        #[cfg(feature = "udev")]
        notify_fd,
    } = init_data;

    state.initialize_outputs();
    state.load_decoration_theme()?;

    if let Some(socket_name) = state.socket_name() {
        // SAFETY: This may not be safe, as other threads have been started, and we can't be sure
        // what they are doing.
        unsafe { std::env::set_var("WAYLAND_DISPLAY", socket_name) };
    }

    event_loop
        .handle()
        .insert_source(Timer::immediate(), move |_, _, _| {
            let main_context = glib::MainContext::default();
            while main_context.pending() {
                main_context.iteration(false);
            }
            TimeoutAction::ToDuration(Duration::from_millis(10))
        })
        .map_err(|err| anyhow!("Unable to register GMainContext source with event loop: {err}"))?;

    #[cfg(feature = "xwayland")]
    {
        match state.start_xwayland(None) {
            Ok(display_number) => {
                // SAFETY: This may not be safe, as other threads have been started, and we can't be sure
                // what they are doing.
                unsafe {
                    std::env::set_var("DISPLAY", format!(":{display_number}"));
                }
            }
            Err(err) => tracing::warn!("Xwayland failed to start; X11 clients will not work: {err}"),
        }
    }

    state.register_ui_comms(main_comms);

    let session = if let Some((command, args)) = session_command {
        env::import_environment();

        let (session, notifier) = session::Session::start(command, args)?;
        event_loop
            .handle()
            .insert_source(notifier, |event, _, state| {
                use smithay::reexports::calloop::channel::Event;
                if let Event::Msg(_) = event {
                    state.shutdown();
                }
            })
            .map_err(|err| anyhow!("Unable to register session app notifier with event loop: {err}"))?;

        Some(session)
    } else {
        None
    };

    #[cfg(feature = "udev")]
    if state.backend_type() == BackendType::Tty
        && let Some(notify_fd) = notify_fd
    {
        // SAFETY: This may not be safe, as we have to trust the parent process that the FD is
        // valid and open.
        unsafe {
            env::notify_fd(notify_fd);
        }
    }

    info!("Initialization completed, starting the main loop.");

    event_loop.run(Some(Duration::from_millis(16)), &mut state, |state| {
        state.refresh_and_flush_clients()
    })?;

    if let Some(session) = session {
        session.kill();
    }

    if let Some(mut dbus_daemon_child) = dbus_daemon_child {
        let _ = dbus_daemon_child.kill();
    }

    Ok(())
}
