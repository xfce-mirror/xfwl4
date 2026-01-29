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

use std::{fmt, sync::mpsc};

use smithay::reexports::calloop::{
    self, EventSource,
    channel::{self, Channel},
    ping::{Ping, PingSource, make_ping},
};
use tracing::warn;

pub struct CalloopXfconfSource {
    channel: xfconf::Channel,
    rx: Channel<(String, glib::Value)>,
    ping_source: PingSource,
    ping: Ping,
}

impl CalloopXfconfSource {
    pub fn new<'a, I>(channel: xfconf::Channel, property_names: I) -> Self
    where
        I: IntoIterator<Item = &'a str>,
    {
        let (tx, rx) = channel::channel();
        let (ping, ping_source) = make_ping().expect("calloop ping creation failed");

        let source = Self {
            channel,
            rx,
            ping_source,
            ping,
        };

        let property_names = property_names.into_iter().collect::<Vec<_>>();
        let property_names = if !property_names.is_empty() {
            property_names.into_iter().map(Some).collect()
        } else {
            vec![None]
        };

        for property_name in property_names {
            source.channel.connect_property_changed(property_name, {
                let tx = tx.clone();
                let ping = source.ping.clone();
                move |_, name, value| match tx.send((name.to_owned(), value.clone())) {
                    Ok(_) => ping.ping(),
                    Err(err) => warn!("Failed to enqueue property-change notification for xfconf channel: {err}"),
                }
            });
        }

        source
    }
}

#[derive(Debug)]
pub struct CalloopXfconfSourceError(calloop::ping::PingError);

impl fmt::Display for CalloopXfconfSourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "xfconf source failed to process events: {}", self.0)
    }
}

impl std::error::Error for CalloopXfconfSourceError {
    fn cause(&self) -> Option<&dyn std::error::Error> {
        Some(&self.0)
    }
}

impl EventSource for CalloopXfconfSource {
    type Event = (String, glib::Value);
    type Metadata = ();
    type Ret = ();
    type Error = CalloopXfconfSourceError;

    fn register(&mut self, poll: &mut calloop::Poll, token_factory: &mut calloop::TokenFactory) -> calloop::Result<()> {
        self.ping_source.register(poll, token_factory)
    }

    fn reregister(&mut self, poll: &mut calloop::Poll, token_factory: &mut calloop::TokenFactory) -> calloop::Result<()> {
        self.ping_source.reregister(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut calloop::Poll) -> calloop::Result<()> {
        self.ping_source.unregister(poll)
    }

    fn process_events<F>(
        &mut self,
        readiness: calloop::Readiness,
        token: calloop::Token,
        mut callback: F,
    ) -> Result<calloop::PostAction, Self::Error>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        const MAX_EVENTS_PER_CHECK: usize = 1024;

        let rx = &self.rx;
        let mut is_empty = false;
        let mut is_disconnected = false;

        let action = self
            .ping_source
            .process_events(readiness, token, |(), &mut ()| {
                for _ in 0..MAX_EVENTS_PER_CHECK {
                    match rx.try_recv() {
                        Ok(event) => callback(event, &mut ()),
                        Err(mpsc::TryRecvError::Empty) => {
                            is_empty = true;
                            break;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            is_disconnected = true;
                            break;
                        }
                    }
                }
            })
            .map_err(CalloopXfconfSourceError)?;

        if is_disconnected {
            Ok(calloop::PostAction::Remove)
        } else if is_empty {
            Ok(action)
        } else {
            // Still more events in the channel; signal to do it aagin
            self.ping.ping();
            Ok(calloop::PostAction::Continue)
        }
    }
}
