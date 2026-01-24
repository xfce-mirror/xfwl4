use std::{fmt, sync::mpsc};

use smithay::reexports::calloop::{
    self, EventSource,
    channel::{self, Channel},
    ping::{Ping, PingSource, make_ping},
};
use tracing::warn;

/// Zips `first` with `second`, ensuring all elements in `first` are iterated over.
///
/// Similar to .zip(), but instead of returning None when either iterator returns None, this
/// ensures that every element in `first` is iterated over, while every element in `second` is
/// wrapped in `Some` in the resulting iterator.  If `second` runs out before `first` does, there
/// will be `None` in the second slot of the tuple.
pub fn zip_all_first<I, J>(first: I, second: J) -> impl Iterator<Item = (I::Item, Option<J::Item>)>
where
    I: IntoIterator,
    J: IntoIterator,
{
    let mut second_iter = second.into_iter();
    first.into_iter().map(move |item| (item, second_iter.next()))
}

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
