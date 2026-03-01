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

use crate::{
    backend::Backend,
    core::{
        state::Xfwl4State,
        util::{ClientExt, OutputExt},
    },
};

pub struct ExtSessionLockState {
    manager_state: SessionLockManagerState,
    session_lock: Option<ExtSessionLockV1>,
    lock_surfaces: HashMap<Output, LockSurface>,
}

impl ExtSessionLockState {
    pub fn new<BackendData: Backend + 'static>(dh: &DisplayHandle) -> Self {
        Self {
            manager_state: SessionLockManagerState::new::<Xfwl4State<BackendData>, _>(dh, |client| !client.has_security_context()),
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
        &mut self.core.ext_session_lock_state.manager_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        if !self.core.ext_session_lock_state.is_session_locker_active(&self.core.display_handle) {
            self.core.ext_session_lock_state.session_lock = Some(confirmation.ext_session_lock().clone());
            confirmation.lock();
        }
    }

    fn new_surface(&mut self, surface: LockSurface, wl_output: WlOutput) {
        if let Some(session_lock) = &self.core.ext_session_lock_state.session_lock
            && let Some(output) = Output::from_resource(&wl_output)
            && surface.wl_surface().client().is_some()
            && surface.wl_surface().client() == session_lock.client()
        {
            surface.with_pending_state(|state| {
                state.size = output.geometry().map(|geom| (geom.size.w as u32, geom.size.h as u32).into());
            });
            surface.send_configure();

            compositor::add_destruction_hook(surface.wl_surface(), |state: &mut Self, surf| {
                state
                    .core
                    .ext_session_lock_state
                    .lock_surfaces
                    .retain(|_, v| v.wl_surface() != surf);
            });
            self.core.ext_session_lock_state.lock_surfaces.insert(output, surface);
        }
    }

    fn unlock(&mut self) {
        self.core.ext_session_lock_state.session_lock = None;
        self.core.ext_session_lock_state.lock_surfaces.clear();
    }
}

delegate_session_lock!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
