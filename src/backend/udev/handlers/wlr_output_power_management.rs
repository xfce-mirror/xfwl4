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

use anyhow::anyhow;
use smithay::{
    output::Output,
    reexports::{
        calloop::timer::{TimeoutAction, Timer},
        drm::control::{Device as ControlDevice, connector, property},
        wayland_protocols_wlr::output_power_management::v1::server::zwlr_output_power_v1::Mode as PowerMode,
    },
};

use crate::{
    backend::udev::{UdevData, UdevOutputId, udev_do_render},
    core::state::Xfwl4State,
    protocols::wlr_output_power_management::{WlrOutputPowerError, WlrOutputPowerManagementHandler, WlrOutputPowerManagementState},
};

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u64)]
enum DrmDpmsMode {
    On = 0,
    #[allow(unused)]
    Standby = 1,
    #[allow(unused)]
    Suspend = 2,
    Off = 3,
}

impl WlrOutputPowerManagementHandler for Xfwl4State<UdevData> {
    fn wlr_output_power_management_state(&mut self) -> &mut WlrOutputPowerManagementState {
        &mut self.backend.wlr_output_power_management_state
    }

    fn on_set_mode(&mut self, output: &Output, new_mode: PowerMode) -> Result<(), WlrOutputPowerError> {
        if let Some(udev_data) = output.user_data().get::<UdevOutputId>()
            && let Some(backend_data) = self.backend.backends.get_mut(&udev_data.device_id)
            && let Some(surface) = backend_data.surfaces.get_mut(&udev_data.crtc)
        {
            let res = match new_mode {
                PowerMode::Off => {
                    if let Some(drm_output) = surface.drm_output.as_ref() {
                        let device = backend_data.drm_output_manager.device();
                        let res = if let Err(err) = set_legacy_dpms(device, surface.connector, DrmDpmsMode::Off) {
                            tracing::info!(
                                "Failed to power down output '{}' using DPMS; shutting down CRTCs instead ({err})",
                                output.name()
                            );
                            if let Err(err) = drm_output.with_compositor(|compositor| compositor.clear()) {
                                tracing::warn!("Failed to shut down CRTCs:wp for output '{}': {err}", output.name());
                                Err(WlrOutputPowerError::TransientFailure)
                            } else {
                                Ok(())
                            }
                        } else {
                            Ok(())
                        };

                        if res.is_ok()
                            && let Some(repaint_timeout) = surface.repaint_timeout.take()
                        {
                            self.core.unregister_timer(repaint_timeout);
                        }

                        res
                    } else {
                        Err(WlrOutputPowerError::OutputNotFound)
                    }
                }
                PowerMode::On => {
                    let device = backend_data.drm_output_manager.device();
                    if let Err(err) = set_legacy_dpms(device, surface.connector, DrmDpmsMode::On) {
                        tracing::error!("Failed to power up output '{}' using DPMS: {err}", output.name());
                    }

                    if surface.repaint_timeout.is_none() {
                        let output = surface.output.clone();
                        let scanout_node = udev_data.device_id;
                        let crtc = udev_data.crtc;
                        let token = self.core.register_timer(Timer::immediate(), move |state| {
                            udev_do_render(state, &output, scanout_node, crtc, state.core.now());
                            TimeoutAction::Drop
                        });
                        surface.repaint_timeout = Some(token);
                    }
                    Ok(())
                }
                _ => Ok(()),
            };

            if res.is_ok() {
                self.wlr_output_power_management_state().output_changed(output, new_mode);
            }

            res
        } else {
            Err(WlrOutputPowerError::OutputNotFound)
        }
    }
}

fn set_legacy_dpms<C: ControlDevice>(device: &C, connector: connector::Handle, mode: DrmDpmsMode) -> anyhow::Result<()> {
    let props = device.get_properties(connector)?;
    let (handles, _) = props.as_props_and_values();

    let handle = handles.iter().try_fold(None::<property::Handle>, |found_handle, handle| {
        if found_handle.is_some() {
            Ok::<_, std::io::Error>(found_handle)
        } else {
            let info = device.get_property(*handle)?;
            Ok(info.name().to_str().ok().filter(|x| *x == "DPMS").map(|_| *handle))
        }
    })?;

    if let Some(handle) = handle {
        device.set_property(connector, handle, mode as u64)?;

        let props = device.get_properties(connector)?;
        let (handles, values) = props.as_props_and_values();
        let dpms_value = handles.iter().zip(values).find(|(h, _)| **h == handle).map(|(_, v)| *v);
        dpms_value
            .ok_or_else(|| anyhow!("Failed to find DPMS property after setting it"))
            .and_then(|set_value| {
                if set_value == mode as u64 {
                    Ok(())
                } else {
                    Err(anyhow!("DPMS setting appeared to succeed, but failed on readback"))
                }
            })
    } else {
        Err(anyhow!("DPMS property not found on connector"))
    }
}
