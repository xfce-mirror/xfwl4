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
    output::Output,
    reexports::{
        wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1,
        wayland_server::{DisplayHandle, Resource, backend::ClientId, protocol::wl_output::WlOutput},
    },
    utils::{IsAlive, SERIAL_COUNTER},
    wayland::{
        compositor,
        session_lock::{LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker},
    },
};

use crate::{
    backend::Backend,
    core::{
        focus::KeyboardFocusTarget,
        state::Xfwl4State,
        util::{ClientExt, OutputExt},
    },
};

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Default, PartialEq)]
enum LockState {
    #[default]
    Unlocked,
    Locked {
        lock: ExtSessionLockV1,
        client_id: ClientId,
        previous_focus: Option<KeyboardFocusTarget>,
    },
    Orphaned,
}

pub struct ExtSessionLockState {
    manager_state: SessionLockManagerState,
    state: LockState,
    lock_surfaces: HashMap<Output, LockSurface>,
}

impl ExtSessionLockState {
    pub fn new<BackendData: Backend + 'static>(dh: &DisplayHandle) -> Self {
        Self {
            manager_state: SessionLockManagerState::new::<Xfwl4State<BackendData>, _>(dh, |client| !client.has_security_context()),
            state: LockState::default(),
            lock_surfaces: HashMap::new(),
        }
    }

    pub fn is_locked(&self) -> bool {
        !matches!(self.state, LockState::Unlocked)
    }

    pub fn lock_surface_for_output(&self, output: &Output) -> Option<&LockSurface> {
        self.lock_surfaces.get(output).filter(|ls| self.is_locked() && ls.alive())
    }

    pub(super) fn client_disconnected(&mut self, client_id: ClientId) {
        if let LockState::Locked {
            client_id: lock_client_id, ..
        } = &self.state
            && client_id == *lock_client_id
        {
            tracing::warn!("Session lock client has quit without unlocking the session; session will remain locked forever");
            self.state = LockState::Orphaned;
            self.lock_surfaces.clear();
        }
    }
}

impl<BackendData: Backend + 'static> SessionLockHandler for Xfwl4State<BackendData> {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.core.protocol_delegates.ext_session_lock_state.manager_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        if !self.core.protocol_delegates.ext_session_lock_state.is_locked()
            && let Some(client_id) = confirmation.ext_session_lock().client().map(|client| client.id())
        {
            if let Some(pointer) = self.core.seat.get_pointer() {
                pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), self.core.now().as_millis());
            }
            if let Some(keyboard) = self.core.seat.get_keyboard() {
                keyboard.unset_grab(self);
            }
            if let Some(touch) = self.core.seat.get_touch() {
                touch.unset_grab(self);
            }

            self.core.protocol_delegates.ext_session_lock_state.state = LockState::Locked {
                lock: confirmation.ext_session_lock().clone(),
                client_id,
                previous_focus: self.core.seat.get_keyboard().and_then(|keyboard| keyboard.current_focus()),
            };

            confirmation.lock();
        }
    }

    fn new_surface(&mut self, surface: LockSurface, wl_output: WlOutput) {
        if let LockState::Locked { client_id, .. } = &self.core.protocol_delegates.ext_session_lock_state.state
            && let Some(output) = Output::from_resource(&wl_output)
            && surface.wl_surface().client().is_some_and(|client| client.id() == *client_id)
        {
            let wl_surface = surface.wl_surface().clone();

            surface.with_pending_state(|state| {
                state.size = output.geometry().map(|geom| (geom.size.w as u32, geom.size.h as u32).into());
            });
            surface.send_configure();

            compositor::add_destruction_hook(surface.wl_surface(), |state: &mut Self, surf| {
                state
                    .core
                    .protocol_delegates
                    .ext_session_lock_state
                    .lock_surfaces
                    .retain(|_, v| v.wl_surface() != surf);
            });
            self.core
                .protocol_delegates
                .ext_session_lock_state
                .lock_surfaces
                .insert(output, surface);

            if self
                .core
                .seat
                .get_keyboard()
                .and_then(|keyboard| keyboard.current_focus())
                .filter(|focus| matches!(focus, KeyboardFocusTarget::LockSurface(w) if w.alive()))
                .is_none()
            {
                self.focus_target(KeyboardFocusTarget::LockSurface(wl_surface), SERIAL_COUNTER.next_serial(), None);
            }
        }
    }

    fn unlock(&mut self) {
        if let LockState::Locked { previous_focus, .. } = &self.core.protocol_delegates.ext_session_lock_state.state {
            let previous_focus = previous_focus.clone();
            self.core.protocol_delegates.ext_session_lock_state.state = LockState::Unlocked;
            self.core.protocol_delegates.ext_session_lock_state.lock_surfaces.clear();

            if let Some(previous_focus) = previous_focus {
                self.focus_target(previous_focus, SERIAL_COUNTER.next_serial(), None);
            }
        }
    }
}
