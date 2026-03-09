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

use std::time::Duration;

use anyhow::{Context, anyhow};
use gettextrs::{LocaleCategory, bind_textdomain_codeset, bindtextdomain, setlocale, textdomain};
use glib::translate::ToGlibPtr;
use smithay::reexports::calloop::{
    EventLoop, channel,
    timer::{TimeoutAction, Timer},
};
use tracing::{error, info};
use xfwl4::{
    backend::{Backend, BackendType},
    build_config::{BUILD_LOCALEDIR, GETTEXT_PACKAGE},
    core::state::Xfwl4State,
    ui::{FromUiMessage, IconSizeHints, ToUiMessage},
};

use crate::app::{
    cli::{self, ChosenBackend},
    env, gtk_settings_dbus,
    zbus_ext::ZBusAdapter,
};

mod app;

#[cfg(feature = "profile-with-tracy-mem")]
#[global_allocator]
static GLOBAL: profiling::tracy_client::ProfiledAllocator<std::alloc::System> =
    profiling::tracy_client::ProfiledAllocator::new(std::alloc::System, 10);

struct InitData<'l, BackendData: Backend + 'static> {
    state: Xfwl4State<BackendData>,
    event_loop: EventLoop<'l, Xfwl4State<BackendData>>,
    thread_context: glib::MainContext,
    export_systemd_dbus_vars: bool,
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

    #[cfg(feature = "profile-with-tracy")]
    profiling::tracy_client::Client::start();

    profiling::register_thread!("Main Thread");

    #[cfg(feature = "profile-with-puffin")]
    let _server = puffin_http::Server::new(&format!("0.0.0.0:{}", puffin_http::DEFAULT_PORT)).unwrap();
    #[cfg(feature = "profile-with-puffin")]
    profiling::puffin::set_scopes_on(true);

    // First spawn the UI thread so it can create and aquire the default GMainContext.  GTK assumes
    // it can own the default one, so we have to create our own, but we want to make sure GTK gets
    // it first.
    #[allow(deprecated)]
    let (to_ui_tx, to_ui_rx) = glib::MainContext::channel(glib::Priority::DEFAULT);
    let (from_ui_tx, from_ui_rx) = channel::channel();
    let _ui_thread_handle = xfwl4::ui::launch_ui_thread(to_ui_rx, from_ui_tx);

    match from_ui_rx.recv().context("Failed to receive from UI thread")? {
        FromUiMessage::DefaultMainContextClaimed => (),
        message => return Err(anyhow!("Got incorrect message from UI thread: {message:?}")),
    }

    // Now we can create our own main context.  By creating one here, acquiring it, and pushing it
    // as thread-default, GBus and other stuff should (hopefully!) use this one rather than the one
    // on the GTK UI thread.
    let thread_context = glib::MainContext::new();
    let _acquired = thread_context
        .acquire()
        .context("Newly-created GMainContext acquire should not fail")?;
    // SAFETY: this succeeds with a non-NULL pointer as long as the context has been acquired.
    unsafe {
        glib::ffi::g_main_context_push_thread_default(thread_context.to_glib_none().0);
    }

    xfconf::init().context("xfconf initialization failed")?;

    let export_systemd_dbus_vars = !cli.no_session;
    #[cfg(feature = "xwayland")]
    let xwayland_scale = cli.xwayland_scale;

    match cli.backend {
        ChosenBackend::Auto => unreachable!(),
        #[cfg(feature = "winit")]
        ChosenBackend::Winit => {
            tracing::info!("Starting xfwl4 with winit backend");
            let (event_loop, state) = xfwl4::backend::winit::init(from_ui_rx, to_ui_tx)?;
            let init_data = InitData {
                state,
                event_loop,
                thread_context: thread_context.clone(),
                export_systemd_dbus_vars,
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
            let (event_loop, state) = xfwl4::backend::udev::init(cli.into(), from_ui_rx, to_ui_tx)?;
            let init_data = InitData {
                state,
                event_loop,
                thread_context: thread_context.clone(),
                export_systemd_dbus_vars,
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
            let (event_loop, state) = xfwl4::backend::x11::init(cli.into(), from_ui_rx, to_ui_tx)?;
            let init_data = InitData {
                state,
                event_loop,
                thread_context: thread_context.clone(),
                export_systemd_dbus_vars,
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
        thread_context,
        export_systemd_dbus_vars,
        #[cfg(feature = "udev")]
        notify_fd,
        #[cfg(feature = "xwayland")]
        xwayland_scale,
    } = init_data;

    state.load_decoration_theme()?;

    if let Some(socket_name) = state.socket_name() {
        // SAFETY: This may not be safe, as other threads have been started, and we can't be sure
        // what they are doing.
        unsafe { std::env::set_var("WAYLAND_DISPLAY", socket_name) };
    }

    event_loop
        .handle()
        .insert_source(Timer::immediate(), move |_, _, _| {
            while thread_context.pending() {
                thread_context.iteration(false);
            }
            TimeoutAction::ToDuration(Duration::from_millis(10))
        })
        .map_err(|err| anyhow!("Unable to register GMainContext source with event loop: {err}"))?;

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
    if state.backend_type() == BackendType::Tty {
        if export_systemd_dbus_vars {
            env::import_environment();
        }
        if let Some(notify_fd) = notify_fd {
            // SAFETY: This may not be safe, as we have to trust the parent process that the FD is
            // valid and open.
            unsafe {
                env::notify_fd(notify_fd);
            }
        }
    }

    let zbus_adapter = match ZBusAdapter::init(event_loop.handle()) {
        Err(err) => {
            tracing::error!("Failed to set up zbus: {err}");
            None
        }
        Ok(adapter) => {
            gtk_settings_dbus::start(&adapter);
            Some(adapter)
        }
    };

    state.send_to_ui(ToUiMessage::WaylandDisplayReady);

    event_loop.handle().insert_idle(|state| {
        state.send_to_ui(ToUiMessage::ProvideIconSizes(IconSizeHints {
            tabwin_mode: state.cycle_tabwin_mode(),
            tabwin_cycle_preview: state.cycle_preview(),
        }));
    });

    info!("Initialization completed, starting the main loop.");

    event_loop.run(Some(Duration::from_millis(16)), &mut state, |state| {
        state.refresh_and_flush_clients()
    })?;

    state.send_to_ui(ToUiMessage::Quit);
    if let Some(zbus_adapter) = zbus_adapter {
        zbus_adapter.shutdown();
    }

    Ok(())
}
