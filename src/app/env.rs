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
use xfwl4::build_config::{BUILD_DATADIR, BUILD_SYSCONFDIR};

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

    fn set_if_unset(varname: &str, val_if_unset: impl FnOnce() -> PathBuf) {
        if let Err(VarError::NotPresent) = env::var(varname) {
            let value = val_if_unset();
            // SAFETY: This is safe if the function's safety constraints are met.
            unsafe {
                env::set_var(varname, value);
            }
        }
    }

    set_if_unset("XDG_CONFIG_HOME", || {
        let mut config_home = home.clone();
        config_home.push(".config");
        config_home
    });

    set_if_unset("XDG_CACHE_HOME", || {
        let mut cache_home = home.clone();
        cache_home.push(".cache");
        cache_home
    });

    set_if_unset("XDG_DATA_HOME", || {
        let mut data_home = home.clone();
        data_home.push(".local");
        data_home.push("share");
        data_home
    });

    set_if_unset("XDG_STATE_HOME", || {
        let mut state_home = home.clone();
        state_home.push(".local");
        state_home.push("state");
        state_home
    });

    set_if_unset("XDG_RUNTIME_DIR", || {
        let mut runtime_dir = PathBuf::from("/run/user");
        runtime_dir.push(rustix::process::getuid().as_raw().to_string());

        if !runtime_dir.exists() {
            runtime_dir = glib::user_runtime_dir();
        }

        runtime_dir
    });

    fn reset_var_array(varname: &str, setter: impl FnOnce(Vec<String>) -> Vec<String>) {
        let values = env::var(varname)
            .map(|var| var.split(":").map(ToOwned::to_owned).collect::<Vec<_>>())
            .unwrap_or_default();
        let new_value = setter(values).join(":");
        // SAFETY: This is safe if the function's safety constraints are met.
        unsafe {
            env::set_var(varname, new_value);
        }
    }

    reset_var_array("XDG_CONFIG_DIRS", |mut config_dirs| {
        let sys_config_dir = "/etc/xdg".to_owned();
        if !config_dirs.contains(&sys_config_dir) {
            config_dirs.push(sys_config_dir);
        }

        let sysconfdir_xdg = format!("{BUILD_SYSCONFDIR}/xdg");
        if !config_dirs.contains(&sysconfdir_xdg) {
            config_dirs.insert(0, sysconfdir_xdg);
        }

        config_dirs
    });

    reset_var_array("XDG_DATA_DIRS", |mut data_dirs| {
        let local_sys_data_dir = "/usr/local/share".to_owned();
        if !data_dirs.contains(&local_sys_data_dir) {
            data_dirs.push(local_sys_data_dir);
        }

        let sys_data_dir = "/usr/share".to_owned();
        if !data_dirs.contains(&sys_data_dir) {
            data_dirs.push(sys_data_dir);
        }

        let datadir = BUILD_DATADIR.to_owned();
        if !data_dirs.contains(&datadir) {
            data_dirs.insert(0, datadir);
        }

        let datadir_xfce = format!("{BUILD_DATADIR}/xfce4");
        if !data_dirs.contains(&datadir_xfce) {
            data_dirs.insert(0, datadir_xfce);
        }

        data_dirs
    });

    let new_xfce4_session_compositor = match env::var("XFCE4_SESSION_COMPOSITOR") {
        Err(_) => Some("xfwl4"),
        Ok(val) => (!val.starts_with("xfwl4") && !val.contains("/xfwl4")).then_some("xfwl4"),
    };
    if let Some(xfce4_session_compositor) = new_xfce4_session_compositor {
        // SAFETY: This is safe if the function's safety constraints are met.
        unsafe {
            env::set_var("XFCE4_SESSION_COMPOSITOR", xfce4_session_compositor);
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
        "XDG_CONFIG_DIRS",
        "XDG_CONFIG_HOME",
        "XDG_CURRENT_DESKTOP",
        "XDG_DATA_DIRS",
        "XDG_DATA_HOME",
        "XDG_MENU_PREFIX",
        "XDG_RUNTIME_DIR",
        "XDG_SESSION_TYPE",
        "XDG_STATE_HOME",
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
