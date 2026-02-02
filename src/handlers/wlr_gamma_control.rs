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

use smithay::output::Output;

use crate::{
    Xfwl4State,
    backend::Backend,
    protocols::wlr_gamma_control::{WlrGammaControlHandler, WlrGammaControlState, delegate_wlr_gamma_control},
};

impl<BackendData: Backend + 'static> WlrGammaControlHandler for Xfwl4State<BackendData> {
    type GammaControlData = BackendData::GammaControlData;

    fn wlr_gamma_control_state(&mut self) -> &mut WlrGammaControlState<Self> {
        &mut self.wlr_gamma_control_state
    }

    fn set_output_gamma(
        &mut self,
        output: Output,
        backend_data: &Self::GammaControlData,
        red: &[u16],
        green: &[u16],
        blue: &[u16],
    ) -> bool {
        self.backend_data.set_output_gamma(output, backend_data, red, green, blue)
    }
}

delegate_wlr_gamma_control!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
