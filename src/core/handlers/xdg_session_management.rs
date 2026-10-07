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
    cell::RefCell,
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::anyhow;
use calloop::{
    LoopHandle, RegistrationToken,
    channel::Event,
    futures::{Scheduler, executor},
    timer::{TimeoutAction, Timer},
};
use glib::variant::{ObjectPath, ToVariant};
use smithay::{
    desktop::space::SpaceElement,
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel,
        wayland_server::{Client, DisplayHandle, Resource},
    },
    utils::{Logical, Rectangle},
    wayland::{compositor, shell::xdg::ToplevelSurface},
};

use crate::{
    backend::Backend,
    core::{
        shell::{WindowElement, WindowState, WorkspaceLocation, xdg::app_id_for_xdg_toplevel},
        state::{Xfwl4Core, Xfwl4State},
        util::{SessionError, SessionEvent, SessionStatus, XfsmClient},
    },
    protocols::xdg_session_management::{
        Session, SessionCreator, SessionManagementHandler, SessionManagementState, ToplevelSessionCreator, ToplevelSessionRemover,
        ToplevelSessionRenamer, proto::xdg_session_manager_v1::Reason,
    },
    util::OutputExt,
};

const SESSION_TOPLEVEL_SYNC_TIMEOUT: Duration = Duration::from_secs(2);

pub struct SessionState {
    session_management_state: SessionManagementState,
    xfsm_client: XfsmClient,
    disable_timeout: Option<RegistrationToken>,

    session_scheduler: Scheduler<()>,
    logout_in_progress: bool,
    session_delegate_scheduler: Scheduler<SessionFutureResult>,

    toplevel_restore_state: HashMap<XdgToplevel, ToplevelRestore>,
    toplevel_sync_timeout_token: Option<RegistrationToken>,
}

#[derive(Debug)]
enum PendingSmAction {
    AddToplevel(ToplevelSessionCreator),
    RestoreToplevel(ToplevelSessionCreator),
    RenameToplevel(ToplevelSessionRenamer),
    RemoveToplevel(ToplevelSessionRemover),
    RemoveSession,
    ReleaseSession,
}

#[derive(Debug)]
enum SessionRegistrationInner {
    Registering(Vec<PendingSmAction>),
    Registered(ObjectPath),
    RegistrationFailed,
}

#[derive(Debug)]
struct SessionRegistration(RefCell<SessionRegistrationInner>);

struct ToplevelSessionMetadataInner {
    client_object_path: ObjectPath,
    toplevel_id: String,
}

struct ToplevelSessionMetadata(RefCell<Option<ToplevelSessionMetadataInner>>);

