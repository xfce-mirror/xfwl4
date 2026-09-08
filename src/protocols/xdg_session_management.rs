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

use smithay::{
    reexports::wayland_server::{
        Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
        backend::{ClientId, GlobalId},
    },
    utils::user_data::UserDataMap,
    wayland::{
        Dispatch2, GlobalDispatch2,
        shell::xdg::{ToplevelSurface, XdgShellHandler},
    },
};
use uuid::Uuid;

use crate::protocols::{
    GlobalData,
    xdg_session_management::proto::{
        xdg_session_manager_v1::{Reason, XdgSessionManagerV1},
        xdg_session_v1::XdgSessionV1,
        xdg_toplevel_session_v1::XdgToplevelSessionV1,
    },
};

pub struct SessionManagementState {
    dh: DisplayHandle,
    global: Option<GlobalId>,
    manager_instances: Vec<XdgSessionManagerV1>,
    sessions: Arc<Mutex<HashMap<XdgSessionV1, Session>>>,
}

pub trait SessionManagementHandler: 'static {
    fn session_management_state(&mut self) -> &mut SessionManagementState;

    fn new_session(&mut self, session: SessionCreator);
    fn remove_session(&mut self, session: Session);
    fn release_session(&mut self, session: Session);

    fn add_toplevel(&mut self, creator: ToplevelSessionCreator);
    fn restore_toplevel(&mut self, creator: ToplevelSessionCreator);
    fn rename_toplevel(&mut self, renamer: ToplevelSessionRenamer);
    fn remove_toplevel(&mut self, remover: ToplevelSessionRemover);
}

#[derive(Debug)]
pub struct SessionCreator {
    instance: XdgSessionV1,
    manager_instance: XdgSessionManagerV1,
    will_replace: Option<XdgSessionV1>,
    client: Client,
    id: String,
    reason: Reason,
    session: Session,
    sessions: Arc<Mutex<HashMap<XdgSessionV1, Session>>>,
    handled: bool,
}

#[derive(Debug)]
struct SessionInner {
    instance: XdgSessionV1,
    id: String,
    reason: Reason,
    toplevels: HashMap<XdgToplevelSessionV1, ToplevelSession>,
    toplevel_instances: HashMap<String, XdgToplevelSessionV1>,
}

#[derive(Debug, Clone)]
pub struct Session {
    inner: Arc<(Mutex<SessionInner>, UserDataMap)>,
}

#[derive(Debug, Clone)]
pub struct ToplevelSessionData(XdgSessionV1);

#[derive(Debug)]
pub struct ToplevelSessionCreator {
    instance: XdgToplevelSessionV1,
    surface: ToplevelSurface,
    name: String,
    session: Session,
    handled: bool,
}

#[derive(Debug)]
pub struct ToplevelSessionRenamer {
    instance: XdgToplevelSessionV1,
    toplevel: ToplevelSession,
    new_name: String,
    session: Session,
    handled: bool,
}

#[derive(Debug)]
pub struct ToplevelSessionRemover {
    name: String,
    surface: Option<ToplevelSurface>,
    session: Session,
}

#[derive(Debug)]
struct ToplevelSessionInner {
    instance: XdgToplevelSessionV1,
    surface: ToplevelSurface,
    name: String,
}

#[derive(Debug, Clone)]
pub struct ToplevelSession {
    inner: Arc<(Mutex<ToplevelSessionInner>, UserDataMap)>,
}

impl SessionManagementState {
    /// Creates a new [`SessionManagementState`] instance
    ///
    /// Note that unlike most Wayland protocol state structs, this does *not* create the associated
    /// `xdg_session_manager_v1` global.  You need to call [`SessionManagementState::enable()`] for
    /// that.
    pub fn new<H: SessionManagementHandler + GlobalDispatch<XdgSessionManagerV1, GlobalData>>(dh: &DisplayHandle) -> Self {
        Self {
            dh: dh.clone(),
            global: None,
            manager_instances: Vec::new(),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn enable<H: GlobalDispatch<XdgSessionManagerV1, GlobalData> + 'static>(&mut self) {
        if self.global.is_none() {
            self.global = Some(self.dh.create_global::<H, XdgSessionManagerV1, _>(1, GlobalData));
        }
    }

    pub fn disable<H: GlobalDispatch<XdgSessionManagerV1, GlobalData> + 'static>(&mut self) {
        if let Some(global) = &self.global {
            self.dh.disable_global::<H>(global.clone());
        }
    }

