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

use std::{collections::HashMap, fmt, sync::Arc, time::Duration};

use anyhow::anyhow;
use calloop::channel::{Channel, Sender, channel};
use futures::{FutureExt, TryFutureExt, lock::Mutex};
use gio::{BusNameWatcherFlags, DBusCallFlags, DBusConnection, DBusError, DBusNodeInfo, DBusProxyFlags};
use glib::variant::ObjectPath;

use crate::{
    core::util::xfsm_client::xfsm_api::{
        session_client_delegate::SessionClientDelegateProxy, session_manager::SessionManagerProxy,
        session_manager_delegate::SessionManagerDelegateProxy,
    },
    protocols::xdg_session_management::proto::xdg_session_manager_v1::Reason,
};

#[gdbus_codegen_macros::generate(
    input_paths = [
        "resources/xfsm-manager-dbus.xml",
        "resources/xfsm-client-dbus.xml",
    ],
    kind = "clients",
    interface_prefix = "org.xfce.",
    include_interfaces = [
        "org.xfce.Session.ClientDelegate",
        "org.xfce.Session.Manager",
        "org.xfce.Session.ManagerDelegate",
    ],
)]
mod xfsm_api {}

const SESSION_MANAGER_NAME: &str = "org.xfce.SessionManager";
const SESSION_MANAGER_PATH: &str = "/org/xfce/SessionManager";
const SESSION_MANAGER_DELEGATE_INTERFACE: &str = "org.xfce.Session.ManagerDelegate";

const XFSM_ERROR_NAME_CONFLICT: &str = "org.xfce.SessionManager.Error.NameConflict";

const XFSM_STATE_CHECKPOINT: u32 = 2;
const XFSM_STATE_SHUTDOWN: u32 = 3;

const INTROSPECTABLE_INTERFACE_NAME: &str = "org.freedesktop.DBus.Introspectable";

#[derive(Clone)]
pub struct XfsmClient {
    event_tx: Sender<SessionEvent>,
    bus: DBusConnection,
    proxies: Arc<Mutex<SessionProxies>>,
    _xfsm_name_owner_watcher_id: Arc<NameOwnerWatcherId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Created,
    Restored,
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
    #[error("Specified name is already in use")]
    NameInUse,
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
        manager_delegate: SessionManagerDelegateProxy,
        client_delegates: HashMap<ObjectPath, SessionClientDelegateProxy>,
    },
    Unavailable,
}

