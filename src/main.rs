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
//
// Portions of this file are based on "anvil", an example compositor
// based on the smithay crate, and are licensed under the MIT license
// with the following terms:
//
// Copyright (C) Victor Berger <victor.berger@m4x.org>
// Copyright (C) Drakulix (Victoria Brekenfeld)
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

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

use crate::cli::ChosenBackend;

mod cli;
mod env;

#[cfg(feature = "profile-with-tracy-mem")]
#[global_allocator]
static GLOBAL: profiling::tracy_client::ProfiledAllocator<std::alloc::System> =
    profiling::tracy_client::ProfiledAllocator::new(std::alloc::System, 10);

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
    let xwayland_scale = if cfg!(feature = "xwayland") { cli.xwayland_scale } else { 1. };

    match cli.backend {
        ChosenBackend::Auto => unreachable!(),
        #[cfg(feature = "winit")]
        ChosenBackend::Winit => {
            tracing::info!("Starting xfwl4 with winit backend");
            let (event_loop, state) = xfwl4::backend::winit::init(from_ui_rx, to_ui_tx)?;
            run_main_loop(event_loop, state, thread_context.clone(), export_systemd_dbus_vars, xwayland_scale)?;
        }
        #[cfg(feature = "udev")]
        ChosenBackend::Tty => {
            tracing::info!("Starting xfwl4 on a tty using udev");
            let (event_loop, state) = xfwl4::backend::udev::init(cli.into(), from_ui_rx, to_ui_tx)?;
            run_main_loop(event_loop, state, thread_context.clone(), export_systemd_dbus_vars, xwayland_scale)?;
        }
        #[cfg(feature = "x11")]
        ChosenBackend::X11 => {
            tracing::info!("Starting xfwl4 with x11 backend");
            let (event_loop, state) = xfwl4::backend::x11::init(cli.into(), from_ui_rx, to_ui_tx)?;
            run_main_loop(event_loop, state, thread_context.clone(), export_systemd_dbus_vars, xwayland_scale)?;
        }
    }

    // Annoyingly gtk_main() blocks in gdk_flush() before returning, but it will never make
    // progress because we aren't handling data on the Wayland socket anymore.
    //if let Err(err) = ui_thread_handle.join() {
    //    warn!("Failed to join UI thread: {err:?}");
    //}

    Ok(())
}

fn run_main_loop<BackendData: Backend + 'static>(
    mut event_loop: EventLoop<'static, Xfwl4State<BackendData>>,
    mut state: Xfwl4State<BackendData>,
    thread_context: glib::MainContext,
    export_systemd_dbus_vars: bool,
    #[allow(unused)] xwayland_scale: f64,
) -> anyhow::Result<()> {
    state.load_decoration_theme()?;

    if let Some(socket_name) = &state.socket_name {
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
    if state.backend_data.backend_type() == BackendType::Tty {
        if export_systemd_dbus_vars {
            env::import_environment();
        }
        // SAFETY: This may not be safe.
        unsafe {
            env::notify_fd();
        }
    }

    state.to_ui_channel_tx.send(ToUiMessage::WaylandDisplayReady)?;

    event_loop.handle().insert_idle(|state| {
        let _ = state.to_ui_channel_tx.send(ToUiMessage::ProvideIconSizes(IconSizeHints {
            tabwin_mode: state.config.cycle_tabwin_mode(),
            tabwin_cycle_preview: state.config.cycle_preview(),
        }));
    });

    info!("Initialization completed, starting the main loop.");

    event_loop.run(Some(Duration::from_millis(16)), &mut state, |state| {
        state.refresh_and_flush_clients()
    })?;

    state.to_ui_channel_tx.send(ToUiMessage::Quit)?;

    Ok(())
}