#[allow(clippy::large_enum_variant)]
enum ToplevelRestore {
    Pending { committed: bool },
    Ready(ToplevelRestoreState),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ToplevelWmPropertyNames {
    Geometry,
    SavedGeometry,
    WorkspaceId,
    OutputEdid,
    States,
    StackingSerials,
}

#[derive(Debug)]
pub struct ToplevelRestoreState {
    pub wm_properties: ToplevelWmProperties,
    creator: ToplevelSessionCreator,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToplevelWmProperties {
    pub geometry: Rectangle<i32, Logical>,
    pub saved_geometry: Option<Rectangle<i32, Logical>>,
    pub workspace_id: Option<u64>,
    pub output_edid: Option<String>,
    pub states: WindowState,
    pub stacking_serials: HashMap<u64, u64>, // workspace ID -> stacking serial
}

#[derive(Debug, Default)]
struct ToplevelSessionStateDirty(AtomicBool);

#[allow(private_interfaces)]
enum SessionFutureResult {
    RegisterClientComplete(Result<RegisterClientComplete, SessionFutureError<SessionCreator>>),
    RegisterToplevelComplete(Result<ToplevelSessionCreator, SessionFutureError<ToplevelSessionCreator>>),
    RestoreToplevelComplete(Result<(ToplevelSessionCreator, Option<ToplevelWmProperties>), SessionFutureError<ToplevelSessionCreator>>),
    RenameToplevelComplete(Result<ToplevelSessionRenamer, SessionFutureError<ToplevelSessionRenamer>>),
    None,
}

struct RegisterClientComplete {
    pub creator: SessionCreator,
    pub object_path: ObjectPath,
    pub reason: Reason,
    pub status: SessionStatus,
}

enum SessionFutureError<T> {
    NameConflict(T),
    SmUnavailable(T),
    Other(T, anyhow::Error),
}

impl SessionState {
    pub fn new<BackendData: Backend + 'static>(
        dh: &DisplayHandle,
        handle: LoopHandle<'_, Xfwl4State<BackendData>>,
    ) -> anyhow::Result<Self> {
        let (sm_status_notifier, xfsm_client) = XfsmClient::new()?;
        handle
            .insert_source(sm_status_notifier, |event, _, state| {
                if let Event::Msg(event) = event {
                    match event {
                        SessionEvent::AvailabilityChanged(xfsm_available) => {
                            if xfsm_available {
                                if let Some(token) = state.core.protocol_delegates.session_state.disable_timeout.take() {
                                    state.core.unregister_timer(token);
                                }

                                state
                                    .core
                                    .protocol_delegates
                                    .session_state
                                    .session_management_state
                                    .enable::<Xfwl4State<BackendData>>();
                            } else if state.core.protocol_delegates.session_state.disable_timeout.is_none() {
                                // If we just outright remove the global, and there's a client with an
                                // in-flight request to bind to it, the client will get killed.  Instead,
                                // disable it, and remove it a second later.

                                state
                                    .core
                                    .protocol_delegates
                                    .session_state
                                    .session_management_state
                                    .disable::<Xfwl4State<BackendData>>();

                                let token = state.core.register_timer(Timer::from_duration(Duration::from_secs(1)), |state| {
                                    let session_state = &mut state.core.protocol_delegates.session_state;
                                    session_state.disable_timeout = None;
                                    session_state.session_management_state.remove::<Xfwl4State<BackendData>>();
                                    TimeoutAction::Drop
                                });
                                state.core.protocol_delegates.session_state.disable_timeout = Some(token);
                            }
                        }

                        SessionEvent::Checkpoint | SessionEvent::ShutdownStarted => {
                            if let Some(token) = state.core.protocol_delegates.session_state.toplevel_sync_timeout_token.take() {
                                state.core.unregister_timer(token);
                            }
                            state.core.session_sync_toplevel_wm_properties();
                        }
                    }
                }
            })
            .expect("failed to insert XfsmClient notifier source");

        let (session_executor, session_scheduler) = executor()?;
        handle
            .insert_source(session_executor, |_, _, state| {
                state.core.protocol_delegates.session_state.logout_in_progress = false;
            })
            .map_err(|err| anyhow!("Failed to register future executor source: {err}"))?;

        let (session_delegate_executor, session_delegate_scheduler) = executor()?;
        handle
            .insert_source(session_delegate_executor, |action, _, state| {
                state.handle_session_future_completion(action);
            })
            .map_err(|err| anyhow!("Failed to register future executor source: {err}"))?;

        let session_management_state = SessionManagementState::new::<Xfwl4State<BackendData>>(dh);

        Ok(Self {
            session_management_state,
            xfsm_client,
            disable_timeout: None,
            session_scheduler,
            logout_in_progress: false,
            session_delegate_scheduler,
            toplevel_restore_state: HashMap::new(),
            toplevel_sync_timeout_token: None,
        })
    }

    pub fn request_logout(&mut self) {
        if !self.logout_in_progress {
            self.logout_in_progress = true;
            let xfsm_client = self.xfsm_client.clone();
            let _ = self.session_scheduler.schedule(async move {
                if let Err(err) = xfsm_client.request_logout().await {
                    tracing::warn!("Failed to request logout from session manager: {err}");
                }
            });
        }
    }

    fn do_set_app_id(&self, object_path: ObjectPath, app_id: &str) {
        let xfsm_client = self.xfsm_client.clone();
        let app_id = app_id.to_owned();
        let _ = self.session_delegate_scheduler.schedule(async move {
            match xfsm_client.set_app_id(&object_path, &app_id).await {
                Ok(_) | Err(SessionError::SmUnavailable) => (),
                Err(err) => tracing::warn!("Failed to tell session manager app ID for client with path {object_path}: {err}"),
            }
            SessionFutureResult::None
        });
    }

    fn do_remove_session(&self, session: Session, object_path: ObjectPath) {
        for toplevel in session.toplevels() {
            ToplevelSessionMetadata::clear(&toplevel.surface());
        }

        let xfsm_client = self.xfsm_client.clone();
        let _ = self.session_delegate_scheduler.schedule(async move {
            match xfsm_client.remove_client(&object_path).await {
                Ok(_) | Err(SessionError::SmUnavailable) => (),
                Err(err) => tracing::warn!("Failed to remove session manager client with path {object_path}: {err}"),
            }
            SessionFutureResult::None
        });
    }

    fn do_release_session(&self, session: Session, object_path: ObjectPath) {
        for toplevel in session.toplevels() {
            ToplevelSessionMetadata::clear(&toplevel.surface());
        }

        let xfsm_client = self.xfsm_client.clone();
        let _ = self.session_delegate_scheduler.schedule(async move {
            match xfsm_client.client_disconnected(&object_path).await {
                Ok(_) | Err(SessionError::SmUnavailable) => (),
                Err(err) => {
                    tracing::warn!("Failed to inform session manager of client disconnection with path {object_path}: {err}")
                }
            }
            SessionFutureResult::None
        });
    }

