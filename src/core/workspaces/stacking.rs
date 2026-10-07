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

//! This file is concerned with persisting stacking order for session management data.  This is
//! done by giving each window an ordered serial number.  Each serial is spaced out: the upper bits
//! are used to space new windows, and the lower bits are used when windows are restacked in
//! between existing windows.  The idea here is to change as few serial numbers as possible when
//! adding or moving a window to any point of the stack, because the serials need to be sent to the
//! session manager over dbus whenever they change.  Ideally, a restack of one window just changes
//! that window's serial number.
//!
//! Eventually, though, we'll run out of space somewhere, and can't place window between two
//! neighboring windows without a serial conflict, so we have to re-space.

use std::{
    cell::{Cell, Ref, RefCell, RefMut},
    collections::HashMap,
    ops::BitOr,
};

use crate::{
    backend::Backend,
    core::{
        shell::{WindowElement, WorkspaceLocation},
        state::Xfwl4Core,
    },
};

/// Default serial for the first window to be placed in a workspace
const DEFAULT_SERIAL: u64 = u64::MAX / 2;
/// Initial gap between serials when placing adjacent windows
const SERIAL_GAP: u64 = 2u64.pow(32);

pub struct StackingSerialsRestoreGuard(WindowElement);

#[derive(Debug, Clone, PartialEq)]
enum SerialUpdateResult {
    /// No changes were made for this window on any workspace
    NoChanges,
    /// Only this window was updated
    UpdatedThisWindow,
    /// List of workspaces where all windows were updated, so that they can be marked dirty.
    UpdatedAllWindowsOnWorkspaces(Vec<u64>),
}

// workspace ID -> stacking serial
#[derive(Default)]
struct StackingSerials {
    restoring: Cell<bool>,
    workspace_serials: RefCell<HashMap<u64, u64>>,
}

