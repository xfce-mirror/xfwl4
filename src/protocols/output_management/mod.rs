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

use bytes::Bytes;
use smithay::{
    output::Output,
    reexports::wayland_protocols_wlr::output_management::v1::server::{
        zwlr_output_head_v1::ZwlrOutputHeadV1, zwlr_output_manager_v1::ZwlrOutputManagerV1, zwlr_output_mode_v1::ZwlrOutputModeV1,
    },
    reexports::wayland_server::{Client, Dispatch, DisplayHandle, GlobalDispatch},
};

use crate::protocols::GlobalData;
use crate::protocols::output_management::{
    wlr_output_management::{WlrOutputManagementGlobalData, WlrOutputManagementHandler, WlrOutputManagementState},
    xfce_output_management::{
        XfceOutputManagementGlobalData, XfceOutputManagementHandler, XfceOutputManagementState,
        proto::{xfce_output_head_private_v1::XfceOutputHeadPrivateV1, xfce_output_manager_private_v1::XfceOutputManagerPrivateV1},
    },
};

pub mod wlr_output_management;
pub mod xfce_output_management;

pub struct OutputManagementState {
    wlr_state: WlrOutputManagementState,
    xfce_state: XfceOutputManagementState,
}

impl OutputManagementState {
    pub fn new<H, F>(dh: &DisplayHandle, filter: F) -> Self
    where
        H: WlrOutputManagementHandler
            + XfceOutputManagementHandler
            + GlobalDispatch<ZwlrOutputManagerV1, WlrOutputManagementGlobalData>
            + GlobalDispatch<XfceOutputManagerPrivateV1, XfceOutputManagementGlobalData>,
        F: for<'c> Fn(&'c Client) -> bool + Clone + Send + Sync + 'static,
    {
        Self {
            wlr_state: WlrOutputManagementState::new::<H, _>(dh, filter.clone()),
            xfce_state: XfceOutputManagementState::new::<H, _>(dh, filter),
        }
    }

    pub fn wlr_output_management_state(&mut self) -> &mut WlrOutputManagementState {
        &mut self.wlr_state
    }

    pub fn xfce_output_management_state(&mut self) -> &mut XfceOutputManagementState {
        &mut self.xfce_state
    }

    pub fn output_created<H>(&mut self, output: &Output, edid: Bytes)
    where
        H: WlrOutputManagementHandler
            + XfceOutputManagementHandler
            + Dispatch<ZwlrOutputHeadV1, GlobalData>
            + Dispatch<ZwlrOutputModeV1, GlobalData>
            + Dispatch<XfceOutputHeadPrivateV1, GlobalData>,
    {
        let serial = self.wlr_state.output_created::<H>(output);
        self.xfce_state.output_created::<H>(output, edid, serial);
    }

    pub fn output_changed<H>(&mut self, output: &Output, is_enabled: bool)
    where
        H: WlrOutputManagementHandler + XfceOutputManagementHandler + Dispatch<ZwlrOutputModeV1, GlobalData>,
    {
        let serial = self.wlr_state.output_changed::<H>(output, is_enabled);
        self.xfce_state.output_changed::<H>(output, is_enabled, serial);
    }

    pub fn output_destroyed(&mut self, output: &Output) {
        let serial = self.wlr_state.output_destroyed(output);
        self.xfce_state.output_destroyed(output, serial);
    }
}
