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

use smithay::{
    backend::drm::DrmNode,
    output::Output,
    reexports::drm::control::{Device, crtc::Handle as CrtcHandle},
};

use crate::backend::udev::UdevData;

#[derive(Debug, Clone, PartialEq)]
pub struct UdevGammaControlData {
    pub drm_node: DrmNode,
    pub crtc: CrtcHandle,
}

impl UdevData {
    pub fn set_output_gamma_real(
        &mut self,
        _output: Output,
        data: &UdevGammaControlData,
        red: &[u16],
        green: &[u16],
        blue: &[u16],
    ) -> bool {
        if let Some(backend_data) = self.backends.get_mut(&data.drm_node) {
            let device = backend_data.drm_output_manager.device();
            if let Err(err) = device.set_gamma(data.crtc, red, green, blue) {
                tracing::info!("Failed to set device gamma ramps: {err}");
                false
            } else {
                true
            }
        } else {
            false
        }
    }
}
