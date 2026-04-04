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
    collections::HashMap,
    sync::{Arc, Mutex},
};

use rand::distr::{Alphanumeric, SampleString};
use smithay::{
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel,
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
            backend::{ClientId, GlobalId},
        },
    },
    utils::user_data::UserDataMap,
};

use crate::protocols::xdg_session_management::proto::{
    xdg_session_manager_v1::{Reason, XdgSessionManagerV1},
    xdg_session_v1::XdgSessionV1,
    xdg_toplevel_session_v1::XdgToplevelSessionV1,
};

pub struct SessionManagementState {
    dh: DisplayHandle,
    _global: GlobalId,
    manager_instances: Vec<XdgSessionManagerV1>,
    sessions: HashMap<XdgSessionV1, Session>,
}

pub trait SessionManagementHandler
where
    Self: GlobalDispatch<XdgSessionManagerV1, ()>
        + Dispatch<XdgSessionManagerV1, ()>
        + Dispatch<XdgSessionV1, ()>
        + Dispatch<XdgToplevelSessionV1, ()>
        + Sized
        + 'static,
{
    fn session_management_state(&mut self) -> &mut SessionManagementState;

    fn has_session(&mut self, session_id: &str) -> bool;

    fn new_session(&mut self, session: Session, reason: Reason);
    fn replace_session(&mut self, replacement_session: Session);
    fn remove_session(&mut self, session: Session);

    fn add_toplevel(&mut self, session: Session, toplevel: ToplevelSession);
    fn restore_toplevel(&mut self, session: Session, toplevel: ToplevelSession) -> bool;
    fn rename_toplevel(&mut self, session: Session, toplevel: ToplevelSession);
    fn remove_toplevel(&mut self, session: Session, toplevel: ToplevelSession);
}

#[derive(Debug)]
struct SessionInner {
    instance: XdgSessionV1,
    id: String,
    toplevels: HashMap<XdgToplevelSessionV1, ToplevelSession>,
    toplevel_instances: HashMap<String, XdgToplevelSessionV1>,
}

#[derive(Debug, Clone)]
pub struct Session {
    inner: Arc<(Mutex<SessionInner>, UserDataMap)>,
}

#[derive(Debug)]
struct ToplevelSessionInner {
    instance: XdgToplevelSessionV1,
    toplevel: XdgToplevel,
    name: String,
}

#[derive(Debug, Clone)]
pub struct ToplevelSession {
    inner: Arc<(Mutex<ToplevelSessionInner>, UserDataMap)>,
}

impl SessionManagementState {
    pub fn new<H: SessionManagementHandler>(dh: &DisplayHandle) -> Self {
        let global = dh.create_global::<H, XdgSessionManagerV1, _>(1, ());
        Self {
            dh: dh.clone(),
            _global: global,
            manager_instances: Vec::new(),
            sessions: HashMap::new(),
        }
    }
}

impl Session {
    pub fn id(&self) -> String {
        self.inner.0.lock().unwrap().id.clone()
    }

    pub fn toplevels(&self) -> Vec<ToplevelSession> {
        self.inner.0.lock().unwrap().toplevels.values().cloned().collect()
    }

    pub fn user_data(&self) -> &UserDataMap {
        &self.inner.1
    }
}

impl ToplevelSession {
    pub fn name(&self) -> String {
        self.inner.0.lock().unwrap().name.clone()
    }

    pub fn user_data(&self) -> &UserDataMap {
        &self.inner.1
    }
}

impl<H: SessionManagementHandler> GlobalDispatch<XdgSessionManagerV1, (), H> for SessionManagementState {
    fn bind(
        state: &mut H,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<XdgSessionManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, H>,
    ) {
        let instance = data_init.init(resource, ());
        state.session_management_state().manager_instances.push(instance);
    }
}

