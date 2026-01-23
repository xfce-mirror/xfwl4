use std::{
    env::{self, VarError},
    ffi::OsStr,
    io::ErrorKind,
    process::Command,
};

use anyhow::Context;
use tracing::warn;

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

    Ok(())
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
pub unsafe fn notify_fd() {
    match env::var("NOTIFY_FD") {
        Err(VarError::NotPresent) => (),
        Err(err) => {
            warn!("Unable to notify parent that we have started; env var is not readable: {err}");
            // SAFETY: This may not be safe, as other threads have been started, and we can't be
            // sure what they are doing.
            unsafe {
                env::remove_var("NOTIFY_FD");
            }
        }
        Ok(notify_fd) => {
            // SAFETY: This may not be safe, as other threads have been started, and we can't be
            // sure what they are doing.
            unsafe {
                env::remove_var("NOTIFY_FD");
            }

            match notify_fd.parse() {
                Err(err) => warn!("Failed to parse the value of the NOTIFY_FD env var: {err}"),
                Ok(notify_fd) => {
                    use std::{fs::File, io::Write, os::fd::FromRawFd};

                    // SAFETY: This may not be safe, as we have to trust the parent process that
                    // the FD is valid and open.
                    let mut notify = unsafe { File::from_raw_fd(notify_fd) };
                    if let Err(err) = notify.write_all(b"READY=1\n") {
                        warn!("Failed to write to notify FD: {err}");
                    }
                }
            }
        }
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
