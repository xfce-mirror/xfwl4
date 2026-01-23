use std::fmt;
#[cfg(feature = "udev")]
use std::path::PathBuf;

use anyhow::anyhow;
use clap::Parser;
use xfwl4::backend::{udev::UdevConfig, x11::X11Config};

#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
pub enum ChosenBackend {
    /// Autodetect the backend
    #[default]
    Auto,
    /// Run as a TTY udev client
    #[cfg(feature = "udev")]
    Tty,
    /// Run as an X11 or Wayland client using winit
    #[cfg(feature = "winit")]
    Winit,
    /// Run as an X11 client
    #[cfg(feature = "x11")]
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
    pub backend: ChosenBackend,

    /// GPU DRM device path (backend=tty)
    #[cfg(feature = "udev")]
    #[arg(long, value_name = "PATH")]
    pub drm_device: Option<PathBuf>,

    /// Disable OpenGL ES instancing (allows drawing many objects with a single render call)
    /// (backend=tty)
    #[cfg(feature = "udev")]
    #[arg(long, default_value_t = false)]
    pub disable_gles_instancing: bool,

    /// Disable 10-bit color (backend=tty)
    #[cfg(feature = "udev")]
    #[arg(long, default_value_t = false)]
    pub disable_10bit_color: bool,

    /// Disable direct scanout (backend=tty)
    #[cfg(feature = "udev")]
    #[arg(long, default_value_t = false)]
    pub disable_direct_scanout: bool,

    /// Disable use of Vulkan (backend=x11)
    #[cfg(feature = "x11")]
    #[arg(long, default_value_t = false)]
    pub disable_vulkan: bool,

    /// UI scale factor for XWayland clients (can be fractional)
    #[cfg(feature = "xwayland")]
    #[arg(long, default_value_t = 1.0)]
    pub xwayland_scale: f64,
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

pub fn parse() -> anyhow::Result<Cli> {
    let mut cli = Cli::parse();

    cli.backend = if let ChosenBackend::Auto = cli.backend {
        if std::env::var("WAYLAND_DISPLAY").is_ok() || std::env::var("WAYLAND_SOCKET").is_ok() {
            if cfg!(feature = "winit") {
                Ok(ChosenBackend::Winit)
            } else {
                Err(anyhow!(
                    "A Wayland session is already running, but the Winit backend is not enabled"
                ))
            }
        } else if std::env::var("DISPLAY").is_ok() {
            if cfg!(feature = "x11") {
                Ok(ChosenBackend::X11)
            } else if cfg!(feature = "winit") {
                Ok(ChosenBackend::Winit)
            } else {
                Err(anyhow!(
                    "An X11 session is already running, but neither the Winit nor X11 backends are enabled"
                ))
            }
        } else if cfg!(feature = "udev") {
            Ok(ChosenBackend::Tty)
        } else {
            Err(anyhow!("No suitable backend is availble"))
        }
    } else {
        Ok(cli.backend)
    }?;

    Ok(cli)
}
