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
    ffi::OsString,
    os::unix::process::ExitStatusExt,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use smithay::reexports::{
    calloop::channel::{Channel, channel},
    rustix::process::{Pid, Signal, kill_process},
};

pub struct Session {
    session_pid: Pid,
    terminated: Arc<AtomicBool>,
}

impl Session {
    pub fn start(command: OsString, args: Vec<OsString>) -> anyhow::Result<(Self, Channel<()>)> {
        let mut session_child = Command::new(&command).args(args).spawn()?;
        let session_pid = Pid::from_child(&session_child);
        let (tx, rx) = channel();
        let terminated = Arc::new(AtomicBool::new(false));

        thread::spawn({
            let terminated = Arc::clone(&terminated);
            move || {
                loop {
                    match session_child.wait() {
                        Err(err) => {
                            tracing::error!("Session app did not start: {err}");
                            break;
                        }
                        Ok(status) => {
                            if let Some(code) = status.code() {
                                if code != 0 && !terminated.load(Ordering::Acquire) {
                                    tracing::warn!("Session app exited with code {code}");
                                }
                                break;
                            } else if let Some(signum) = status.signal() {
                                if !terminated.load(Ordering::Acquire) {
                                    tracing::warn!("Session app exited with signal {signum}");
                                }
                                break;
                            }
                        }
                    }
                }

                terminated.store(true, Ordering::Release);
                let _ = tx.send(());
            }
        });

        Ok((Self { session_pid, terminated }, rx))
    }

    pub fn kill(&self) {
        if !self.terminated.load(Ordering::Acquire) {
            self.terminated.store(true, Ordering::Release);
            let _ = kill_process(self.session_pid, Signal::TERM);
        }
    }
}
