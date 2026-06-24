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
    env::{self, VarError},
    ffi::{OsStr, OsString},
    io::ErrorKind,
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Child, Command},
    thread::sleep,
    time::{Duration, Instant},
};

use anyhow::{Context, anyhow};
use rand::distr::{Alphanumeric, SampleString};
use tracing::warn;

use crate::app::mini_dbus::session_bus_running;

/// Initializes some environment variables
///
/// # Safety
///
/// This is only safe if called from a single-threaded environment, or if you can somehow guarantee
/// that no other thread is reading, setting, or removing environment variables.
pub unsafe fn init_environment() -> anyhow::Result<()> {
    // SAFETY: This is safe if the function's safety constraints are met.
    unsafe {
        env::set_var("DESKTOP_SESSION", "xfce");
        env::set_var("XDG_CURRENT_DESKTOP", "XFCE");
        env::set_var("XDG_MENU_PREFIX", "xfce-");
        env::set_var("XDG_SESSION_TYPE", "wayland");
    }

    let home = env::home_dir().context("Unable to determine home directory")?;

    if let Err(VarError::NotPresent) = env::var("XDG_CONFIG_HOME") {
        let mut config_home = home.clone();
        config_home.push(".config");
        // SAFETY: This is safe if the function's safety constraints are met.
        unsafe {
            env::set_var("XDG_CONFIG_HOME", config_home);
        }
    }

    if let Err(VarError::NotPresent) = env::var("XDG_CACHE_HOME") {
        let mut cache_home = home.clone();
        cache_home.push(".cache");
        // SAFETY: This is safe if the function's safety constraints are met.
        unsafe {
            env::set_var("XDG_CACHE_HOME", cache_home);
        }
    }

    if let Err(VarError::NotPresent) = env::var("XDG_RUNTIME_DIR") {
        let mut runtime_dir = PathBuf::from("/run/user");
        runtime_dir.push(rustix::process::getuid().as_raw().to_string());

        if !runtime_dir.exists() {
            runtime_dir = glib::user_runtime_dir();
        }

        // SAFETY: This is safe if the function's safety constraints are met.
        unsafe {
            env::set_var("XDG_RUNTIME_DIR", runtime_dir);
        }
    }

    Ok(())
}

/// Checks if a D-Bus session daemon is running, and starts one if not
///
/// This deliberately probes (and waits on) the bus socket directly rather than going through GLib,
/// because `gio::bus_get_sync()` caches a `GDBusConnection` and spawns a worker thread. We must run
/// before [`crate::ui::start()`] forks the UI supervisor, and that supervisor can only safely fork
/// the UI process if our address space has never started a thread.
///
/// # Safety
///
/// This is only safe if called from a single-threaded environment, or if you can somehow guarantee
/// that no other thread is reading, setting, or removing environment variables.
pub unsafe fn ensure_dbus_session_daemon() -> anyhow::Result<Option<Child>> {
    if session_bus_running() {
        Ok(None)
    } else {
        let mut socket_path = glib::user_runtime_dir();
        socket_path.push(format!("xfwl4-{}", Alphanumeric.sample_string(&mut rand::rng(), 32)));

        let mut address = OsString::from("unix:path=");
        address.push(&socket_path);

        let mut child = Command::new("dbus-daemon")
            .arg("--session")
            .arg("--nofork")
            .arg("--nopidfile")
            .arg("--address")
            .arg(&address)
            .spawn()
            .context("Failed to spawn dbus-daemon")?;

        // SAFETY: This is safe if the function's safety constraints are met.
        unsafe {
            env::set_var("DBUS_SESSION_BUS_ADDRESS", &address);
        }

        let start = Instant::now();
        loop {
            const MAX_DBUS_WAIT_TIME: Duration = Duration::from_secs(2);

            if UnixStream::connect(&socket_path).is_ok() {
                break Ok(Some(child));
            } else if start.elapsed() > MAX_DBUS_WAIT_TIME {
                let _ = child.kill();
                break Err(anyhow!("Failed to start D-Bus session bus"));
            } else {
                sleep(Duration::from_millis(5));
            }
        }
    }
}

/// Fetch and parse the systemd NOTIFY_FD env var
///
/// This also removes the var from the environment, which we want to do early, before other threads
/// are started.
///
/// # Safety
///
/// This is only safe if called from a single-threaded environment, or if you can somehow guarantee
/// that no other thread is reading, setting, or removing environment variables.
#[cfg(feature = "udev")]
pub unsafe fn extract_notify_fd_from_env() -> anyhow::Result<Option<std::os::fd::RawFd>> {
    use anyhow::anyhow;

    match env::var("NOTIFY_FD") {
        Err(VarError::NotPresent) => Ok(None),
        Err(err) => {
            // SAFETY: This is safe if the function's safety constraints are met.
            unsafe {
                env::remove_var("NOTIFY_FD");
            }
            Err(anyhow!(
                "Unable to notify parent that we have started; env var NOTIFY_FD is not readable: {err}"
            ))
        }
        Ok(notify_fd) => {
            // SAFETY: This is safe if the function's safety constraints are met.
            unsafe {
                env::remove_var("NOTIFY_FD");
            }

            match notify_fd.parse() {
                Err(err) => Err(anyhow!("Failed to parse the value of the NOTIFY_FD env var: {err}")),
                Ok(notify_fd) => Ok(Some(notify_fd)),
            }
        }
    }
}

#[cfg(feature = "udev")]
pub fn import_environment() {
    let env_vars = [
        "DESKTOP_SESSION",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_CURRENT_DESKTOP",
        "XDG_MENU_PREFIX",
        "XDG_SESSION_TYPE",
    ];

    maybe_run_command("xdg-user-dirs-update", []);

    let mut args = vec!["--user", "import-environment"];
    args.extend(env_vars.iter());
    maybe_run_command("systemctl", args);

    maybe_run_command("dbus-update-activation-environment", env_vars);
}

#[cfg(feature = "udev")]
pub unsafe fn notify_fd(notify_fd: std::os::fd::RawFd) {
    use std::{fs::File, io::Write, os::fd::FromRawFd};

    // SAFETY: This may not be safe, as we have to trust the parent process that
    // the FD is valid and open.
    let mut notify = unsafe { File::from_raw_fd(notify_fd) };
    if let Err(err) = notify.write_all(b"READY=1\n") {
        warn!("Failed to write to notify FD: {err}");
    }
}

fn maybe_run_command<I, S>(command: S, args: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let command = command.as_ref();

    match Command::new(command).args(args).status() {
        Err(err) if err.kind() != ErrorKind::NotFound => warn!("Failed to run {command:?}: {err}"),
        Ok(status) if !status.success() => warn!("{command:?} exited with failure: {status}"),
        _ => (),
    }
}
