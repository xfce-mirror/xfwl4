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

#[cfg(feature = "udev")]
use std::path::PathBuf;
use std::{fmt, time::Duration};

use anyhow::anyhow;
use clap::Parser;
use smithay::reexports::calloop::EventLoop;
use tracing::{error, info};
use xfwl4::{
    Xfwl4State,
    backend::{Backend, udev::UdevConfig, x11::X11Config},
};

#[cfg(feature = "profile-with-tracy-mem")]
#[global_allocator]
static GLOBAL: profiling::tracy_client::ProfiledAllocator<std::alloc::System> =
    profiling::tracy_client::ProfiledAllocator::new(std::alloc::System, 10);

#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
enum ChosenBackend {
    /// Autodetect the backend
    #[default]
    Auto,
    /// Run as a TTY udev client
    Tty,
    /// Run as an X11 or Wayland client using winit
    Winit,
    /// Run as an X11 client
    X11,
}

impl fmt::Display for ChosenBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Tty => f.write_str("tty"),
            Self::Winit => f.write_str("winit"),
            Self::X11 => f.write_str("x11"),
        }
    }
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    /// Which backend to use
    #[arg(long, value_enum, default_value_t)]
    backend: ChosenBackend,

    /// GPU DRM device path (backend=tty)
    #[cfg(feature = "udev")]
    #[arg(long, value_name = "PATH")]
    drm_device: Option<PathBuf>,

    /// Disable OpenGL ES instancing (allows drawing many objects with a single render call)
    /// (backend=tty)
    #[cfg(feature = "udev")]
    #[arg(long, default_value_t = false)]
    disable_gles_instancing: bool,

    /// Disable 10-bit color (backend=tty)
    #[cfg(feature = "udev")]
    #[arg(long, default_value_t = false)]
    disable_10bit_color: bool,

    /// Disable direct scanout (backend=tty)
    #[cfg(feature = "udev")]
    #[arg(long, default_value_t = false)]
    disable_direct_scanout: bool,

    /// Disable use of Vulkan (backend=x11)
    #[cfg(feature = "x11")]
    #[arg(long, default_value_t = false)]
    disable_vulkan: bool,

    /// UI scale factor for XWayland clients (can be fractional)
    #[cfg(feature = "xwayland")]
    #[arg(long, default_value_t = 1.0)]
    xwayland_scale: f64,
}

fn main() {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_env("XFWL4_LOG") {
        tracing_subscriber::fmt().compact().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().compact().init();
    }

    #[cfg(feature = "profile-with-tracy")]
    profiling::tracy_client::Client::start();

    profiling::register_thread!("Main Thread");

    #[cfg(feature = "profile-with-puffin")]
    let _server = puffin_http::Server::new(&format!("0.0.0.0:{}", puffin_http::DEFAULT_PORT)).unwrap();
    #[cfg(feature = "profile-with-puffin")]
    profiling::puffin::set_scopes_on(true);

    let mut cli = Cli::parse();

    if let ChosenBackend::Auto = cli.backend {
        cli.backend = if std::env::var("WAYLAND_DISPLAY").is_ok() || std::env::var("WAYLAND_SOCKET").is_ok() {
            if cfg!(feature = "winit") {
                ChosenBackend::Winit
            } else {
                error!("A Wayland session is already running, but the Winit backend is not enabled");
                std::process::exit(1);
            }
        } else if std::env::var("DISPLAY").is_ok() {
            if cfg!(feature = "x11") {
                ChosenBackend::X11
            } else if cfg!(feature = "winit") {
                ChosenBackend::Winit
            } else {
                error!("An X11 session is already running, but neither the Winit nor X11 backends are enabled");
                std::process::exit(1);
            }
        } else {
            ChosenBackend::Tty
        };
    }

    let xwayland_scale = if cfg!(feature = "xwayland") { cli.xwayland_scale } else { 1. };

    if let Err(err) = match cli.backend {
        #[cfg(feature = "winit")]
        ChosenBackend::Winit => {
            tracing::info!("Starting xfwl4 with winit backend");
            xfwl4::backend::winit::init().and_then(|(event_loop, state)| run(event_loop, state, xwayland_scale))
        }
        #[cfg(feature = "udev")]
        ChosenBackend::Tty => {
            tracing::info!("Starting xfwl4 on a tty using udev");
            xfwl4::backend::udev::init(cli.into()).and_then(|(event_loop, state)| run(event_loop, state, xwayland_scale))
        }
        #[cfg(feature = "x11")]
        ChosenBackend::X11 => {
            tracing::info!("Starting xfwl4 with x11 backend");
            xfwl4::backend::x11::init(cli.into()).and_then(|(event_loop, state)| run(event_loop, state, xwayland_scale))
        }
        _ => Err(anyhow!("Unknown or unsupported backend {} selected", cli.backend)),
    } {
        error!("Fatal error: {err}");
        std::process::exit(1);
    }
}

fn run<BackendData: Backend + 'static>(
    mut event_loop: EventLoop<'static, Xfwl4State<BackendData>>,
    mut state: Xfwl4State<BackendData>,
    #[allow(unused)] xwayland_scale: f64,
) -> anyhow::Result<()> {
    #[cfg(feature = "xwayland")]
    state.start_xwayland(xwayland_scale);

    info!("Initialization completed, starting the main loop.");

    event_loop.run(Some(Duration::from_millis(16)), &mut state, |state| {
        state.refresh_and_flush_clients()
    })?;

    Ok(())
}

impl From<Cli> for X11Config {
    fn from(value: Cli) -> Self {
        Self {
            disable_vulkan: value.disable_vulkan,
        }
    }
}

impl From<Cli> for UdevConfig {
    fn from(value: Cli) -> Self {
        Self {
            drm_device: value.drm_device,
            disable_gles_instancing: value.disable_gles_instancing,
            disable_10bit_color: value.disable_10bit_color,
            disable_direct_scanout: value.disable_direct_scanout,
        }
    }
}