impl<H: SessionManagementHandler> Dispatch<XdgSessionManagerV1, (), H> for SessionManagementState {
    fn request(
        state: &mut H,
        client: &Client,
        resource: &XdgSessionManagerV1,
        request: <XdgSessionManagerV1 as Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, H>,
    ) {
        use proto::xdg_session_manager_v1::{Error, Reason, Request};

        match request {
            Request::GetSession { id, reason, session_id } => {
                let instance = data_init.init(id, ());

                enum NewSessionStatus {
                    CreateNew,
                    Replaced(String),
                    Exists(String),
                }

                let new_session_status = if let Some(existing_session_id) = session_id {
                    if let Some(existing) = state
                        .session_management_state()
                        .sessions
                        .values()
                        .find(|session| session.inner.0.lock().unwrap().id == existing_session_id)
                        && let Some(existing_client) = existing.inner.0.lock().unwrap().instance.client()
                    {
                        if existing_client == *client {
                            resource.post_error(Error::InUse, format!("session with id '{existing_session_id}' is already in use"));
                            None
                        } else {
                            existing.inner.0.lock().unwrap().instance.replaced();
                            Some(NewSessionStatus::Replaced(existing_session_id))
                        }
                    } else if state.has_session(&existing_session_id) {
                        Some(NewSessionStatus::Exists(existing_session_id))
                    } else {
                        Some(NewSessionStatus::CreateNew)
                    }
                } else {
                    Some(NewSessionStatus::CreateNew)
                };

                if let Some(new_session_status) = new_session_status {
                    let create_new_session = |state: &mut H, id: String| {
                        let session = Session {
                            inner: Arc::new((
                                Mutex::new(SessionInner {
                                    instance: instance.clone(),
                                    id: id.clone(),
                                    toplevels: HashMap::new(),
                                    toplevel_instances: HashMap::new(),
                                }),
                                UserDataMap::new(),
                            )),
                        };
                        state.session_management_state().sessions.insert(instance.clone(), session.clone());
                        session
                    };

                    let reason = match reason {
                        WEnum::Value(reason) => reason,
                        _ => Reason::Launch,
                    };

                    match new_session_status {
                        NewSessionStatus::CreateNew => {
                            let id = Alphanumeric.sample_string(&mut rand::rng(), 32);
                            let session = create_new_session(state, id.clone());
                            state.new_session(session, reason);
                            instance.created(id);
                        }

                        NewSessionStatus::Exists(id) => {
                            let session = create_new_session(state, id);
                            state.new_session(session, reason);
                            instance.restored();
                        }

                        NewSessionStatus::Replaced(id) => {
                            let session = create_new_session(state, id);
                            state.replace_session(session);
                            instance.restored();
                        }
                    }
                }
            }

            Request::Destroy => {}
        }
    }

    fn destroyed(state: &mut H, _client: ClientId, resource: &XdgSessionManagerV1, _data: &()) {
        state
            .session_management_state()
            .manager_instances
            .retain(|instance| instance != resource);
    }
}

impl<H: SessionManagementHandler> Dispatch<XdgSessionV1, (), H> for SessionManagementState {
    fn request(
        state: &mut H,
        _client: &Client,
        resource: &XdgSessionV1,
        request: <XdgSessionV1 as Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, H>,
    ) {
        use proto::xdg_session_v1::{Error, Request};

        match request {
            Request::AddToplevel { id, toplevel, name } => {
                let instance = data_init.init(id, ());

                if let Some(session) = state.session_management_state().sessions.get(resource).cloned() {
                    let mut inner = session.inner.0.lock().unwrap();
                    if inner.toplevels.values().any(|t| t.inner.0.lock().unwrap().name == name) {
                        resource.post_error(Error::NameInUse, format!("toplevel name '{name}' is already used"));
                    } else if inner.toplevels.values().any(|t| t.inner.0.lock().unwrap().toplevel == toplevel) {
                        resource.post_error(Error::NameInUse, "toplevel is already in session");
                    } else {
                        let toplevel = ToplevelSession {
                            inner: Arc::new((
                                Mutex::new(ToplevelSessionInner {
                                    instance: instance.clone(),
                                    toplevel,
                                    name: name.clone(),
                                }),
                                UserDataMap::new(),
                            )),
                        };
                        inner.toplevel_instances.insert(name, instance.clone());
                        inner.toplevels.insert(instance, toplevel.clone());
                        drop(inner);

                        state.add_toplevel(session, toplevel);
                    }
                }
            }

            Request::RestoreToplevel { id, toplevel, name } => {
                let instance = data_init.init(id, ());

                if let Some(session) = state.session_management_state().sessions.get(resource).cloned() {
                    let mut inner = session.inner.0.lock().unwrap();
                    if inner.toplevels.values().any(|t| t.inner.0.lock().unwrap().name == name) {
                        resource.post_error(Error::NameInUse, format!("toplevel name '{name}' is already used"));
                    } else if inner.toplevels.values().any(|t| t.inner.0.lock().unwrap().toplevel == toplevel) {
                        resource.post_error(Error::NameInUse, "toplevel is already in session");
                    } else {
                        let toplevel = ToplevelSession {
                            inner: Arc::new((
                                Mutex::new(ToplevelSessionInner {
                                    instance: instance.clone(),
                                    toplevel,
                                    name: name.clone(),
                                }),
                                UserDataMap::new(),
                            )),
                        };
                        inner.toplevel_instances.insert(name, instance.clone());
                        inner.toplevels.insert(instance.clone(), toplevel.clone());
                        drop(inner);

                        if state.restore_toplevel(session, toplevel) {
                            instance.restored();
                        }
                    }
                }
            }

            Request::RemoveToplevel { name } => {
                let session_and_toplevel = state.session_management_state().sessions.get(resource).and_then(|session| {
                    let mut inner = session.inner.0.lock().unwrap();
                    inner
                        .toplevel_instances
                        .remove(&name)
                        .and_then(|instance| inner.toplevels.remove(&instance))
                        .map(|toplevel| (session.clone(), toplevel))
                });

                if let Some((session, toplevel)) = session_and_toplevel {
                    state.remove_toplevel(session, toplevel);
                }
            }

            Request::Remove => {
                if let Some(session) = state.session_management_state().sessions.remove(resource) {
                    state.remove_session(session);
                }
            }

            Request::Destroy => {}
        }
    }