    pub fn remove<H: GlobalDispatch<XdgSessionManagerV1, GlobalData> + 'static>(&mut self) {
        if let Some(global) = self.global.take() {
            self.dh.remove_global::<H>(global);
        }
    }

    pub fn sessions_for_client(&self, client: &Client) -> Vec<Session> {
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|(instance, _)| instance.client().is_some_and(|session_client| session_client == *client))
            .map(|(_, session)| session.clone())
            .collect()
    }
}

impl SessionCreator {
    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn reason(&self) -> Reason {
        self.reason
    }

    pub fn user_data(&self) -> &UserDataMap {
        self.session.user_data()
    }

    fn create_or_restore(mut self, new_id: Option<&str>, final_reason: Reason, is_restore: bool) -> Session {
        if is_restore {
            self.instance.restored();
        } else {
            let id = new_id.unwrap_or(&self.id);
            self.instance.created(id.to_owned());
        }

        let mut session = self.session.inner.0.lock().unwrap();
        session.reason = final_reason;

        if let Some(new_id) = new_id
            && self.id != new_id
        {
            session.id = new_id.to_owned();
        } else if let Some(replaced_instance) = &self.will_replace
            && self.sessions.lock().unwrap().remove(replaced_instance).is_some()
        {
            replaced_instance.replaced();
        }

        self.handled = true;

        self.session.clone()
    }

    pub fn created(self, updated_id: Option<&str>, final_reason: Reason) -> Session {
        self.create_or_restore(updated_id, final_reason, false)
    }

    pub fn restored(self, final_reason: Reason) -> Session {
        self.create_or_restore(None, final_reason, true)
    }

    pub fn id_in_use(mut self) {
        use proto::xdg_session_manager_v1::Error;
        self.manager_instance
            .post_error(Error::InUse, format!("session with id '{}' is already in use", self.id));
        self.handled = true;
    }
}

impl Drop for SessionCreator {
    fn drop(&mut self) {
        if !self.handled {
            self.sessions.lock().unwrap().remove(&self.instance);
            self.handled = true;
        }
    }
}

impl Session {
    pub fn id(&self) -> String {
        self.inner.0.lock().unwrap().id.clone()
    }

    pub fn reason(&self) -> Reason {
        self.inner.0.lock().unwrap().reason
    }

    pub fn xdg_session(&self) -> XdgSessionV1 {
        self.inner.0.lock().unwrap().instance.clone()
    }

    pub fn toplevels(&self) -> Vec<ToplevelSession> {
        self.inner.0.lock().unwrap().toplevels.values().cloned().collect()
    }

    pub fn user_data(&self) -> &UserDataMap {
        &self.inner.1
    }
}

impl ToplevelSessionCreator {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn surface(&self) -> &ToplevelSurface {
        &self.surface
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn name_in_use(mut self) {
        use proto::xdg_session_v1::Error;

        let mut inner = self.session.inner.0.lock().unwrap();
        inner.toplevels.remove(&self.instance);
        inner.toplevel_instances.remove(&self.name);
        inner
            .instance
            .post_error(Error::NameInUse, format!("toplevel name {} is already in use", self.name));

        self.handled = true;
    }

    pub fn added(mut self) {
        self.handled = true;
    }

    pub fn restored(mut self) {
        self.instance.restored();
        self.handled = true;
    }
}

impl Drop for ToplevelSessionCreator {
    fn drop(&mut self) {
        if !self.handled {
            let mut inner = self.session.inner.0.lock().unwrap();
            inner.toplevels.remove(&self.instance);
            inner.toplevel_instances.remove(&self.name);
            self.handled = true;
        }
    }
}

impl ToplevelSessionRenamer {
    pub fn old_name(&self) -> String {
        self.toplevel.inner.0.lock().unwrap().name.clone()
    }

    pub fn new_name(&self) -> &str {
        &self.new_name
    }

