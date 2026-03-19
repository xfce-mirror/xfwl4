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
    reexports::wayland_server::{Client, DisplayHandle},
};

use crate::protocols::output_management::{
    wlr_output_management::{WlrOutputManagementHandler, WlrOutputManagementState},
    xfce_output_management::{XfceOutputManagementHandler, XfceOutputManagementState},
};

pub mod wlr_output_management;
pub mod xfce_output_management;

pub struct OutputManagementState {
    wlr: WlrOutputManagementState,
    xfce: XfceOutputManagementState,
}

impl OutputManagementState {
    pub fn new<H, F>(dh: &DisplayHandle, filter: F) -> Self
    where
        H: WlrOutputManagementHandler + XfceOutputManagementHandler,
        F: for<'c> Fn(&'c Client) -> bool + Clone + Send + Sync + 'static,
    {
        Self {
            wlr: WlrOutputManagementState::new::<H, _>(dh, filter.clone()),
            xfce: XfceOutputManagementState::new::<H, _>(dh, filter),
        }
    }

    pub fn wlr_output_management_state(&mut self) -> &mut WlrOutputManagementState {
        &mut self.wlr
    }

    pub fn xfce_output_management_state(&mut self) -> &mut XfceOutputManagementState {
        &mut self.xfce
    }

    pub fn output_created<H: WlrOutputManagementHandler + XfceOutputManagementHandler>(&mut self, output: &Output, edid: Bytes) {
        self.wlr.output_created::<H>(output);
        self.xfce.output_created::<H>(output, edid);
    }

    pub fn output_changed<H: WlrOutputManagementHandler + XfceOutputManagementHandler>(&mut self, output: &Output, is_enabled: bool) {
        self.wlr.output_changed::<H>(output, is_enabled);
    }

    pub fn output_destroyed(&mut self, output: &Output) {
        self.wlr.output_destroyed(output);
        self.xfce.output_destroyed(output);
    }
}