    fn do_add_toplevel(&self, creator: ToplevelSessionCreator, object_path: ObjectPath) {
        let xfsm_client = self.xfsm_client.clone();
        let _ = self.session_delegate_scheduler.schedule(async move {
            match xfsm_client.register_toplevel(&object_path, creator.name()).await {
                Ok(_) => SessionFutureResult::RegisterToplevelComplete(Ok(creator)),
                Err(err) => SessionFutureResult::RegisterToplevelComplete(Err(SessionFutureError::from_session_error(err, creator))),
            }
        });
    }

    fn do_restore_toplevel(&self, creator: ToplevelSessionCreator, object_path: ObjectPath) {
        let xfsm_client = self.xfsm_client.clone();
        let _ = self.session_delegate_scheduler.schedule(async move {
            match xfsm_client.restore_toplevel(&object_path, creator.name()).await {
                Ok(wm_properties) => {
                    let props = (!wm_properties.is_empty()).then(|| ToplevelWmProperties::from(wm_properties));
                    SessionFutureResult::RestoreToplevelComplete(Ok((creator, props)))
                }
                Err(err) => SessionFutureResult::RestoreToplevelComplete(Err(SessionFutureError::from_session_error(err, creator))),
            }
        });
    }

    fn do_rename_toplevel(&self, renamer: ToplevelSessionRenamer, object_path: ObjectPath) {
        // Be optimistic about it succeeding.  We don't want the core to call
        // update_wm_properties_for_toplevel() while the request is in flight, but before it's
        // completed, using the old name.
        ToplevelSessionMetadata::update(&renamer.surface(), object_path.clone(), renamer.new_name().to_owned());

        let xfsm_client = self.xfsm_client.clone();
        let _ = self.session_delegate_scheduler.schedule(async move {
            match xfsm_client
                .rename_toplevel(&object_path, &renamer.old_name(), renamer.new_name())
                .await
            {
                Ok(_) => SessionFutureResult::RenameToplevelComplete(Ok(renamer)),
                Err(err) => SessionFutureResult::RenameToplevelComplete(Err(SessionFutureError::from_session_error(err, renamer))),
            }
        });
    }

    fn do_remove_toplevel(&self, remover: ToplevelSessionRemover, object_path: ObjectPath) {
        if let Some(surface) = remover.surface() {
            ToplevelSessionMetadata::clear(surface);
        }

        let xfsm_client = self.xfsm_client.clone();
        let _ = self.session_delegate_scheduler.schedule(async move {
            match xfsm_client.remove_toplevel(&object_path, remover.name()).await {
                Ok(_) | Err(SessionError::SmUnavailable) => (),
                Err(err) => tracing::warn!("Failed to remove toplevel from session manager: {err}"),
            }
            SessionFutureResult::None
        });
    }
}

impl<BackendData: Backend + 'static> SessionManagementHandler for Xfwl4State<BackendData> {
    fn session_management_state(&mut self) -> &mut SessionManagementState {
        &mut self.core.protocol_delegates.session_state.session_management_state
    }

    fn new_session(&mut self, creator: SessionCreator) {
        if let Some(pid) = creator
            .client()
            .get_credentials(&self.core.display_handle)
            .ok()
            .filter(|creds| creds.pid > 0)
            .map(|creds| creds.pid as u32)
        {
            SessionRegistration::init(&creator);

            let xfsm_client = self.core.protocol_delegates.session_state.xfsm_client.clone();
            let _ = self
                .core
                .protocol_delegates
                .session_state
                .session_delegate_scheduler
                .schedule(async move {
                    match xfsm_client.register_client(creator.id(), pid, creator.reason()).await {
                        Ok((object_path, reason, status)) => SessionFutureResult::RegisterClientComplete(Ok(RegisterClientComplete {
                            creator,
                            object_path,
                            reason,
                            status,
                        })),
                        Err(err) => SessionFutureResult::RegisterClientComplete(Err(SessionFutureError::from_session_error(err, creator))),
                    }
                });
        } else {
            tracing::info!("Couldn't get PID of session manager client");
            SessionRegistration::failed(self, &creator);
        }
    }

    fn remove_session(&mut self, session: Session) {
        if let Some(object_path) = SessionRegistration::get(&session) {
            self.core.protocol_delegates.session_state.do_remove_session(session, object_path);
        } else {
            SessionRegistration::push_pending(&session, PendingSmAction::RemoveSession);
        }
    }

    fn release_session(&mut self, session: Session) {
        if let Some(object_path) = SessionRegistration::get(&session) {
            self.core.protocol_delegates.session_state.do_release_session(session, object_path);
        } else {
            SessionRegistration::push_pending(&session, PendingSmAction::ReleaseSession);
        }
    }

