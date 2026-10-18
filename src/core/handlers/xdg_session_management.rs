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
    fs::{self, OpenOptions},
    io::Write,
    os::fd::AsFd,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

use anyhow::anyhow;
use calloop::{
    LoopHandle, RegistrationToken,
    channel::Event,
    futures::{Scheduler, executor},
    timer::{TimeoutAction, Timer},
};
use smithay::{
    desktop::space::SpaceElement,
    reexports::{wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel, wayland_server::DisplayHandle},
    utils::{IsAlive, Logical, Rectangle},
    wayland::{compositor, shell::xdg::ToplevelSurface},
};

use crate::{
    backend::Backend,
    core::{
        config::OutputsConfig,
        shell::{WindowElement, WindowState, WorkspaceLocation},
        state::{Xfwl4Core, Xfwl4State},
        util::{SessionError, SessionEvent, XfsmClient},
        workspaces::WorkspaceManager,
    },
    protocols::xdg_session_management::{
        Session, SessionCreator, SessionManagementHandler, SessionManagementState, ToplevelSessionCreator, ToplevelSessionRemover,
        ToplevelSessionRenamer, proto::xdg_session_manager_v1::Reason,
    },
    util::OutputExt,
};

const SESSION_STORE_FILENAME: &str = "sessions.yaml";
const SESSION_STORE_SAVE_TIMEOUT: Duration = Duration::from_secs(2);
const SESSION_DATA_EXPIRATION: Duration = Duration::from_hours(9 * 30 * 24); // ~9 months

pub struct SessionState {
    session_management_state: SessionManagementState,
    xfsm_client: XfsmClient,

    session_scheduler: Scheduler<SessionFutureResult>,
    logout_in_progress: bool,

    session_store: SessionStore,
    session_store_save_token: Option<RegistrationToken>,
    toplevel_restore_state: HashMap<XdgToplevel, ToplevelRestoreState>,
}

struct ToplevelSessionMetadataInner {
    session_id: String,
    toplevel_id: String,
}

struct ToplevelSessionMetadata(RefCell<Option<ToplevelSessionMetadataInner>>);