impl<BackendData: Backend + 'static> Xfwl4Core<BackendData> {
    #[must_use]
    pub(in crate::core) fn restore_window_stacking_serials(
        &mut self,
        window: &WindowElement,
        saved_serials: &HashMap<u64, u64>,
    ) -> (StackingSerialsRestoreGuard, bool) {
        let guard = window.start_stacking_restore();

        for (workspace_id, serial) in saved_serials {
            let (workspace_id, target_serial) = (*workspace_id, *serial);

            if let Some(workspace) = self
                .workspace_manager
                .workspace_index_for_id(workspace_id)
                .and_then(|index| self.workspace_manager.workspaces_mut().get_mut(index as usize))
            {
                let (windows_below, windows_above) = workspace.visible_windows().partition::<Vec<_>, _>(|window| {
                    window
                        .stacking_serials()
                        .get(&workspace_id)
                        .is_some_and(|serial| *serial < target_serial)
                });

                let serial_collision = windows_above.first().is_some_and(|window| {
                    window
                        .stacking_serials()
                        .get(&workspace_id)
                        .is_some_and(|serial| *serial == target_serial)
                });

                // This is one of the very *very* few times it is not only safe, but actually
                // correct, to bypass the wrappers on `Xfwl4State` in `mod.rs`.  We do want to
                // avoid those functions restacking parent/child trees when restoring a window's
                // stacking position, and we want to avoid them rewriting the window stacking
                // serials.
                //
                // The one problem is that we need to be careful in future updates to those
                // functions: if they end up taking on more responsibilities, not calling them from
                // here might have consequences.
                if let Some(reference_window) = windows_below.into_iter().last().cloned() {
                    workspace.raise_window_above(window, &reference_window, false);
                } else {
                    workspace.lower_window(window);
                }

                if !serial_collision {
                    window.stacking_serials_mut().insert(workspace_id, target_serial);
                }
            }
        }

        // We need to run the update because it's possible the restored serial collided with an
        // existing one, and we need to allocate new serials.
        let serial_changes = self.update_window_stacking_serial(window);

        (guard, serial_changes)
    }

    pub(in crate::core) fn update_window_stacking_serial(&self, window: &WindowElement) -> bool {
        match self.update_window_stacking_serial_internal(window) {
            SerialUpdateResult::NoChanges => false,
            SerialUpdateResult::UpdatedThisWindow => {
                window.set_session_state_dirty(true);
                true
            }
            SerialUpdateResult::UpdatedAllWindowsOnWorkspaces(workspace_ids) => {
                let windows = workspace_ids.into_iter().flat_map(|workspace_id| {
                    self.workspace_manager
                        .workspace_index_for_id(workspace_id)
                        .and_then(|workspace_index| self.workspace_manager.workspaces().get(workspace_index as usize))
                        .map(|workspace| workspace.visible_windows().collect::<Vec<_>>())
                        .unwrap_or_default()
                });
                for window in windows {
                    window.set_session_state_dirty(true);
                }
                true
            }
        }
    }

    fn update_window_stacking_serial_internal(&self, window: &WindowElement) -> SerialUpdateResult {
        if window.minimized() {
            let mut serials = window.stacking_serials_mut();
            if serials.is_empty() {
                SerialUpdateResult::NoChanges
            } else {
                serials.clear();
                SerialUpdateResult::UpdatedThisWindow
            }
        } else {
            let workspace_ids_and_windows = match window.props().workspace_loc {
                WorkspaceLocation::Single(index) => {
                    if let Some(workspace_id_and_windows) = self
                        .workspace_manager
                        .workspaces()
                        .get(index as usize)
                        .map(|workspace| (workspace.id(), workspace.visible_windows().cloned().collect::<Vec<_>>()))
                    {
                        vec![workspace_id_and_windows]
                    } else {
                        tracing::warn!("BUG: window claims to be on workspace index {index}, but that workspace doesn't exist");
                        vec![]
                    }
                }

                WorkspaceLocation::All => self
                    .workspace_manager
                    .workspaces()
                    .iter()
                    .map(|workspace| (workspace.id(), workspace.visible_windows().cloned().collect()))
                    .collect::<Vec<_>>(),
            };

            let is_restoring = window.is_restoring();

            let changes = workspace_ids_and_windows
                .into_iter()
                .fold(SerialUpdateResult::NoChanges, |changes, (workspace_id, windows)| {
                    if let Some(window_pos) = windows.iter().position(|other| other == window) {
                        let old_serial = window.stacking_serials().get(&workspace_id).copied();
                        let above_serial = windows
                            .get(window_pos + 1)
                            .and_then(|above| above.stacking_serials().get(&workspace_id).copied());
                        let below_serial = window_pos
                            .checked_sub(1)
                            .and_then(|below_pos| windows.get(below_pos))
                            .and_then(|below| below.stacking_serials().get(&workspace_id).copied());

                        // This is `Result<Option<u64>, ()>`.  Meanings:
                        //
                        // - `Ok(Some(T))`: new serial is needed
                        // - `Ok(None)`: new serial is not needed
                        // - `Err(())`: new serial could not be generated; need to re-space
                        //
                        let new_serial = if !is_restoring || old_serial.is_none() {
                            match (old_serial, above_serial, below_serial) {
                                (None, None, None) => Ok(Some(DEFAULT_SERIAL)),
                                (Some(_), None, None) => Ok(None),

                                (Some(old_serial), None, Some(below_serial)) if old_serial > below_serial => Ok(None),
                                (_, None, Some(below_serial)) => below_serial.checked_add(SERIAL_GAP).ok_or(()).map(Some),

                                (Some(old_serial), Some(above_serial), None) if old_serial < above_serial => Ok(None),
                                (_, Some(above_serial), None) => above_serial.checked_sub(SERIAL_GAP).ok_or(()).map(Some),

                                (Some(old_serial), Some(above_serial), Some(below_serial))
                                    if old_serial > below_serial && old_serial < above_serial =>
                                {
                                    Ok(None)
                                }
                                (_, Some(above_serial), Some(below_serial)) if above_serial - below_serial > 1 => {
                                    Ok(Some(below_serial + (above_serial - below_serial) / 2))
                                }

                                (_, Some(_), Some(_)) => Err(()),
                            }
                        } else {
                            // If we are restoring and `old_serial` exists, then that's really
                            // definitely the serial we want to use, so don't allocate a new one.
                            Ok(None)
                        };

                        let new_changes = match new_serial {
                            Ok(Some(new_serial)) => {
                                let mut serials = window.stacking_serials_mut();
                                let old_serial = serials.insert(workspace_id, new_serial);
                                old_serial.is_none_or(|old_serial| old_serial != new_serial).into()
                            }

                            Ok(None) => SerialUpdateResult::NoChanges,

                            Err(_) => {
                                // We have no room either above the old topmost window, below the old
                                // bottommost window, or between its neighbors, so just re-serial everything.
                                self.reserial_windows_for_workspace(workspace_id, &windows);
                                SerialUpdateResult::UpdatedAllWindowsOnWorkspaces(vec![workspace_id])
                            }
                        };

                        changes | new_changes
                    } else {
                        tracing::warn!("BUG: window not found in workspace with ID {workspace_id}");
                        changes
                    }
                });

            if let WorkspaceLocation::Single(index) = window.props().workspace_loc
                && let Some(workspace_id) = self
                    .workspace_manager
                    .workspaces()
                    .get(index as usize)
                    .map(|workspace| workspace.id())
            {
                // Handle a sticky -> not sticky transition, or a move to a different workspace.
                let mut serials = window.stacking_serials_mut();
                if serials.len() > 1 {
                    serials.retain(|id, _| *id == workspace_id);
                    changes | SerialUpdateResult::UpdatedThisWindow
                } else {
                    changes
                }
            } else {
                changes
            }
        }
    }

    fn reserial_windows_for_workspace(&self, workspace_id: u64, windows: &[WindowElement]) {
        let (lower, upper) = windows.split_at(windows.len() / 2);
        let mut lower_iter = lower.iter().rev().cloned();
        let mut upper_iter = upper.iter();

        if let Some(center) = upper_iter.next() {
            center.stacking_serials_mut().insert(workspace_id, DEFAULT_SERIAL);
        }

        for serial_diff in (SERIAL_GAP..u64::MAX).step_by(SERIAL_GAP as usize) {
            let mut inserted_any = false;

            if let Some(lower) = lower_iter.next() {
                lower.stacking_serials_mut().insert(workspace_id, DEFAULT_SERIAL - serial_diff);
                inserted_any = true;
            }

            if let Some(upper) = upper_iter.next() {
                upper.stacking_serials_mut().insert(workspace_id, DEFAULT_SERIAL + serial_diff);
                inserted_any = true;
            }

            if !inserted_any {
                break;
            }
        }
    }
}