    fn destroyed(state: &mut H, _client: ClientId, resource: &XdgSessionV1, _data: &()) {
        state.session_management_state().sessions.remove(resource);
    }
}

impl<H: SessionManagementHandler> Dispatch<XdgToplevelSessionV1, (), H> for SessionManagementState {
    fn request(
        state: &mut H,
        _client: &Client,
        resource: &XdgToplevelSessionV1,
        request: <XdgToplevelSessionV1 as Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, H>,
    ) {
        use proto::xdg_toplevel_session_v1::Request;

        match request {
            Request::Rename { name } => {
                if let Some((session, toplevel)) = state.session_management_state().sessions.values().find_map(|session| {
                    session
                        .inner
                        .0
                        .lock()
                        .unwrap()
                        .toplevels
                        .get(resource)
                        .map(|toplevel| (session.clone(), toplevel.clone()))
                }) {
                    let mut session_inner = session.inner.0.lock().unwrap();
                    if session_inner.toplevel_instances.contains_key(&name) {
                        session_inner.instance.post_error(
                            proto::xdg_session_v1::Error::NameInUse,
                            format!("name '{name}' is in use for another toplevel"),
                        );
                    } else {
                        let mut toplevel_inner = toplevel.inner.0.lock().unwrap();

                        if let Some(instance) = session_inner.toplevel_instances.remove(&toplevel_inner.name) {
                            session_inner.toplevel_instances.insert(name.clone(), instance);
                        }
                        drop(session_inner);

                        toplevel_inner.name = name;
                        drop(toplevel_inner);

                        state.rename_toplevel(session, toplevel);
                    }
                }
            }

            Request::Destroy => {}
        }
    }

    fn destroyed(state: &mut H, _client: ClientId, resource: &XdgToplevelSessionV1, _data: &()) {
        for session in state.session_management_state().sessions.values() {
            let mut inner = session.inner.0.lock().unwrap();
            if let Some(toplevel) = inner.toplevels.remove(resource) {
                inner.toplevel_instances.remove(&toplevel.inner.0.lock().unwrap().name);
                break;
            }
        }
    }
}

macro_rules! delegate_session_management {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::protocols::xdg_session_management::proto::xdg_session_manager_v1::XdgSessionManagerV1: ()
        ] => $crate::protocols::xdg_session_management::SessionManagementState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::protocols::xdg_session_management::proto::xdg_session_manager_v1::XdgSessionManagerV1: ()
        ] => $crate::protocols::xdg_session_management::SessionManagementState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::protocols::xdg_session_management::proto::xdg_session_v1::XdgSessionV1: ()
        ] => $crate::protocols::xdg_session_management::SessionManagementState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::protocols::xdg_session_management::proto::xdg_toplevel_session_v1::XdgToplevelSessionV1: ()
        ] => $crate::protocols::xdg_session_management::SessionManagementState);
    };
}

pub(crate) use delegate_session_management;

pub mod proto {
    use smithay::reexports::{wayland_protocols::xdg::shell::server::xdg_toplevel, wayland_server};

    pub mod __interfaces {
        use smithay::reexports::{wayland_protocols::xdg::shell::server::__interfaces::*, wayland_server::backend as wayland_backend};

        wayland_scanner::generate_interfaces!("./resources/xdg-session-management-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("./resources/xdg-session-management-v1.xml");
}