#[derive(Debug)]
pub struct ToplevelRestoreState {
    pub wm_properties: ToplevelWmProperties,
    creator: ToplevelSessionCreator,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ToplevelWmProperties {
    #[serde(with = "super::super::util::serde_smithay::serde_rectangle")]
    pub geometry: Rectangle<i32, Logical>,
    #[serde(
        with = "super::super::util::serde_smithay::serde_optional_rectangle",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub saved_geometry: Option<Rectangle<i32, Logical>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub workspace_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub output_edid: Option<String>,
    #[serde(with = "serde_window_state")]
    pub states: WindowState,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub stacking_serials: HashMap<u64, u64>, // workspace ID -> stacking serial
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct SessionStore {
    sessions: HashMap<String, SessionData>,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct SessionData {
    #[serde(skip)]
    owner: Option<Session>,
    toplevels: HashMap<String, ToplevelSessionData>,
    last_seen: u64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ToplevelSessionData {
    #[serde(skip)]
    owner: Option<WindowElement>,
    last_seen: u64,
    #[serde(flatten)]
    wm_properties: ToplevelWmProperties,
}

#[derive(Debug, Default)]
struct ToplevelSessionStateDirty(AtomicBool);

#[allow(private_interfaces)]
enum SessionFutureResult {
    GetEnvironmentComplete(Result<HashMap<String, String>, SessionError>),
    RequestLogoutComplete,
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
                                state.core.protocol_delegates.session_state.update_session_environment();
                            }
                        }

                        SessionEvent::Checkpoint | SessionEvent::ShutdownStarted => state.core.flush_session_store(),
                    }
                }
            })
            .expect("failed to insert XfsmClient notifier source");

        let (session_executor, session_scheduler) = executor()?;
        handle
            .insert_source(session_executor, |result, _, state| {
                state.core.handle_session_future_completion(result);
            })
            .map_err(|err| anyhow!("Failed to register future executor source: {err}"))?;

        let session_management_state = SessionManagementState::new::<Xfwl4State<BackendData>>(dh);

        let session_store = Self::load_session_store().unwrap_or_else(|err| {
            tracing::warn!("Failed to read session store from disk; re-initializing with empty store: {err}");
            SessionStore::default()
        });

        Ok(Self {
            session_management_state,
            xfsm_client,
            session_scheduler,
            logout_in_progress: false,
            session_store,
            session_store_save_token: None,
            toplevel_restore_state: HashMap::new(),
        })
    }

    fn session_store_dir() -> PathBuf {
        let mut dir = glib::user_state_dir();
        dir.push("xfce4");
        dir.push("xfwl4");
        dir
    }

    fn load_session_store() -> anyhow::Result<SessionStore> {
        let mut store_path = Self::session_store_dir();
        store_path.push(SESSION_STORE_FILENAME);

        if store_path.exists() {
            let f = fs::File::open(&store_path)?;
            Ok(yaml_serde::from_reader::<_, SessionStore>(f)?)
        } else {
            Ok(SessionStore::default())
        }
    }

    fn save_session_store(&mut self) -> anyhow::Result<()> {
        match now_msec() {
            Ok(now_msec) => {
                self.session_store.sessions.retain(|_, session_data| {
                    if session_data.owner.is_some() {
                        session_data.last_seen = now_msec;
                        true
                    } else if now_msec.saturating_sub(session_data.last_seen) > SESSION_DATA_EXPIRATION.as_millis() as u64 {
                        false
                    } else {
                        session_data.toplevels.retain(|_, toplevel_data| {
                            if toplevel_data.owner.as_ref().filter(IsAlive::alive).is_some() {
                                toplevel_data.last_seen = now_msec;
                                true
                            } else {
                                now_msec.saturating_sub(toplevel_data.last_seen) > SESSION_DATA_EXPIRATION.as_millis() as u64
                            }
                        });
                        true
                    }
                });
            }
            Err(err) => tracing::warn!("Unable to get current time: {err}"),
        }

        let mut store_path_tmp = Self::session_store_dir();
        fs::create_dir_all(&store_path_tmp)?;

        store_path_tmp.push(format!("{SESSION_STORE_FILENAME}.new"));
        let mut f = OpenOptions::new().write(true).truncate(true).create(true).open(&store_path_tmp)?;
        yaml_serde::to_writer(&f, &self.session_store)?;

        f.flush()?;
        rustix::fs::fsync(f.as_fd())?;

        let mut store_path = Self::session_store_dir();
        store_path.push(SESSION_STORE_FILENAME);
        fs::rename(store_path_tmp, store_path)?;

        Ok(())
    }

    pub fn request_logout(&mut self) {
        if !self.logout_in_progress {
            self.logout_in_progress = true;
            let xfsm_client = self.xfsm_client.clone();
            let _ = self.session_scheduler.schedule(async move {
                if let Err(err) = xfsm_client.request_logout().await {
                    tracing::warn!("Failed to request logout from session manager: {err}");
                }
                SessionFutureResult::RequestLogoutComplete
            });
        }
    }

    fn update_session_environment(&self) {
        let xfsm_client = self.xfsm_client.clone();
        let _ = self
            .session_scheduler
            .schedule(async move { SessionFutureResult::GetEnvironmentComplete(xfsm_client.get_environment().await) });
    }
}

