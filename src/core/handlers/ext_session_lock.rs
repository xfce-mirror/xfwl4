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
    output::{Output, WeakOutput},
    reexports::wayland_server::{DisplayHandle, Resource, backend::ClientId, protocol::wl_output::WlOutput},
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
#[derive(Debug, Default)]
enum LockState {
    #[default]
    Unlocked,
    Locking {
        locker: SessionLocker,
        client_id: ClientId,
        previous_focus: Option<KeyboardFocusTarget>,
        pending_outputs: Vec<WeakOutput>,
    },
    Locked {
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

    pub fn is_lock_pending(&self) -> bool {
        matches!(self.state, LockState::Locking { .. })
    }

    pub fn is_locked(&self) -> bool {
        !matches!(self.state, LockState::Unlocked)
    }

    pub fn lock_surface_for_output(&self, output: &Output) -> Option<&LockSurface> {
        self.lock_surfaces.get(output).filter(|ls| self.is_locked() && ls.alive())
    }

    pub(super) fn output_locked(&mut self, output: &Output) {
        if let LockState::Locking { pending_outputs, .. } = &mut self.state {
            pending_outputs.retain(|pending_output| pending_output != output && pending_output.is_alive());

            if pending_outputs.is_empty() {
                let LockState::Locking {
                    locker,
                    client_id,
                    previous_focus,
                    ..
                } = std::mem::replace(&mut self.state, LockState::Orphaned)
                else {
                    unreachable!()
                };

                self.state = LockState::Locked { client_id, previous_focus };
                // This isn't 100% spec compliant: the spec wants me to not send `locked` to the
                // client until blanked frames have hit each monitor, but we do it here when
                // blanked frames have been merely submitted to the hardware.  Doing the latter is
                // a bit more annoying, and I feel like it doesn't matter much.
                locker.lock();
            }
        }
    }

    pub(super) fn client_disconnected(&mut self, client_id: ClientId) {
        if matches!(&self.state, LockState::Locked { client_id: lock_client_id, .. } | LockState::Locking { client_id: lock_client_id, .. } if client_id == *lock_client_id)
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

            let previous_focus = self.core.seat.get_keyboard().and_then(|keyboard| keyboard.current_focus());
            if let Some(keyboard) = self.core.seat.get_keyboard() {
                keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
            }

            let pending_outputs = self
                .core
                .outputs_config
                .outputs()
                .into_iter()
                .map(|(_, output)| output.downgrade())
                .collect::<Vec<_>>();
            if !pending_outputs.is_empty() {
                self.core.protocol_delegates.ext_session_lock_state.state = LockState::Locking {
                    locker: confirmation,
                    client_id,
                    previous_focus,
                    pending_outputs,
                };
            } else {
                self.core.protocol_delegates.ext_session_lock_state.state = LockState::Locked { client_id, previous_focus };
                confirmation.lock();
            }

            self.schedule_render();
        }
    }

    fn new_surface(&mut self, surface: LockSurface, wl_output: WlOutput) {
        // We don't need to check on the locking client or ExtSessionLockV1 instance here, because
        // we always accept the first lock request we get, and (while locking or locked) drop the
        // rest immediately, which will cause smithay to stop sending us surfaces created by other
        // clients/instances.
        if matches!(
            self.core.protocol_delegates.ext_session_lock_state.state,
            LockState::Locking { .. } | LockState::Locked { .. }
        ) && let Some(output) = Output::from_resource(&wl_output)
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
                .insert(output.clone(), surface);
            self.schedule_render_output(&output);

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

            self.schedule_render();
        }
    }
}