    fn add_toplevel(&mut self, creator: ToplevelSessionCreator) {
        if let Some(object_path) = SessionRegistration::get(creator.session()) {
            self.core.protocol_delegates.session_state.do_add_toplevel(creator, object_path);
        } else {
            SessionRegistration::push_pending(&creator.session().clone(), PendingSmAction::AddToplevel(creator));
        }
    }

    fn restore_toplevel(&mut self, creator: ToplevelSessionCreator) {
        let mut set_up_pending_and_hooks = || {
            self.core.protocol_delegates.session_state.toplevel_restore_state.insert(
                creator.surface().xdg_toplevel().clone(),
                ToplevelRestore::Pending { committed: false },
            );

            let hook_id_holder = Arc::new(Mutex::new(None));
            let hook_id = {
                let hook_id_holder = Arc::clone(&hook_id_holder);
                compositor::add_post_commit_hook::<Self, _>(creator.surface().wl_surface(), move |state, _, wl_surface| {
                    if let Some((hook_id, xdg_toplevel)) = hook_id_holder.lock().unwrap().take() {
                        compositor::remove_post_commit_hook(wl_surface, &hook_id);

                        if state
                            .core
                            .protocol_delegates
                            .session_state
                            .toplevel_restore_state
                            .get(&xdg_toplevel)
                            .is_some_and(|restore| matches!(restore, ToplevelRestore::Pending { .. }))
                        {
                            state
                                .core
                                .protocol_delegates
                                .session_state
                                .toplevel_restore_state
                                .insert(xdg_toplevel, ToplevelRestore::Pending { committed: true });
                        }
                    }
                })
            };
            hook_id_holder
                .lock()
                .unwrap()
                .replace((hook_id, creator.surface().xdg_toplevel().clone()));

            let xdg_toplevel = creator.surface().xdg_toplevel().clone();
            compositor::add_destruction_hook::<Self, _>(creator.surface().wl_surface(), move |state, _| {
                state
                    .core
                    .protocol_delegates
                    .session_state
                    .toplevel_restore_state
                    .remove(&xdg_toplevel);
            });
        };

        if let Some(object_path) = SessionRegistration::get(creator.session()) {
            set_up_pending_and_hooks();
            self.core.protocol_delegates.session_state.do_restore_toplevel(creator, object_path);
        } else if SessionRegistration::is_registering(creator.session()) {
            set_up_pending_and_hooks();
            SessionRegistration::push_pending(&creator.session().clone(), PendingSmAction::RestoreToplevel(creator));
        }
    }

    fn rename_toplevel(&mut self, renamer: ToplevelSessionRenamer) {
        if let Some(object_path) = SessionRegistration::get(renamer.session()) {
            self.core.protocol_delegates.session_state.do_rename_toplevel(renamer, object_path);
        } else {
            SessionRegistration::push_pending(&renamer.session().clone(), PendingSmAction::RenameToplevel(renamer));
        }
    }