impl<BackendData: Backend + 'static> SessionManagementHandler for Xfwl4State<BackendData> {
    fn session_management_state(&mut self) -> &mut SessionManagementState {
        &mut self.core.protocol_delegates.session_state.session_management_state
    }

    fn new_session(&mut self, creator: SessionCreator) {
        let now = now_msec().unwrap_or(0);
        let restored = self
            .core
            .protocol_delegates
            .session_state
            .session_store
            .sessions
            .contains_key(creator.id());

        let session_data = self
            .core
            .protocol_delegates
            .session_state
            .session_store
            .sessions
            .entry(creator.id().to_owned())
            .or_default();
        session_data.owner = Some(creator.session().clone());
        session_data.last_seen = now;
        for toplevel in session_data.toplevels.values_mut() {
            if let Some(old_owner) = toplevel.owner.take().filter(IsAlive::alive) {
                if let Some(old_surface) = old_owner.0.toplevel() {
                    ToplevelSessionMetadata::clear(old_surface);
                }
                toplevel.last_seen = now;
            }
        }

        if restored {
            creator.restored(Reason::SessionRestore);
        } else {
            creator.created(None, Reason::Launch);
        }

        self.core.queue_session_save();
    }

    fn remove_session(&mut self, session: Session) {
        if self
            .core
            .protocol_delegates
            .session_state
            .session_store
            .sessions
            .remove(&session.id())
            .is_some()
        {
            self.core.queue_session_save();
        }
    }

    fn release_session(&mut self, session: Session) {
        let now = now_msec().unwrap_or(0);

        if let Some(session_data) = self
            .core
            .protocol_delegates
            .session_state
            .session_store
            .sessions
            .get_mut(&session.id())
        {
            if session_data.owner.is_some() {
                session_data.last_seen = now;
                session_data.owner = None;
            }

            for toplevel in session_data.toplevels.values_mut() {
                if let Some(old_owner) = toplevel.owner.take().filter(IsAlive::alive) {
                    if let Some(old_surface) = old_owner.0.toplevel() {
                        ToplevelSessionMetadata::clear(old_surface);
                    }
                    toplevel.last_seen = now;
                }
            }

            self.core.queue_session_save();
        }
    }

    fn add_toplevel(&mut self, creator: ToplevelSessionCreator) {
        let now = now_msec().unwrap_or(0);

        if let Some(window) = self.window_for_surface(creator.surface().wl_surface())
            && let Some(session_data) = self
                .core
                .protocol_delegates
                .session_state
                .session_store
                .sessions
                .get_mut(&creator.session().id())
        {
            if session_data.toplevels.contains_key(creator.name()) {
                creator.name_in_use();
            } else {
                let wm_properties = window.wm_properties(&self.core.workspace_manager, &self.core.outputs_config);
                let toplevel_data = ToplevelSessionData {
                    owner: Some(window),
                    wm_properties,
                    last_seen: now,
                };
                session_data.toplevels.insert(creator.name().to_owned(), toplevel_data);
                ToplevelSessionMetadata::update(creator.surface(), creator.session().id(), creator.name().to_owned());

                self.core.queue_session_save();
                creator.added();
            }
        }
    }

    fn restore_toplevel(&mut self, creator: ToplevelSessionCreator) {
        let now = now_msec().unwrap_or(0);

        if let Some(window) = self.window_for_surface(creator.surface().wl_surface())
            && let Some(session_data) = self
                .core
                .protocol_delegates
                .session_state
                .session_store
                .sessions
                .get_mut(&creator.session().id())
        {
            ToplevelSessionMetadata::update(creator.surface(), creator.session().id(), creator.name().to_owned());

            if let Some(toplevel_data) = session_data.toplevels.get_mut(creator.name()) {
                toplevel_data.owner = Some(window);
                toplevel_data.last_seen = now;

                let hook_id_holder = Arc::new(Mutex::new(None));
                let hook_id = {
                    let surface = creator.surface().clone();
                    let hook_id_holder = Arc::clone(&hook_id_holder);
                    compositor::add_post_commit_hook::<Self, _>(creator.surface().wl_surface(), move |state, _, wl_surface| {
                        if let Some((hook_id, xdg_toplevel)) = hook_id_holder.lock().unwrap().take() {
                            compositor::remove_post_commit_hook(wl_surface, &hook_id);

                            if let Some(restore_state) = state
                                .core
                                .protocol_delegates
                                .session_state
                                .toplevel_restore_state
                                .remove(&xdg_toplevel)
                            {
                                state.handle_toplevel_restore(surface.clone(), restore_state);
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

                let xdg_toplevel = creator.surface().xdg_toplevel().clone();
                let restore_state = ToplevelRestoreState {
                    wm_properties: toplevel_data.wm_properties.clone(),
                    creator,
                };
                self.core
                    .protocol_delegates
                    .session_state
                    .toplevel_restore_state
                    .insert(xdg_toplevel, restore_state);
            } else {
                let wm_properties = window.wm_properties(&self.core.workspace_manager, &self.core.outputs_config);
                let toplevel_data = ToplevelSessionData {
                    owner: Some(window),
                    wm_properties,
                    last_seen: now,
                };
                session_data.toplevels.insert(creator.name().to_owned(), toplevel_data);
                creator.added();
            }

            self.core.queue_session_save();
        }
    }

    fn rename_toplevel(&mut self, renamer: ToplevelSessionRenamer) {
        if let Some(session_data) = self
            .core
            .protocol_delegates
            .session_state
            .session_store
            .sessions
            .get_mut(&renamer.session().id())
        {
            if session_data.toplevels.contains_key(renamer.new_name()) {
                renamer.name_in_use();
            } else if let Some(toplevel_data) = session_data.toplevels.remove(&renamer.old_name()) {
                session_data.toplevels.insert(renamer.new_name().to_owned(), toplevel_data);
                self.core.queue_session_save();
            }
        }
    }

    fn remove_toplevel(&mut self, remover: ToplevelSessionRemover) {
        if let Some(session_data) = self
            .core
            .protocol_delegates
            .session_state
            .session_store
            .sessions
            .get_mut(&remover.session().id())
            && session_data.toplevels.remove(remover.name()).is_some()
        {
            self.core.queue_session_save();
        }
    }
}

impl<BackendData: Backend + 'static> Xfwl4Core<BackendData> {
    pub(in crate::core) fn take_restore_state_for_toplevel(&mut self, surface: &ToplevelSurface) -> Option<ToplevelRestoreState> {
        self.protocol_delegates
            .session_state
            .toplevel_restore_state
            .remove(surface.xdg_toplevel())
    }

    pub(in crate::core) fn queue_window_session_sync(&mut self, window: &WindowElement) {
        window.set_session_state_dirty(true);
        self.queue_session_save();
    }

    pub(in crate::core) fn sync_session_store_for_window(&mut self, window: &WindowElement) {
        if let Some(surface) = window.0.toplevel()
            && window.session_state_dirty()
            && let Some((session_id, toplevel_id)) = ToplevelSessionMetadata::get(surface)
            && let Some(session_data) = self.protocol_delegates.session_state.session_store.sessions.get_mut(&session_id)
            && let Some(toplevel_data) = session_data.toplevels.get_mut(&toplevel_id)
        {
            toplevel_data.wm_properties = window.wm_properties(&self.workspace_manager, &self.outputs_config);
            window.set_session_state_dirty(false);
            self.queue_session_save();
        }
    }

    pub(in crate::core) fn flush_session_store(&mut self) {
        if let Some(token) = self.protocol_delegates.session_state.session_store_save_token.take() {
            self.unregister_timer(token);
            self.session_sync_toplevel_wm_properties();
        }
    }

    fn session_sync_toplevel_wm_properties(&mut self) {
        let windows = self
            .workspace_manager
            .workspaces()
            .iter()
            .flat_map(|workspace| workspace.visible_windows().chain(workspace.minimized_windows()))
            .cloned()
            .collect::<Vec<_>>();

        for window in windows {
            self.sync_session_store_for_window(&window);
        }

        if let Err(err) = self.protocol_delegates.session_state.save_session_store() {
            tracing::warn!("Failed to write sessions to disk: {err}");
        }
    }

    fn queue_session_save(&mut self) {
        if self.protocol_delegates.session_state.session_store_save_token.is_none() {
            let token = self.register_timer(Timer::from_duration(SESSION_STORE_SAVE_TIMEOUT), |state| {
                state.core.protocol_delegates.session_state.session_store_save_token = None;
                state.core.session_sync_toplevel_wm_properties();
                TimeoutAction::Drop
            });
            self.protocol_delegates.session_state.session_store_save_token = Some(token);
        }
    }

    fn handle_session_future_completion(&mut self, action: SessionFutureResult) {
        match action {
            SessionFutureResult::GetEnvironmentComplete(result) => match result {
                Err(err) => tracing::warn!("Failed to fetch environment from session manager: {err}"),
                Ok(env_vars) => {
                    for (key, value) in env_vars {
                        self.register_subprocess_env(key, value);
                    }
                }
            },
            SessionFutureResult::RequestLogoutComplete => self.protocol_delegates.session_state.logout_in_progress = false,
        }
    }
}

impl ToplevelSessionMetadata {
    fn get(surface: &ToplevelSurface) -> Option<(String, String)> {
        compositor::with_states(surface.wl_surface(), |states| {
            states.data_map.get::<ToplevelSessionMetadata>().and_then(|data| {
                data.0
                    .borrow()
                    .as_ref()
                    .map(|inner| (inner.session_id.clone(), inner.toplevel_id.clone()))
            })
        })
    }

    fn update(surface: &ToplevelSurface, session_id: String, toplevel_id: String) {
        compositor::with_states(surface.wl_surface(), |states| {
            if let Some(data) = states.data_map.get::<ToplevelSessionMetadata>() {
                data.0
                    .borrow_mut()
                    .replace(ToplevelSessionMetadataInner { session_id, toplevel_id });
            } else {
                states.data_map.insert_if_missing(|| {
                    ToplevelSessionMetadata(RefCell::new(Some(ToplevelSessionMetadataInner { session_id, toplevel_id })))
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

    fn wm_properties<BackendData: Backend + 'static>(
        &self,
        workspace_manager: &WorkspaceManager<BackendData>,
        outputs_config: &OutputsConfig<BackendData>,
    ) -> ToplevelWmProperties {
        let geometry = if self.minimized() {
            workspace_manager.minimized_window_geometry(self)
        } else {
            workspace_manager.window_geometry(self)
        }
        .unwrap_or_else(|| SpaceElement::geometry(self));

        let output_edid = workspace_manager.outputs().find_map(|output| {
            output.geometry().and_then(|output_geom| {
                if output_geom.contains(geometry.loc) {
                    outputs_config.config_for_output(output).and_then(|config| config.edid_hash.clone())
                } else {
                    None
                }
            })
        });

        let states = self.state().difference(WindowState::ACTIVATED | WindowState::DEMANDS_ATTENTION);

        let (saved_geometry, workspace_id) = {
            let props = self.props();

            let saved_geometry = props.saved_geom;
            let workspace_id = match props.workspace_loc {
                WorkspaceLocation::Single(index) => workspace_manager.workspaces().get(index as usize).map(|workspace| workspace.id()),
                WorkspaceLocation::All => None,
            };

            (saved_geometry, workspace_id)
        };

        let stacking_serials = self.stacking_serials().clone();

        ToplevelWmProperties {
            geometry,
            saved_geometry,
            output_edid,
            states,
            workspace_id,
            stacking_serials,
        }
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

fn now_msec() -> anyhow::Result<u64> {
    Ok(SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_millis() as u64)
}

mod serde_window_state {
    use std::fmt;

    use serde::{
        Deserializer, Serializer,
        de::{SeqAccess, Visitor},
        ser::SerializeSeq,
    };

    use crate::core::shell::WindowState;

    const MAPPING: [(WindowState, &str); 16] = [
        (WindowState::ACTIVATED, "activated"),
        (WindowState::MINIMIZED, "minimized"),
        (WindowState::MAXIMIZED_VERTICAL, "maximized_vertical"),
        (WindowState::MAXIMIZED_HORIZONTAL, "maximized_horizontal"),
        (WindowState::SHADED, "shaded"),
        (WindowState::STICKY, "sticky"),
        (WindowState::FULLSCREEN, "fullscreen"),
        (WindowState::SKIP_TASKBAR, "skip_taskbar"),
        (WindowState::SKIP_PAGER, "skip_pager"),
        (WindowState::KEEP_ABOVE, "keep_above"),
        (WindowState::KEEP_BELOW, "keep_below"),
        (WindowState::DEMANDS_ATTENTION, "demands_attention"),
        (WindowState::TILED_UP, "tiled_up"),
        (WindowState::TILED_DOWN, "tiled_down"),
        (WindowState::TILED_LEFT, "tiled_left"),
        (WindowState::TILED_RIGHT, "tiled_right"),
    ];

    pub fn serialize<S>(state: &WindowState, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let values = MAPPING.iter().flat_map(|(bit, value)| state.contains(*bit).then_some(*value));

        let (lower, upper) = values.size_hint();
        let len = upper.unwrap_or(lower);

        let mut seq = serializer.serialize_seq(Some(len))?;
        for value in values {
            seq.serialize_element(&value)?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<WindowState, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct WindowStateVisitor;

        impl<'de> Visitor<'de> for WindowStateVisitor {
            type Value = WindowState;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an array of state strings")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut state = WindowState::empty();

                while let Some(value) = seq.next_element::<String>()? {
                    if let Some(bit) = MAPPING.iter().find_map(|(bit, s)| (value == *s).then_some(*bit)) {
                        state |= bit;
                    }
                }

                Ok(state)
            }
        }

        deserializer.deserialize_seq(WindowStateVisitor)
    }
}