    pub fn surface(&self) -> ToplevelSurface {
        self.toplevel.inner.0.lock().unwrap().surface.clone()
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn name_in_use(mut self) {
        use proto::xdg_session_v1::Error;

        let mut inner = self.session.inner.0.lock().unwrap();
        inner.toplevels.remove(&self.instance);
        inner.toplevel_instances.remove(&self.new_name);
        inner
            .instance
            .post_error(Error::NameInUse, format!("toplevel name {} is already in use", self.new_name));

        self.handled = true;
    }

    pub fn renamed(mut self) {
        self.toplevel.inner.0.lock().unwrap().name = self.new_name.clone();
        self.handled = true;
    }
}

impl Drop for ToplevelSessionRenamer {
    fn drop(&mut self) {
        if !self.handled {
            let mut inner = self.session.inner.0.lock().unwrap();
            inner.toplevel_instances.remove(&self.new_name);
            let old_name = self.toplevel.inner.0.lock().unwrap().name.clone();
            inner.toplevel_instances.insert(old_name, self.instance.clone());
        }
    }
}

impl ToplevelSessionRemover {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn surface(&self) -> Option<&ToplevelSurface> {
        self.surface.as_ref()
    }

    pub fn session(&self) -> &Session {
        &self.session
    }
}

impl ToplevelSession {
    pub fn name(&self) -> String {
        self.inner.0.lock().unwrap().name.clone()
    }

    pub fn surface(&self) -> ToplevelSurface {
        self.inner.0.lock().unwrap().surface.clone()
    }

