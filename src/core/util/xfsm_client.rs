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

use std::{collections::HashMap, sync::Arc, time::Duration};

use calloop::channel::{Channel, Sender, channel};
use futures::lock::Mutex;
use gio::{BusNameWatcherFlags, DBusConnection, DBusProxyFlags};

use crate::core::util::xfsm_client::xfsm_api::session_manager::SessionManagerProxy;

#[gdbus_codegen_macros::generate(
    input_paths = [
        "resources/xfsm-manager-dbus.xml",
    ],
    kind = "clients",
    interface_prefix = "org.xfce.",
    include_interfaces = [
        "org.xfce.Session.Manager",
    ],
)]
mod xfsm_api {}

const SESSION_MANAGER_NAME: &str = "org.xfce.SessionManager";
const SESSION_MANAGER_PATH: &str = "/org/xfce/SessionManager";

const XFSM_STATE_CHECKPOINT: u32 = 2;
const XFSM_STATE_SHUTDOWN: u32 = 3;

#[derive(Clone)]
pub struct XfsmClient {
    event_tx: Sender<SessionEvent>,
    bus: DBusConnection,
    proxies: Arc<Mutex<SessionProxies>>,
    _xfsm_name_owner_watcher_id: Arc<NameOwnerWatcherId>,
}

pub enum SessionEvent {
    AvailabilityChanged(bool),
    Checkpoint,
    ShutdownStarted,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Session management is unavailable")]
    SmUnavailable,
    #[error("{0}")]
    Other(
        #[from]
        #[source]
        anyhow::Error,
    ),
}

// We need to avoid naming the `WatcherId` type due to a bug in gio-rs:
// https://github.com/gtk-rs/gtk-rs-core/pull/2054
struct NameOwnerWatcherId(Option<Box<dyn FnOnce() + Send + Sync>>);

#[derive(Debug, Default, PartialEq)]
enum SessionProxies {
    #[default]
    Uninitialized,
    Available {
        manager: SessionManagerProxy,
    },
    Unavailable,
}

impl XfsmClient {
    pub fn new() -> anyhow::Result<(Channel<SessionEvent>, Self)> {
        let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
        let proxies = Arc::new(Mutex::new(SessionProxies::default()));
        let (event_tx, event_rx) = channel::<SessionEvent>();

        let handle_sm_available_changed = {
            let proxies = Arc::clone(&proxies);
            let event_tx = event_tx.clone();
            move |sm_available: bool| {
                // We don't normally use the glib futures executor, but I don't want
                // `XfsmClient::new()` to take a `LoopHandle` and have to create its own.
                glib::spawn_future_local({
                    let event_tx = event_tx.clone();
                    let proxies = Arc::clone(&proxies);
                    async move {
                        let mut guard = proxies.lock().await;

                        if sm_available {
                            *guard = SessionProxies::Uninitialized;
                            let _ = event_tx.send(SessionEvent::AvailabilityChanged(true));
                        } else if !matches!(*guard, SessionProxies::Unavailable) {
                            *guard = SessionProxies::Unavailable;
                            let _ = event_tx.send(SessionEvent::AvailabilityChanged(false));
                        }
                    }
                });
            }
        };

        let watcher_id = gio::bus_watch_name_on_connection(
            &bus,
            SESSION_MANAGER_NAME,
            BusNameWatcherFlags::NONE,
            {
                let handle_sm_available_changed = handle_sm_available_changed.clone();
                move |_, _, _| handle_sm_available_changed(true)
            },
            move |_, _| handle_sm_available_changed(false),
        );

        Ok((
            event_rx,
            Self {
                event_tx,
                bus,
                proxies,
                _xfsm_name_owner_watcher_id: Arc::new(NameOwnerWatcherId(Some(Box::new(move || gio::bus_unwatch_name(watcher_id))))),
            },
        ))
    }

    pub async fn get_environment(&self) -> Result<HashMap<String, String>, SessionError> {
        if let Some(manager) = self.proxies.lock().await.manager(&self.bus, &self.event_tx).await {
            Ok(manager.get_environment_future().await?)
        } else {
            Err(SessionError::SmUnavailable)
        }
    }

    pub async fn request_logout(&self) -> Result<(), SessionError> {
        if let Some(manager) = self.proxies.lock().await.manager(&self.bus, &self.event_tx).await {
            Ok(manager.logout_future(true, true).await?)
        } else {
            Err(SessionError::SmUnavailable)
        }
    }
}

impl Drop for NameOwnerWatcherId {
    fn drop(&mut self) {
        if let Some(unwatch) = self.0.take() {
            unwatch()
        }
    }
}

impl SessionProxies {
    async fn manager(&mut self, bus: &DBusConnection, event_tx: &Sender<SessionEvent>) -> Option<&SessionManagerProxy> {
        self.init(bus, event_tx).await;
        match self {
            Self::Available { manager, .. } => Some(manager),
            _ => None,
        }
    }

    async fn init(&mut self, bus: &DBusConnection, event_tx: &Sender<SessionEvent>) {
        if *self == Self::Uninitialized {
            let result = SessionManagerProxy::new_future(
                bus,
                DBusProxyFlags::DO_NOT_LOAD_PROPERTIES,
                Some(SESSION_MANAGER_NAME),
                SESSION_MANAGER_PATH,
                Some(Duration::from_millis(500)),
            )
            .await;

            *self = match result {
                Ok(manager) => {
                    let event_tx = event_tx.clone();
                    manager.connect_state_changed(move |_, _, new_state| {
                        if new_state == XFSM_STATE_CHECKPOINT {
                            let _ = event_tx.send(SessionEvent::Checkpoint);
                        } else if new_state == XFSM_STATE_SHUTDOWN {
                            let _ = event_tx.send(SessionEvent::ShutdownStarted);
                        }
                    });

                    Self::Available { manager }
                }
                Err(err) => {
                    tracing::warn!("Unable to connect to xfce4-session: {err}");
                    Self::Unavailable
                }
            };
        }
    }
}

impl From<glib::Error> for SessionError {
    fn from(value: glib::Error) -> Self {
        Self::Other(value.into())
    }
}
