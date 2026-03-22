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
    ffi::OsStr,
    os::{fd::AsRawFd, unix::ffi::OsStrExt},
    time::{Duration, Instant},
};

use anyhow::anyhow;
use rustix::{
    io::Errno,
    process::{Pid, WaitOptions},
};

use crate::{
    ui::{SupervisorComms, ui_main::run_ui},
    util::io::{close_all_fds, read_until, write_all},
};

const INITIAL_RESTART_DELAY: Duration = Duration::from_millis(8);
const MAX_RESTART_DELAY: Duration = Duration::from_secs(1);

enum SupervisorAction {
    Restart,
    Exit,
}

pub fn run_supervisor(supervisor_comms: SupervisorComms) -> anyhow::Result<()> {
    // Wait until the main process sends the socket name followed by a NUL byte.
    let mut buf = [0u8; 256];
    tracing::trace!("Waiting for socket name from main process");
    let socket_name_len =
        read_until(&supervisor_comms.from_main, &mut buf, b"\0").map_err(|err| anyhow!("Failed to read socket name: {err}"))?;

    let socket_name = OsStr::from_bytes(&buf[..(socket_name_len - 1)]);
    tracing::debug!("Main process is listening on display {socket_name:?}");
    // SAFETY: We will never spawn any other threads.
    unsafe { std::env::set_var("WAYLAND_DISPLAY", socket_name) };

    let mut restart_delay = INITIAL_RESTART_DELAY;

    loop {
        // Pipe for supervisor -> UI process comms.
        let (ui_from_supervisor_rx, supervisor_to_ui_tx) = rustix::pipe::pipe()?;
        let start_time = Instant::now();

        // SAFETY: We're starting in a single-threaded process.
        match unsafe { libc::fork() } {
            -1 => break Err(anyhow!("fork() for UI process failed")),

            0 => {
                drop(supervisor_to_ui_tx);

                let except = &[ui_from_supervisor_rx.as_raw_fd()];
                close_all_fds(except);

                run_ui(ui_from_supervisor_rx)?;
            }

            ui_pid => {
                tracing::info!("UI process started as PID {ui_pid}");
                let ui_pid = Pid::from_raw(ui_pid).ok_or_else(|| anyhow!("PID returned from fork() was invalid"))?;

                // Write the UI's PID back to the main process.
                let ui_pid_bytes = ui_pid.as_raw_pid().to_ne_bytes();
                write_all(&supervisor_comms.to_main, &ui_pid_bytes).map_err(|err| anyhow!("Failed to write PID to main process: {err}"))?;

                // Wait until the main process got the message.
                let mut zero = [0u8; 1];
                read_until(&supervisor_comms.from_main, &mut zero, b"\0")
                    .map_err(|err| anyhow!("Failed to read PID ACK from main process: {err}"))?;

                // Tell the UI process to initialize and get to work.
                write_all(supervisor_to_ui_tx, &zero).map_err(|err| anyhow!("Failed to write start signal to UI process: {err}"))?;

                match supervise(ui_pid).map_err(|err| anyhow!("UI process supervision failed: {err}"))? {
                    SupervisorAction::Restart => {
                        if start_time.elapsed() > MAX_RESTART_DELAY * 2 {
                            restart_delay = INITIAL_RESTART_DELAY;
                        }

                        tracing::debug!("Restarting UI process in {}ms", restart_delay.as_millis());
                        std::thread::sleep(restart_delay);
                        restart_delay *= 2;
                    }
                    SupervisorAction::Exit => break Ok(()),
                }
            }
        }
    }
}

fn supervise(ui_process_pid: Pid) -> anyhow::Result<SupervisorAction> {
    loop {
        match rustix::process::waitpid(Some(ui_process_pid), WaitOptions::UNTRACED | WaitOptions::CONTINUED) {
            Ok(None) => {
                // This only happens if you pass WaitOptions::NO_HANG, which we are not doing, but
                // I suppose perhaps it could also happen if the syscall is interrupted?  So let's
                // just loop again, after a short delay in order to avoid spinning like crazy if
                // something weird is going on.
                tracing::warn!("waitpid() on UI process returned no status; this shouldn't happen");
                std::thread::sleep(Duration::from_millis(16));
            }
            Ok(Some((_, status))) if status.exited() && status.exit_status().is_some_and(|es| es == 0) => {
                tracing::debug!("UI process has shut down, so we will too");
                break Ok(SupervisorAction::Exit);
            }
            Ok(Some((_, status))) if status.exited() => {
                tracing::warn!("UI process has exited with status {}; restarting", status.exit_status().unwrap());
                break Ok(SupervisorAction::Restart);
            }
            Ok(Some((_, status))) if status.signaled() => {
                tracing::warn!(
                    "UI proces has exited with signal {}; restarting",
                    status.terminating_signal().unwrap()
                );
                break Ok(SupervisorAction::Restart);
            }
            Ok(Some((_, status))) if status.stopped() => {
                // The UI process has been stopped via a signal.  We don't love that
                // it was stopped, because that will make things in the main process behave
                // strangely, but there's not too much we can do about it except loop again and
                // hope that someone resumes it.
                tracing::warn!("UI process has been stopped by a signal; the compositor may behave strangely until it is resumed");
                std::thread::sleep(Duration::from_millis(16));
            }
            Ok(Some(_)) => {
                // This is probably (hopefully) the resumed case (`status.continued()`), so just
                // loop again.
            }
            Err(Errno::AGAIN | Errno::CHILD | Errno::INTR) => {
                // AGAIN shouldn't happen, as we didn't specify WaitOptions::NOHANG (and rustix
                // turns that into a None return, anyway). But CHILD can happen if the UI process
                // dies before we even get to call waitpid(), and INTR can happen if we're
                // interrupted by a signal (which we shouldn't be; any signal that interrupts us
                // should probably kill us), so we should just try again.
            }
            Err(err) => break Err(err.into()),
        }
    }
}