    fn remove_toplevel(&mut self, remover: ToplevelSessionRemover) {
        if let Some(object_path) = SessionRegistration::get(remover.session()) {
            self.core.protocol_delegates.session_state.do_remove_toplevel(remover, object_path);
        } else {
            SessionRegistration::push_pending(&remover.session().clone(), PendingSmAction::RemoveToplevel(remover));
        }
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    fn handle_session_future_completion(&mut self, action: SessionFutureResult) {
        match action {
            SessionFutureResult::RegisterClientComplete(result) => self.handle_register_client_complete(result),
            SessionFutureResult::RegisterToplevelComplete(result) => self.handle_register_toplevel_complete(result),
            SessionFutureResult::RestoreToplevelComplete(result) => self.handle_restore_toplevel_complete(result),
            SessionFutureResult::RenameToplevelComplete(result) => self.handle_rename_toplevel_complete(result),
            SessionFutureResult::None => (),
        }
    }

    fn handle_register_client_complete(&mut self, result: Result<RegisterClientComplete, SessionFutureError<SessionCreator>>) {
        match result {
            Ok(RegisterClientComplete {
                creator,
                object_path,
                reason,
                status,
            }) => {
                let session = match status {
                    SessionStatus::Created => creator.created(None, reason),
                    SessionStatus::Restored => creator.restored(reason),
                };
                let client = session.xdg_session().client();

                let pending_actions = SessionRegistration::registered(&session, object_path.clone());
                let result = pending_actions.into_iter().try_fold((), |_, action| {
                    let session_dropped = matches!(action, PendingSmAction::RemoveSession | PendingSmAction::ReleaseSession);
                    let session_state = &self.core.protocol_delegates.session_state;

                    match action {
                        PendingSmAction::AddToplevel(creator) => session_state.do_add_toplevel(creator, object_path.clone()),
                        PendingSmAction::RestoreToplevel(creator) => session_state.do_restore_toplevel(creator, object_path.clone()),
                        PendingSmAction::RenameToplevel(renamer) => session_state.do_rename_toplevel(renamer, object_path.clone()),
                        PendingSmAction::RemoveToplevel(remover) => session_state.do_remove_toplevel(remover, object_path.clone()),
                        PendingSmAction::RemoveSession => session_state.do_remove_session(session.clone(), object_path.clone()),
                        PendingSmAction::ReleaseSession => session_state.do_release_session(session.clone(), object_path.clone()),
                    }

                    if session_dropped { Err(()) } else { Ok(()) }
                });

                if result.is_ok()
                    && let Some(client) = client
                {
                    self.core.set_session_app_id_for_client(client, object_path);
                }
            }
            Err(SessionFutureError::NameConflict(creator)) => {
                SessionRegistration::failed(self, &creator);
                creator.id_in_use();
            }
            Err(SessionFutureError::SmUnavailable(creator)) => SessionRegistration::failed(self, &creator),
            Err(SessionFutureError::Other(creator, err)) => {
                tracing::warn!("Failed to register client with session manager: {err}");
                SessionRegistration::failed(self, &creator);
            }
        }
    }

    fn handle_register_toplevel_complete(&mut self, result: Result<ToplevelSessionCreator, SessionFutureError<ToplevelSessionCreator>>) {
        match result {
            Ok(creator) => {
                if let Some(object_path) = SessionRegistration::get(creator.session()) {
                    ToplevelSessionMetadata::update(creator.surface(), object_path, creator.name().to_owned());

                    if let Some(window) = self.window_for_surface(creator.surface().wl_surface()) {
                        self.core.queue_window_session_sync(&window);
                    }
                }
                creator.added();
            }
            Err(SessionFutureError::NameConflict(creator)) => creator.name_in_use(),
            Err(SessionFutureError::SmUnavailable(_)) => (),
            Err(SessionFutureError::Other(_, err)) => tracing::info!("Failed to register toplevel: {err}"),
        }
    }

    fn handle_restore_toplevel_complete(
        &mut self,
        result: Result<(ToplevelSessionCreator, Option<ToplevelWmProperties>), SessionFutureError<ToplevelSessionCreator>>,
    ) {
        match result {
            Ok((creator, wm_properties)) => {
                if let Some(object_path) = SessionRegistration::get(creator.session()) {
                    ToplevelSessionMetadata::update(creator.surface(), object_path, creator.name().to_owned());
                }

                let surface = creator.surface().clone();
                if let Some(wm_properties) = wm_properties {
                    // We don't call creator.restored() here because that needs to be delayed until
                    // the window's state actually has been restored, which will get handled in the
                    // xdg-shell code.
                    let xdg_toplevel = surface.xdg_toplevel().clone();
                    let restore_state = ToplevelRestoreState { creator, wm_properties };

                    if let Some(restore) = self.core.protocol_delegates.session_state.toplevel_restore_state.get(&xdg_toplevel) {
                        if matches!(restore, ToplevelRestore::Pending { committed: true }) {
                            self.core
                                .protocol_delegates
                                .session_state
                                .toplevel_restore_state
                                .remove(&xdg_toplevel);
                            self.handle_toplevel_restore(surface, restore_state);
                        } else {
                            self.core
                                .protocol_delegates
                                .session_state
                                .toplevel_restore_state
                                .insert(xdg_toplevel.clone(), ToplevelRestore::Ready(restore_state));
                        }
                    } else {
                        // The surface died mid-restore, so we don't want to send added or restored
                        // to the client, and instead just drop the creator.
                    }
                } else {
                    self.abandon_toplevel_restore(&surface);
                    if let Some(window) = self.window_for_surface(surface.wl_surface()) {
                        self.core.queue_window_session_sync(&window);
                    }

                    creator.added();
                }
            }
            Err(SessionFutureError::NameConflict(creator)) => creator.name_in_use(),
            Err(SessionFutureError::SmUnavailable(creator)) => self.abandon_toplevel_restore(creator.surface()),
            Err(SessionFutureError::Other(creator, err)) => {
                tracing::info!("Failed to restore toplevel: {err}");
                self.abandon_toplevel_restore(creator.surface());
            }
        }
    }

    fn handle_rename_toplevel_complete(&mut self, result: Result<ToplevelSessionRenamer, SessionFutureError<ToplevelSessionRenamer>>) {
        match result {
            Ok(renamer) => renamer.renamed(),
            Err(SessionFutureError::NameConflict(renamer)) => renamer.name_in_use(),
            Err(SessionFutureError::SmUnavailable(_)) => (),
            Err(SessionFutureError::Other(_, err)) => tracing::info!("Failed to rename toplevel: {err}"),
        }
    }

    fn abandon_toplevel_restore(&mut self, surface: &ToplevelSurface) {
        self.core
            .protocol_delegates
            .session_state
            .toplevel_restore_state
            .remove(surface.xdg_toplevel());
        self.maybe_place_pending_window(surface.wl_surface());
    }
}

impl<BackendData: Backend + 'static> Xfwl4Core<BackendData> {
    pub fn is_restore_state_pending_for_toplevel(&self, surface: &ToplevelSurface) -> bool {
        self.protocol_delegates
            .session_state
            .toplevel_restore_state
            .get(surface.xdg_toplevel())
            .is_some_and(|restore| matches!(restore, ToplevelRestore::Pending { .. }))
    }