    pub fn user_data(&self) -> &UserDataMap {
        &self.inner.1
    }
}

impl<H> GlobalDispatch2<XdgSessionManagerV1, H> for GlobalData
where
    H: SessionManagementHandler + Dispatch<XdgSessionManagerV1, GlobalData>,
{
    fn bind(
        &self,
        state: &mut H,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<XdgSessionManagerV1>,
        data_init: &mut DataInit<'_, H>,
    ) {
        let instance = data_init.init(resource, GlobalData);
        state.session_management_state().manager_instances.push(instance);
    }
}

impl<H> Dispatch2<XdgSessionManagerV1, H> for GlobalData
where
    H: SessionManagementHandler + Dispatch<XdgSessionV1, GlobalData>,
{
    fn request(
        &self,
        state: &mut H,
        client: &Client,
        resource: &XdgSessionManagerV1,
        request: <XdgSessionManagerV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, H>,
    ) {
        use proto::xdg_session_manager_v1::{Error, Request};

        match request {
            Request::GetSession { id, reason, session_id } => {
                let instance = data_init.init(id, GlobalData);

                match reason {
                    WEnum::Value(reason) => {
                        let (new_session_id, replaced_session) = if let Some(existing_session_id) = session_id {
                            if existing_session_id.trim().is_empty() {
                                resource.post_error(Error::InvalidSessionId, "session ID cannot be an empty string");
                                (None, None)
                            } else if let Some(existing) =
                                state
                                    .session_management_state()
                                    .sessions
                                    .lock()
                                    .unwrap()
                                    .iter()
                                    .find_map(|(instance, session)| {
                                        (session.inner.0.lock().unwrap().id == existing_session_id).then_some(instance)
                                    })
                            {
                                if existing.client().is_some_and(|existing_client| existing_client == *client) {
                                    resource.post_error(Error::InUse, format!("session with id '{existing_session_id}' is already in use"));
                                    (None, None)
                                } else {
                                    (
                                        Some(existing_session_id),
                                        Some(existing).filter(|existing| existing.is_alive()).cloned(),
                                    )
                                }
                            } else {
                                (Some(existing_session_id), None)
                            }
                        } else {
                            (Some(Uuid::new_v4().to_string()), None)
                        };

                        if let Some(id) = new_session_id {
                            let session = Session {
                                inner: Arc::new((
                                    Mutex::new(SessionInner {
                                        instance: instance.clone(),
                                        id: id.clone(),
                                        reason,
                                        toplevels: HashMap::new(),
                                        toplevel_instances: HashMap::new(),
                                    }),
                                    UserDataMap::new(),
                                )),
                            };
                            let sessions = Arc::clone(&state.session_management_state().sessions);
                            sessions.lock().unwrap().insert(instance.clone(), session.clone());

                            let creator = SessionCreator {
                                id,
                                instance,
                                manager_instance: resource.clone(),
                                will_replace: replaced_session,
                                client: client.clone(),
                                reason,
                                session,
                                sessions,
                                handled: false,
                            };
                            state.new_session(creator);
                        }
                    }

                    WEnum::Unknown(value) => resource.post_error(Error::InvalidReason, format!("reason {value} is not valid")),
                }
            }

            Request::Destroy => {}
        }
    }

    fn destroyed(&self, state: &mut H, _client: ClientId, resource: &XdgSessionManagerV1) {
        state
            .session_management_state()
            .manager_instances
            .retain(|instance| instance != resource);
    }
}

impl<H> Dispatch2<XdgSessionV1, H> for GlobalData
where
    H: SessionManagementHandler + XdgShellHandler + Dispatch<XdgToplevelSessionV1, ToplevelSessionData>,
{
    fn request(
        &self,
        state: &mut H,
        _client: &Client,
        resource: &XdgSessionV1,
        request: <XdgSessionV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, H>,
    ) {
        use proto::xdg_session_v1::{Error, Request};

        let register_toplevel =
            |inner: &mut SessionInner, session: Session, instance: XdgToplevelSessionV1, surface: ToplevelSurface, name: String| {
                let toplevel = ToplevelSession {
                    inner: Arc::new((
                        Mutex::new(ToplevelSessionInner {
                            instance: instance.clone(),
                            surface: surface.clone(),
                            name: name.clone(),
                        }),
                        UserDataMap::new(),
                    )),
                };
                inner.toplevel_instances.insert(name.clone(), instance.clone());
                inner.toplevels.insert(instance.clone(), toplevel.clone());

                ToplevelSessionCreator {
                    instance,
                    surface,
                    name,
                    session,
                    handled: false,
                }
            };

        match request {
            Request::AddToplevel { id, toplevel, name } => {
                let instance = data_init.init(id, ToplevelSessionData(resource.clone()));

                if name.trim().is_empty() {
                    resource.post_error(Error::InvalidName, "toplevel name cannot be empty");
                } else if let Some(session) = { state.session_management_state().sessions.lock().unwrap().get(resource).cloned() } {
                    let mut inner = session.inner.0.lock().unwrap();
                    if inner.toplevels.values().any(|t| t.inner.0.lock().unwrap().name == name) {
                        resource.post_error(Error::NameInUse, format!("toplevel name '{name}' is already used"));
                    } else if inner
                        .toplevels
                        .values()
                        .any(|t| t.inner.0.lock().unwrap().surface.xdg_toplevel() == &toplevel)
                    {
                        resource.post_error(Error::AlreadyAdded, "toplevel is already in session");
                    } else if let Some(surface) = state.xdg_shell_state().get_toplevel(&toplevel) {
                        let creator = register_toplevel(&mut inner, session.clone(), instance, surface, name);
                        drop(inner);
                        state.add_toplevel(creator);
                    } else {
                        tracing::warn!("No ToplevelSurface found for XdgToplevelV1; client will be confused");
                    }
                }
            }

            Request::RestoreToplevel { id, toplevel, name } => {
                let instance = data_init.init(id, ToplevelSessionData(resource.clone()));

                if name.trim().is_empty() {
                    resource.post_error(Error::InvalidName, "toplevel name cannot be empty");
                } else if let Some(session) = { state.session_management_state().sessions.lock().unwrap().get(resource).cloned() } {
                    let mut inner = session.inner.0.lock().unwrap();
                    if inner.toplevels.values().any(|t| t.inner.0.lock().unwrap().name == name) {
                        resource.post_error(Error::NameInUse, format!("toplevel name '{name}' is already used"));
                    } else if inner
                        .toplevels
                        .values()
                        .any(|t| t.inner.0.lock().unwrap().surface.xdg_toplevel() == &toplevel)
                    {
                        resource.post_error(Error::AlreadyAdded, "toplevel is already in session");
                    } else if let Some(surface) = state.xdg_shell_state().get_toplevel(&toplevel) {
                        if surface.with_committed_state(|state| state.is_some()) {
                            resource.post_error(Error::AlreadyMapped, "toplevel is already mapped");
                        } else {
                            let creator = register_toplevel(&mut inner, session.clone(), instance, surface, name);
                            drop(inner);
                            state.restore_toplevel(creator);
                        }
                    }
                }
            }

            Request::RemoveToplevel { name } => {
                if name.trim().is_empty() {
                    resource.post_error(Error::InvalidName, "toplevel name cannot be empty");
                } else if let Some(session) = { state.session_management_state().sessions.lock().unwrap().get(resource).cloned() } {
                    let mut inner = session.inner.0.lock().unwrap();
                    let surface = if let Some(toplevel) = inner.toplevel_instances.remove(&name) {
                        inner
                            .toplevels
                            .remove(&toplevel)
                            .map(|toplevel| toplevel.inner.0.lock().unwrap().surface.clone())
                    } else {
                        None
                    };
                    drop(inner);

                    let remover = ToplevelSessionRemover { name, surface, session };
                    state.remove_toplevel(remover);
                }
            }

            Request::Remove => {
                if let Some(session) = { state.session_management_state().sessions.lock().unwrap().remove(resource) } {
                    state.remove_session(session);
                }
            }

            Request::Destroy => (),
        }
    }

    fn destroyed(&self, state: &mut H, _client: ClientId, resource: &XdgSessionV1) {
        if let Some(session) = { state.session_management_state().sessions.lock().unwrap().remove(resource) } {
            state.release_session(session);
        }
    }
}

impl<H: SessionManagementHandler> Dispatch2<XdgToplevelSessionV1, H> for ToplevelSessionData {
    fn request(
        &self,
        state: &mut H,
        _client: &Client,
        resource: &XdgToplevelSessionV1,
        request: <XdgToplevelSessionV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, H>,
    ) {
        use proto::{xdg_session_v1::Error, xdg_toplevel_session_v1::Request};

        match request {
            Request::Rename { name } => {
                if name.trim().is_empty() {
                    resource.post_error(Error::InvalidName, "toplevel name cannot be empty");
                } else if let Some(session) = { state.session_management_state().sessions.lock().unwrap().get(&self.0).cloned() } {
                    let mut inner = session.inner.0.lock().unwrap();

                    if inner.toplevel_instances.contains_key(&name) {
                        inner
                            .instance
                            .post_error(Error::NameInUse, format!("toplevel name '{name}' is already used"));
                    } else if let Some(toplevel) = inner.toplevels.get(resource).cloned() {
                        let (old_name, instance) = {
                            let inner = toplevel.inner.0.lock().unwrap();
                            (inner.name.clone(), inner.instance.clone())
                        };

                        inner.toplevel_instances.remove(&old_name);
                        inner.toplevel_instances.insert(name.clone(), instance.clone());
                        drop(inner);

                        let renamer = ToplevelSessionRenamer {
                            new_name: name,
                            instance,
                            toplevel,
                            session,
                            handled: false,
                        };
                        state.rename_toplevel(renamer);
                    }
                }
            }

            Request::Destroy => {}
        }
    }