impl XfsmClient {
    pub fn new() -> anyhow::Result<(Channel<SessionEvent>, Self)> {
        let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
        let proxies = Arc::new(Mutex::new(SessionProxies::default()));
        let (event_tx, event_rx) = channel::<SessionEvent>();

        let handle_sm_available_changed = {
            let bus = bus.clone();
            let proxies = Arc::clone(&proxies);
            let event_tx = event_tx.clone();
            move |sm_available: bool| {
                // We don't normally use the glib futures executor, but I don't want
                // `XfsmClient::new()` to take a `LoopHandle` and have to create its own.
                glib::spawn_future_local({
                    let bus = bus.clone();
                    let event_tx = event_tx.clone();
                    let proxies = Arc::clone(&proxies);
                    async move {
                        let mut guard = proxies.lock().await;

                        if sm_available {
                            let has_manager_delegate = bus
                                .call_future(
                                    Some(SESSION_MANAGER_NAME),
                                    SESSION_MANAGER_PATH,
                                    INTROSPECTABLE_INTERFACE_NAME,
                                    "Introspect",
                                    None,
                                    None,
                                    DBusCallFlags::NONE,
                                    1000,
                                )
                                .map(|result| match result {
                                    Err(err) => Err(anyhow!("failed to fetch introspection data for session manager: {err}")),
                                    Ok(retval) => retval
                                        .try_child_get::<String>(0)
                                        .map_err(|err| anyhow!("invalid return from 'Introspect' method: {err}"))
                                        .and_then(|retval0| {
                                            retval0.ok_or_else(|| anyhow!("no XML string returned from 'Introspect' method"))
                                        })
                                        .and_then(|xml| {
                                            DBusNodeInfo::for_xml(&xml).map_err(|err| anyhow!("failed to parse introspection data: {err}"))
                                        })
                                        .and_then(|node| {
                                            node.lookup_interface(SESSION_MANAGER_DELEGATE_INTERFACE)
                                                .map(|_| ())
                                                .ok_or_else(|| anyhow!("xfce4-session is too old"))
                                        }),
                                })
                                .await;

                            if let Err(err) = has_manager_delegate {
                                tracing::warn!("Session management unavailable: {err}");
                                *guard = SessionProxies::Unavailable;
                            } else {
                                *guard = SessionProxies::Uninitialized;
                                let _ = event_tx.send(SessionEvent::AvailabilityChanged(true));
                            }
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

    //
    // Manager methods
    //

    pub async fn request_logout(&self) -> Result<(), SessionError> {
        if let Some(manager) = self.proxies.lock().await.manager(&self.bus, &self.event_tx).await {
            Ok(manager.logout_future(true, true).await?)
        } else {
            Err(SessionError::SmUnavailable)
        }
    }

    //
    // ManagerDelegate methods
    //

    pub async fn register_client(
        &self,
        client_id: &str,
        pid: u32,
        reason: Reason,
    ) -> Result<(ObjectPath, Reason, SessionStatus), SessionError> {
        if let Some(manager_delegate) = self.proxies.lock().await.manager_delegate(&self.bus, &self.event_tx).await {
            Ok(manager_delegate
                .register_client_future(client_id, pid, reason.as_str())
                .map_ok(|(object_path, reason, status)| {
                    (
                        object_path,
                        Reason::try_from(reason).unwrap_or(Reason::Launch),
                        SessionStatus::try_from(status).unwrap_or(SessionStatus::Created),
                    )
                })
                .await?)
        } else {
            Err(SessionError::SmUnavailable)
        }
    }

    pub async fn remove_client(&self, object_path: &ObjectPath) -> Result<(), SessionError> {
        let mut proxies = self.proxies.lock().await;
        proxies.drop_client_delegate(object_path);

        if let Some(manager_delegate) = proxies.manager_delegate(&self.bus, &self.event_tx).await {
            Ok(manager_delegate.remove_client_future(object_path).await?)
        } else {
            Err(SessionError::SmUnavailable)
        }
    }

    pub async fn client_disconnected(&self, object_path: &ObjectPath) -> Result<(), SessionError> {
        let mut proxies = self.proxies.lock().await;
        proxies.drop_client_delegate(object_path);

        if let Some(manager_delegate) = proxies.manager_delegate(&self.bus, &self.event_tx).await {
            Ok(manager_delegate.client_disconnected_future(object_path).await?)
        } else {
            Err(SessionError::SmUnavailable)
        }
    }

    //
    // ClientDelegate methods
    //

    pub async fn set_app_id(&self, object_path: &ObjectPath, app_id: &str) -> Result<(), SessionError> {
        if let Some(client_delegate) = self
            .proxies
            .lock()
            .await
            .client_delegate(&self.bus, object_path, &self.event_tx)
            .await
        {
            Ok(client_delegate.set_app_id_future(app_id).await?)
        } else {
            Err(SessionError::SmUnavailable)
        }
    }

    pub async fn register_toplevel(&self, object_path: &ObjectPath, toplevel_id: &str) -> Result<(), SessionError> {
        if let Some(client_delegate) = self
            .proxies
            .lock()
            .await
            .client_delegate(&self.bus, object_path, &self.event_tx)
            .await
        {
            Ok(client_delegate.register_toplevel_future(toplevel_id).await?)
        } else {
            Err(SessionError::SmUnavailable)
        }
    }

    pub async fn restore_toplevel(
        &self,
        object_path: &ObjectPath,
        toplevel_id: &str,
    ) -> Result<HashMap<String, glib::Variant>, SessionError> {
        if let Some(client_delegate) = self
            .proxies
            .lock()
            .await
            .client_delegate(&self.bus, object_path, &self.event_tx)
            .await
        {
            Ok(client_delegate.restore_toplevel_future(toplevel_id).await?)
        } else {
            Err(SessionError::SmUnavailable)
        }
    }

    pub async fn replace_toplevel_wm_properties(
        &self,
        object_path: &ObjectPath,
        toplevel_id: &str,
        wm_properties: &HashMap<String, glib::Variant>,
    ) -> Result<(), SessionError> {
        if let Some(client_delegate) = self
            .proxies
            .lock()
            .await
            .client_delegate(&self.bus, object_path, &self.event_tx)
            .await
        {
            // NoReply method: only fails locally, never waits on a reply.
            Ok(client_delegate.replace_toplevel_wm_properties(toplevel_id, wm_properties)?)
        } else {
            Err(SessionError::SmUnavailable)
        }
    }

    pub async fn rename_toplevel(&self, object_path: &ObjectPath, toplevel_id: &str, new_toplevel_id: &str) -> Result<(), SessionError> {
        if let Some(client_delegate) = self
            .proxies
            .lock()
            .await
            .client_delegate(&self.bus, object_path, &self.event_tx)
            .await
        {
            Ok(client_delegate.rename_toplevel_future(toplevel_id, new_toplevel_id).await?)
        } else {
            Err(SessionError::SmUnavailable)
        }
    }

    pub async fn remove_toplevel(&self, object_path: &ObjectPath, toplevel_id: &str) -> Result<(), SessionError> {
        if let Some(client_delegate) = self
            .proxies
            .lock()
            .await
            .client_delegate(&self.bus, object_path, &self.event_tx)
            .await
        {
            Ok(client_delegate.remove_toplevel_future(toplevel_id).await?)
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

    async fn manager_delegate(&mut self, bus: &DBusConnection, event_tx: &Sender<SessionEvent>) -> Option<&SessionManagerDelegateProxy> {
        self.init(bus, event_tx).await;
        match self {
            Self::Available { manager_delegate, .. } => Some(manager_delegate),
            _ => None,
        }
    }

    async fn client_delegate(
        &mut self,
        bus: &DBusConnection,
        object_path: &ObjectPath,
        event_tx: &Sender<SessionEvent>,
    ) -> Option<&SessionClientDelegateProxy> {
        self.init(bus, event_tx).await;
        match self {
            Self::Available { client_delegates, .. } => {
                if !client_delegates.contains_key(object_path) {
                    match SessionClientDelegateProxy::new_future(
                        bus,
                        DBusProxyFlags::DO_NOT_LOAD_PROPERTIES | DBusProxyFlags::DO_NOT_CONNECT_SIGNALS,
                        Some(SESSION_MANAGER_NAME),
                        object_path.as_str(),
                        Some(Duration::from_millis(500)),
                    )
                    .await
                    {
                        Err(err) => tracing::info!("Failed to create client dbus proxy for {object_path}: {err}"),
                        Ok(proxy) => {
                            client_delegates.insert(object_path.clone(), proxy);
                        }
                    }
                }

                client_delegates.get(object_path)
            }
            _ => None,
        }
    }

    fn drop_client_delegate(&mut self, object_path: &ObjectPath) {
        if let Self::Available { client_delegates, .. } = self {
            client_delegates.remove(object_path);
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
            .and_then(|manager| {
                SessionManagerDelegateProxy::new_future(
                    bus,
                    DBusProxyFlags::DO_NOT_LOAD_PROPERTIES | DBusProxyFlags::DO_NOT_CONNECT_SIGNALS,
                    Some(SESSION_MANAGER_NAME),
                    SESSION_MANAGER_PATH,
                    Some(Duration::from_millis(500)),
                )
                .map_ok(|manager_delegate| (manager, manager_delegate))
            })
            .await;

            *self = match result {
                Ok((manager, manager_delegate)) => {
                    let event_tx = event_tx.clone();
                    manager.connect_state_changed(move |_, _, new_state| {
                        if new_state == XFSM_STATE_CHECKPOINT {
                            let _ = event_tx.send(SessionEvent::Checkpoint);
                        } else if new_state == XFSM_STATE_SHUTDOWN {
                            let _ = event_tx.send(SessionEvent::ShutdownStarted);
                        }
                    });

                    Self::Available {
                        manager,
                        manager_delegate,
                        client_delegates: HashMap::new(),
                    }
                }
                Err(err) => {
                    tracing::warn!("Unable to connect to xfce4-session; session management will be unavailable: {err}");
                    Self::Unavailable
                }
            };
        }
    }
}

impl From<glib::Error> for SessionError {
    fn from(value: glib::Error) -> Self {
        if DBusError::remote_error(&value).is_some_and(|name| name == XFSM_ERROR_NAME_CONFLICT) {
            Self::NameInUse
        } else {
            Self::Other(value.into())
        }
    }
}

impl fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Created => f.write_str("created"),
            Self::Restored => f.write_str("restored"),
        }
    }
}

impl TryFrom<String> for SessionStatus {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "created" => Ok(Self::Created),
            "restored" => Ok(Self::Restored),
            other => Err(anyhow!("Unknown SessionStatus value \"{other}\"")),
        }
    }
}
