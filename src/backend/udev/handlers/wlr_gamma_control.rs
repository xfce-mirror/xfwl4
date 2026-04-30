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

use smithay::{output::Output, reexports::drm::control::Device};

use crate::{
    backend::udev::{UdevData, UdevOutputId},
    core::state::Xfwl4State,
    protocols::wlr_gamma_control::{WlrGammaControlHandler, WlrGammaControlState},
};

impl WlrGammaControlHandler for Xfwl4State<UdevData> {
    fn wlr_gamma_control_state(&mut self) -> &mut WlrGammaControlState {
        &mut self.backend.wlr_gamma_control_state
    }

    fn set_output_gamma(&mut self, output: &Output, red: &[u16], green: &[u16], blue: &[u16]) -> bool {
        if let Some(data) = output.user_data().get::<UdevOutputId>() {
            if let Some(backend_data) = self.backend.backends.get_mut(&data.device_id) {
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
        } else {
            false
        }
    }
}