    pub fn take_restore_state_for_toplevel(&mut self, surface: &ToplevelSurface) -> Option<ToplevelRestoreState> {
        if self
            .protocol_delegates
            .session_state
            .toplevel_restore_state
            .get(surface.xdg_toplevel())
            .is_some_and(|restore| matches!(restore, ToplevelRestore::Ready(_)))
        {
            self.protocol_delegates
                .session_state
                .toplevel_restore_state
                .remove(surface.xdg_toplevel())
                .map(|restore| {
                    let ToplevelRestore::Ready(restore_state) = restore else {
                        unreachable!()
                    };
                    restore_state
                })
        } else {
            None
        }
    }

    fn set_session_app_id_for_client(&self, client: Client, object_path: ObjectPath) {
        let app_id = self
            .workspace_manager
            .workspaces()
            .iter()
            .flat_map(|workspace| workspace.all_windows())
            .find_map(|window| {
                window.0.toplevel().and_then(|surface| {
                    surface
                        .xdg_toplevel()
                        .client()
                        .is_some_and(|surface_client| surface_client == client)
                        .then(|| app_id_for_xdg_toplevel(surface))
                        .flatten()
                })
            });

        if let Some(app_id) = app_id {
            self.protocol_delegates.session_state.do_set_app_id(object_path, &app_id);
        }
    }

    pub(in crate::core) fn client_sessions_set_app_id(&self, client: &Client, app_id: &str) {
        for session in self
            .protocol_delegates
            .session_state
            .session_management_state
            .sessions_for_client(client)
        {
            if let Some(object_path) = SessionRegistration::get(&session) {
                self.protocol_delegates.session_state.do_set_app_id(object_path, app_id);
            }
        }
    }

    pub(in crate::core) fn queue_window_session_sync(&mut self, window: &WindowElement) {
        window.set_session_state_dirty(true);

        if self.protocol_delegates.session_state.toplevel_sync_timeout_token.is_none() {
            let token = self.register_timer(Timer::from_duration(SESSION_TOPLEVEL_SYNC_TIMEOUT), |state| {
                state.core.protocol_delegates.session_state.toplevel_sync_timeout_token = None;
                state.core.session_sync_toplevel_wm_properties();
                TimeoutAction::Drop
            });
            self.protocol_delegates.session_state.toplevel_sync_timeout_token = Some(token);
        }
    }

    pub(in crate::core) fn do_window_session_sync(&self, window: &WindowElement) {
        if let Some(surface) = window.0.toplevel()
            && window.session_state_dirty()
            && let Some((object_path, toplevel_id)) = ToplevelSessionMetadata::get(surface)
        {
            self.update_wm_properties_for_window(window, object_path, toplevel_id);
        }
    }

    fn session_sync_toplevel_wm_properties(&self) {
        for workspace in self.workspace_manager.workspaces() {
            let windows = workspace
                .visible_windows()
                .chain(workspace.minimized_windows())
                .cloned()
                .collect::<Vec<_>>();

            for window in windows {
                self.do_window_session_sync(&window);
            }
        }
    }