impl WindowElement {
    fn start_stacking_restore(&self) -> StackingSerialsRestoreGuard {
        let data = self.0.user_data().get_or_insert(StackingSerials::default);
        data.workspace_serials.borrow_mut().clear();
        data.restoring.set(true);
        StackingSerialsRestoreGuard(self.clone())
    }

    fn stacking_restore_complete(&self) {
        self.0.user_data().get_or_insert(StackingSerials::default).restoring.set(false);
    }

    fn is_restoring(&self) -> bool {
        self.0.user_data().get_or_insert(StackingSerials::default).restoring.get()
    }

    pub fn stacking_serials(&self) -> Ref<'_, HashMap<u64, u64>> {
        self.0
            .user_data()
            .get_or_insert(StackingSerials::default)
            .workspace_serials
            .borrow()
    }

    fn stacking_serials_mut(&self) -> RefMut<'_, HashMap<u64, u64>> {
        self.0
            .user_data()
            .get_or_insert(StackingSerials::default)
            .workspace_serials
            .borrow_mut()
    }
}

impl Drop for StackingSerialsRestoreGuard {
    fn drop(&mut self) {
        self.0.stacking_restore_complete();
    }
}

impl From<bool> for SerialUpdateResult {
    fn from(value: bool) -> Self {
        if value { Self::UpdatedThisWindow } else { Self::NoChanges }
    }
}

impl BitOr for SerialUpdateResult {
    type Output = SerialUpdateResult;

    fn bitor(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Self::UpdatedAllWindowsOnWorkspaces(mut a), Self::UpdatedAllWindowsOnWorkspaces(b)) => {
                a.extend(b);
                Self::UpdatedAllWindowsOnWorkspaces(a)
            }
            (lhs @ Self::UpdatedAllWindowsOnWorkspaces(_), _) => lhs,
            (_, rhs @ Self::UpdatedAllWindowsOnWorkspaces(_)) => rhs,
            (Self::NoChanges, rhs) => rhs,
            (lhs, Self::NoChanges) => lhs,
            (Self::UpdatedThisWindow, Self::UpdatedThisWindow) => Self::UpdatedThisWindow,
        }
    }
}
