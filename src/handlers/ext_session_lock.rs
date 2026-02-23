use std::collections::HashMap;

use smithay::{
    delegate_session_lock,
    output::Output,
    reexports::{
        wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1,
        wayland_server::{DisplayHandle, Resource, protocol::wl_output::WlOutput},
    },
    wayland::{
        compositor,
        session_lock::{LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker},
    },
};

use crate::{Xfwl4State, backend::Backend, util::OutputExt};

pub struct ExtSessionLockState {
    manager_state: SessionLockManagerState,
    session_lock: Option<ExtSessionLockV1>,
    lock_surfaces: HashMap<Output, LockSurface>,
}

impl ExtSessionLockState {
    pub fn new<BackendData: Backend + 'static>(dh: &DisplayHandle) -> Self {
        Self {
            manager_state: SessionLockManagerState::new::<Xfwl4State<BackendData>, _>(dh, |_| true),
            session_lock: None,
            lock_surfaces: HashMap::new(),
        }
    }

    pub fn lock_surface_for_output(&self, output: &Output) -> Option<&LockSurface> {
        self.lock_surfaces
            .get(output)
            .filter(|ls| self.session_lock.is_some() && ls.alive())
    }

    fn is_session_locker_active(&mut self, dh: &DisplayHandle) -> bool {
        if let Some(session_lock) = self.session_lock.take() {
            if dh.get_client(session_lock.id()).is_ok() {
                self.session_lock = Some(session_lock);
                true
            } else {
                false
            }
        } else {
            false
        }
    }
}

impl<BackendData: Backend + 'static> SessionLockHandler for Xfwl4State<BackendData> {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.ext_session_lock_state.manager_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        if !self.ext_session_lock_state.is_session_locker_active(&self.display_handle) {
            self.ext_session_lock_state.session_lock = Some(confirmation.ext_session_lock().clone());
            confirmation.lock();
        }
    }

    fn new_surface(&mut self, surface: LockSurface, wl_output: WlOutput) {
        if let Some(session_lock) = &self.ext_session_lock_state.session_lock
            && let Some(output) = Output::from_resource(&wl_output)
            && surface.wl_surface().client().is_some()
            && surface.wl_surface().client() == session_lock.client()
        {
            surface.with_pending_state(|state| {
                state.size = output.geometry().map(|geom| (geom.size.w as u32, geom.size.h as u32).into());
            });
            surface.send_configure();

            compositor::add_destruction_hook(surface.wl_surface(), |state: &mut Self, surf| {
                state.ext_session_lock_state.lock_surfaces.retain(|_, v| v.wl_surface() != surf);
            });
            self.ext_session_lock_state.lock_surfaces.insert(output, surface);
        }
    }

    fn unlock(&mut self) {
        self.ext_session_lock_state.session_lock = None;
        self.ext_session_lock_state.lock_surfaces.clear();
    }
}

delegate_session_lock!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