    fn update_wm_properties_for_window(&self, window: &WindowElement, object_path: ObjectPath, toplevel_id: String) {
        let geometry = if window.minimized() {
            self.workspace_manager.minimized_window_geometry(window)
        } else {
            self.workspace_manager.window_geometry(window)
        }
        .unwrap_or_else(|| SpaceElement::geometry(window));

        let output_edid = self.workspace_manager.outputs().find_map(|output| {
            output.geometry().and_then(|output_geom| {
                if output_geom.contains(geometry.loc) {
                    self.outputs_config
                        .config_for_output(output)
                        .and_then(|config| config.edid_hash.clone())
                } else {
                    None
                }
            })
        });

        let states = window.state().difference(WindowState::ACTIVATED | WindowState::DEMANDS_ATTENTION);

        let (saved_geometry, workspace_id) = {
            let props = window.props();

            let saved_geometry = props.saved_geom;
            let workspace_id = match props.workspace_loc {
                WorkspaceLocation::Single(index) => self
                    .workspace_manager
                    .workspaces()
                    .get(index as usize)
                    .map(|workspace| workspace.id()),
                WorkspaceLocation::All => None,
            };

            (saved_geometry, workspace_id)
        };

        let stacking_serials = window.stacking_serials().clone();

        let wm_properties = ToplevelWmProperties {
            geometry,
            saved_geometry,
            output_edid,
            states,
            workspace_id,
            stacking_serials,
        }
        .to_hash_map();

        let xfsm_client = self.protocol_delegates.session_state.xfsm_client.clone();
        let window = window.clone();
        let _ = self
            .protocol_delegates
            .session_state
            .session_delegate_scheduler
            .schedule(async move {
                if let Err(err) = xfsm_client
                    .replace_toplevel_wm_properties(&object_path, &toplevel_id, &wm_properties)
                    .await
                {
                    tracing::warn!("Failed to send WM properties update to SM: {err}");
                } else {
                    window.set_session_state_dirty(false);
                }
                SessionFutureResult::None
            });
    }
}

impl<T> SessionFutureError<T> {
    fn from_session_error(err: SessionError, data: T) -> Self {
        match err {
            SessionError::NameInUse => Self::NameConflict(data),
            SessionError::SmUnavailable => Self::SmUnavailable(data),
            SessionError::Other(err) => Self::Other(data, err),
        }
    }
}

impl SessionRegistration {
    fn init(creator: &SessionCreator) {
        creator
            .user_data()
            .insert_if_missing(|| SessionRegistration(RefCell::new(SessionRegistrationInner::Registering(Vec::new()))));
    }

    fn is_registering(session: &Session) -> bool {
        matches!(session.user_data().get::<SessionRegistration>(), Some(sop) if matches!(&*sop.0.borrow(), SessionRegistrationInner::Registering(_)))
    }

    fn push_pending(session: &Session, pending_action: PendingSmAction) {
        if let Some(sop) = session.user_data().get::<SessionRegistration>()
            && let SessionRegistrationInner::Registering(pending_actions) = &mut *sop.0.borrow_mut()
        {
            pending_actions.push(pending_action);
        }
    }

    fn registered(session: &Session, object_path: ObjectPath) -> Vec<PendingSmAction> {
        if let Some(sop) = session.user_data().get::<SessionRegistration>() {
            if let SessionRegistrationInner::Registering(pending_actions) = sop.0.replace(SessionRegistrationInner::Registered(object_path))
            {
                pending_actions
            } else {
                Vec::new()
            }
        } else {
            session
                .user_data()
                .insert_if_missing(|| SessionRegistration(RefCell::new(SessionRegistrationInner::Registered(object_path))));
            Vec::new()
        }
    }

    fn failed<BackendData: Backend + 'static>(state: &mut Xfwl4State<BackendData>, creator: &SessionCreator) {
        if let Some(sop) = creator.user_data().get::<SessionRegistration>() {
            if let SessionRegistrationInner::Registering(pending_actions) = sop.0.replace(SessionRegistrationInner::RegistrationFailed) {
                for action in pending_actions {
                    if let PendingSmAction::RestoreToplevel(creator) = action {
                        state.abandon_toplevel_restore(creator.surface());
                    }
                }
            }
        } else {
            creator
                .user_data()
                .insert_if_missing(|| SessionRegistration(RefCell::new(SessionRegistrationInner::RegistrationFailed)));
        }
    }

    fn get(session: &Session) -> Option<ObjectPath> {
        session.user_data().get::<SessionRegistration>().and_then(|sop| {
            if let SessionRegistrationInner::Registered(object_path) = &*sop.0.borrow() {
                Some(object_path.clone())
            } else {
                None
            }
        })
    }
}

impl ToplevelSessionMetadata {
    fn get(surface: &ToplevelSurface) -> Option<(ObjectPath, String)> {
        compositor::with_states(surface.wl_surface(), |states| {
            states.data_map.get::<ToplevelSessionMetadata>().and_then(|data| {
                data.0
                    .borrow()
                    .as_ref()
                    .map(|inner| (inner.client_object_path.clone(), inner.toplevel_id.clone()))
            })
        })
    }

    fn update(surface: &ToplevelSurface, client_object_path: ObjectPath, toplevel_id: String) {
        compositor::with_states(surface.wl_surface(), |states| {
            if let Some(data) = states.data_map.get::<ToplevelSessionMetadata>() {
                data.0.borrow_mut().replace(ToplevelSessionMetadataInner {
                    client_object_path,
                    toplevel_id,
                });
            } else {
                states.data_map.insert_if_missing(|| {
                    ToplevelSessionMetadata(RefCell::new(Some(ToplevelSessionMetadataInner {
                        client_object_path,
                        toplevel_id,
                    })))
                });
            }
        });
    }

