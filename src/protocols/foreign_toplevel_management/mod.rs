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

use std::sync::Arc;

use smithay::{
    output::Output,
    reexports::{
        wayland_protocols_wlr::foreign_toplevel::v1::server::{
            zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1, zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
        },
        wayland_server::{Client, Dispatch, DisplayHandle, GlobalDispatch},
    },
};

use crate::{
    core::shell::WindowState,
    protocols::{
        ext_workspace::{ExtWorkspaceHandler, ExtWorkspaceState},
        foreign_toplevel_management::{
            wlr_foreign_toplevel_management::{
                WlrForeignToplevelHandler, WlrForeignToplevelManagementGlobalData, WlrForeignToplevelManagementState,
            },
            xfce_foreign_toplevel_management::{
                IconSize, XfceForeignToplevelHandler, XfceForeignToplevelManagementGlobalData, XfceForeignToplevelManagementState,
                proto::xfce_foreign_toplevel_manager_private_v1::XfceForeignToplevelManagerPrivateV1,
            },
        },
    },
};

pub mod wlr_foreign_toplevel_management;
pub mod xfce_foreign_toplevel_management;

pub struct ForeignToplevelManagementState {
    wlr: WlrForeignToplevelManagementState,
    xfce: XfceForeignToplevelManagementState,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ToplevelId(String);

#[derive(Clone)]
pub struct ToplevelHandleData(Arc<ToplevelId>);

pub struct ToplevelCreatedInput {
    pub title: String,
    pub app_id: String,
    pub state: WindowState,
    pub outputs: Vec<Output>,
    pub parent: Option<ToplevelId>,
    pub workspace_id: Option<String>,
    pub icon_name: Option<String>,
    pub icon_sizes: Vec<IconSize>,
}

pub struct ToplevelChangedInput {
    pub title: Option<String>,
    pub app_id: Option<String>,
    pub state: WindowState,
    pub outputs_added: Vec<Output>,
    pub outputs_removed: Vec<Output>,
    pub parent: Option<Option<ToplevelId>>,
    pub workspace_id: Option<Option<String>>,
    pub icon_name: Option<Option<String>>,
    pub icon_sizes: Option<Vec<IconSize>>,
}

impl ForeignToplevelManagementState {
    pub fn new<H, F>(dh: &DisplayHandle, filter: F) -> Self
    where
        H: WlrForeignToplevelHandler
            + GlobalDispatch<ZwlrForeignToplevelManagerV1, WlrForeignToplevelManagementGlobalData>
            + XfceForeignToplevelHandler
            + GlobalDispatch<XfceForeignToplevelManagerPrivateV1, XfceForeignToplevelManagementGlobalData>,
        F: for<'c> Fn(&'c Client) -> bool + Clone + Send + Sync + 'static,
    {
        Self {
            wlr: WlrForeignToplevelManagementState::new::<H, _>(dh, filter.clone()),
            xfce: XfceForeignToplevelManagementState::new::<H, _>(dh, filter),
        }
    }

    pub fn toplevel_created<H>(&mut self, input: ToplevelCreatedInput) -> ToplevelId
    where
        H: WlrForeignToplevelHandler + Dispatch<ZwlrForeignToplevelHandleV1, ToplevelHandleData>,
    {
        let toplevel_id = self
            .wlr
            .toplevel_created::<H>(input.title, input.app_id, input.state, input.outputs, input.parent);
        self.xfce.toplevel_created(
            Arc::clone(&toplevel_id),
            input.state,
            input.workspace_id,
            input.icon_name,
            input.icon_sizes,
        );
        toplevel_id.as_ref().clone()
    }

    pub fn toplevel_changed<D: ExtWorkspaceHandler>(
        &mut self,
        toplevel_id: &ToplevelId,
        input: ToplevelChangedInput,
        workspace_state: &ExtWorkspaceState<D>,
    ) {
        let mut changes_sent = self.wlr.toplevel_changed(
            toplevel_id,
            input.title,
            input.app_id,
            input.state,
            input.outputs_added,
            input.outputs_removed,
            input.parent,
        );
        changes_sent |= self.xfce.toplevel_changed(
            workspace_state,
            toplevel_id,
            input.state,
            input.workspace_id,
            input.icon_name,
            input.icon_sizes,
        );

        if changes_sent {
            self.wlr.send_done(toplevel_id);
        }
    }

    pub fn toplevel_destroyed(&mut self, toplevel_id: &ToplevelId) {
        self.xfce.toplevel_destroyed(toplevel_id);
        self.wlr.toplevel_destroyed(toplevel_id);
    }

    pub fn flush_client_workspace_events<D: ExtWorkspaceHandler>(&mut self, ext_workspace_state: &ExtWorkspaceState<D>, client: &Client) {
        self.xfce.flush_client_workspace_events(ext_workspace_state, client);
    }

    pub fn wlr_state(&mut self) -> &mut WlrForeignToplevelManagementState {
        &mut self.wlr
    }

    pub fn xfce_state(&mut self) -> &mut XfceForeignToplevelManagementState {
        &mut self.xfce
    }
}
