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
    os::unix::process::ExitStatusExt,
    process::Child,
    sync::mpsc,
    thread::{self},
};

use calloop::{
    EventSource,
    channel::{Channel, channel},
    ping::{PingSource, make_ping},
};

pub struct CalloopChildSource {
    rx: Channel<ChildExited>,
    ping_source: PingSource,
}

#[derive(Debug)]
pub enum ChildExited {
    Normal(i32),
    Signaled(i32),
    Error(std::io::Error),
}

impl CalloopChildSource {
    pub fn new(mut child: Child) -> anyhow::Result<Self> {
        let (tx, rx) = channel();
        let (ping, ping_source) = make_ping()?;

        thread::Builder::new().name("calloop-child-watch".to_owned()).spawn(move || {
            let result = match child.wait() {
                Err(err) => {
                    let _ = child.kill();
                    ChildExited::Error(err)
                }
                Ok(status) => {
                    if let Some(code) = status.code() {
                        ChildExited::Normal(code)
                    } else if let Some(signal) = status.signal() {
                        ChildExited::Signaled(signal)
                    } else {
                        let _ = child.kill();
                        ChildExited::Error(std::io::Error::other("Child exit status invalid"))
                    }
                }
            };

            let _ = tx.send(result);
            ping.ping();
        })?;

        Ok(Self { rx, ping_source })
    }
}

impl EventSource for CalloopChildSource {
    type Event = ChildExited;
    type Metadata = ();
    type Ret = ();
    type Error = std::io::Error;

    fn process_events<F>(
        &mut self,
        readiness: calloop::Readiness,
        token: calloop::Token,
        mut callback: F,
    ) -> Result<calloop::PostAction, Self::Error>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        let rx = &self.rx;
        let mut is_done = false;

        let action = self
            .ping_source
            .process_events(readiness, token, |(), &mut ()| match rx.try_recv() {
                Ok(event) => {
                    callback(event, &mut ());
                    is_done = true;
                }
                Err(mpsc::TryRecvError::Empty) => (),
                Err(mpsc::TryRecvError::Disconnected) => is_done = true,
            })
            .map_err(std::io::Error::other)?;

        if is_done { Ok(calloop::PostAction::Remove) } else { Ok(action) }
    }

    fn register(&mut self, poll: &mut calloop::Poll, token_factory: &mut calloop::TokenFactory) -> calloop::Result<()> {
        self.ping_source.register(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut calloop::Poll) -> calloop::Result<()> {
        self.ping_source.unregister(poll)
    }

    fn reregister(&mut self, poll: &mut calloop::Poll, token_factory: &mut calloop::TokenFactory) -> calloop::Result<()> {
        self.ping_source.reregister(poll, token_factory)
    }
}