    fn clear(surface: &ToplevelSurface) {
        compositor::with_states(surface.wl_surface(), |states| {
            if let Some(data) = states.data_map.get::<ToplevelSessionMetadata>() {
                *data.0.borrow_mut() = None;
            }
        });
    }
}

impl WindowElement {
    pub(in crate::core) fn session_state_dirty(&self) -> bool {
        self.0
            .user_data()
            .get::<ToplevelSessionStateDirty>()
            .map(|dirty| dirty.0.load(Ordering::Acquire))
            .unwrap_or(false)
    }

    pub(in crate::core) fn set_session_state_dirty(&self, is_dirty: bool) {
        self.0
            .user_data()
            .get_or_insert(ToplevelSessionStateDirty::default)
            .0
            .store(is_dirty, Ordering::Release);
    }
}

impl ToplevelRestoreState {
    pub fn reason(&self) -> Reason {
        self.creator.session().reason()
    }

    pub fn restored(self) {
        self.creator.restored();
    }
}

impl ToplevelWmPropertyNames {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Geometry => "Geometry",
            Self::SavedGeometry => "SavedGeometry",
            Self::WorkspaceId => "WorkspaceId",
            Self::OutputEdid => "OutputEdid",
            Self::States => "States",
            Self::StackingSerials => "StackingSerials",
        }
    }

    fn to_variant(self, properties: &ToplevelWmProperties) -> Option<glib::Variant> {
        match self {
            Self::Geometry => Some(
                (
                    properties.geometry.loc.x,
                    properties.geometry.loc.y,
                    properties.geometry.size.w,
                    properties.geometry.size.h,
                )
                    .to_variant(),
            ),
            Self::SavedGeometry => properties
                .saved_geometry
                .map(|geom| (geom.loc.x, geom.loc.y, geom.size.w, geom.size.h).to_variant()),
            Self::WorkspaceId => properties.workspace_id.map(|id| id.to_variant()),
            Self::OutputEdid => properties.output_edid.as_ref().map(|edid| edid.to_variant()),
            Self::States => Some(properties.states.bits().to_variant()),
            Self::StackingSerials => Some(properties.stacking_serials.iter().collect::<Vec<_>>().to_variant()),
        }
    }

    fn to_hash_map_tuple(self, properties: &ToplevelWmProperties) -> Option<(String, glib::Variant)> {
        self.to_variant(properties).map(|value| (self.as_str().to_owned(), value))
    }
}

impl ToplevelWmProperties {
    fn to_hash_map(&self) -> HashMap<String, glib::Variant> {
        [
            ToplevelWmPropertyNames::Geometry,
            ToplevelWmPropertyNames::SavedGeometry,
            ToplevelWmPropertyNames::WorkspaceId,
            ToplevelWmPropertyNames::OutputEdid,
            ToplevelWmPropertyNames::States,
            ToplevelWmPropertyNames::StackingSerials,
        ]
        .into_iter()
        .flat_map(|name| name.to_hash_map_tuple(self))
        .collect()
    }
}

impl From<HashMap<String, glib::Variant>> for ToplevelWmProperties {
    fn from(value: HashMap<String, glib::Variant>) -> Self {
        Self {
            geometry: value
                .get(ToplevelWmPropertyNames::Geometry.as_str())
                .and_then(|geom| geom.get::<(i32, i32, i32, i32)>())
                .map(|(x, y, w, h)| Rectangle::new((x, y).into(), (w, h).into()))
                .unwrap_or_default(),
            saved_geometry: value
                .get(ToplevelWmPropertyNames::SavedGeometry.as_str())
                .and_then(|geom| geom.get::<(i32, i32, i32, i32)>())
                .map(|(x, y, w, h)| Rectangle::new((x, y).into(), (w, h).into())),
            workspace_id: value
                .get(ToplevelWmPropertyNames::WorkspaceId.as_str())
                .and_then(|index| index.get::<u64>()),
            output_edid: value
                .get(ToplevelWmPropertyNames::OutputEdid.as_str())
                .and_then(|edid| edid.get::<String>()),
            states: value
                .get(ToplevelWmPropertyNames::States.as_str())
                .and_then(|states| states.get::<u32>())
                .map(WindowState::from_bits_truncate)
                .unwrap_or(WindowState::empty()),
            stacking_serials: value
                .get(ToplevelWmPropertyNames::StackingSerials.as_str())
                .and_then(|serial| serial.get::<Vec<(u64, u64)>>())
                .map(|serials| serials.into_iter().collect())
                .unwrap_or_default(),
        }
    }
}