    fn destroyed(&self, state: &mut H, _client: ClientId, resource: &XdgToplevelSessionV1) {
        if let Some(session) = state.session_management_state().sessions.lock().unwrap().get(&self.0) {
            let mut inner = session.inner.0.lock().unwrap();
            if let Some(toplevel) = inner.toplevels.remove(resource) {
                inner.toplevel_instances.remove(&toplevel.inner.0.lock().unwrap().name);
            }
        }
    }
}

pub mod proto {
    use smithay::reexports::{wayland_protocols::xdg::shell::server::xdg_toplevel, wayland_server};
    use std::fmt;

    pub mod __interfaces {
        use smithay::reexports::{wayland_protocols::xdg::shell::server::__interfaces::*, wayland_server::backend as wayland_backend};

        wayland_scanner::generate_interfaces!("./resources/xdg-session-management-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("./resources/xdg-session-management-v1.xml");

    impl xdg_session_manager_v1::Reason {
        pub fn as_str(&self) -> &'static str {
            match self {
                Self::Launch => "launch",
                Self::Recover => "recover",
                Self::SessionRestore => "session_restore",
            }
        }
    }

    impl fmt::Display for xdg_session_manager_v1::Reason {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.as_str())
        }
    }

    impl TryFrom<String> for xdg_session_manager_v1::Reason {
        type Error = anyhow::Error;

        fn try_from(value: String) -> Result<Self, Self::Error> {
            match value.as_str() {
                "launch" => Ok(Self::Launch),
                "recover" => Ok(Self::Recover),
                "session_restore" => Ok(Self::SessionRestore),
                other => Err(anyhow::anyhow!("Unknown xdg_session_manager_v1.reason value \"{other}\"")),
            }
        }
    }
}
