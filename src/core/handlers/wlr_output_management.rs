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

use crate::{
    backend::Backend,
    core::{
        config::{OutputConfigChange, scale_from_fractional},
        state::Xfwl4State,
    },
    protocols::wlr_output_management::{
        ConfiguredMode, OutputConfigurationUpdate, WlrOutputConfiguration, WlrOutputManagementHandler, WlrOutputManagementState,
        delegate_wlr_output_management,
    },
};

impl<BackendData: Backend + 'static> WlrOutputManagementHandler for Xfwl4State<BackendData> {
    fn wlr_output_management_state(&mut self) -> &mut WlrOutputManagementState {
        &mut self.wlr_output_management_state
    }

    fn on_test_configuration(&mut self, configuration: WlrOutputConfiguration) {
        tracing::debug!("test configuration {configuration:?}");

        if configuration
            .updates()
            .iter()
            .any(|update| matches!(update, OutputConfigurationUpdate::Enable(head) if head.adaptive_sync().is_some()))
        {
            configuration.send_failed();
        } else {
            configuration.send_succeeded();
        }
    }

    fn on_apply_configuration(&mut self, configuration: WlrOutputConfiguration) {
        tracing::debug!("apply configuration {configuration:?}");

        let mut failed = false;
        for update in configuration.updates() {
            if let Some((output, config_change)) = match update {
                OutputConfigurationUpdate::Enable(head) => head.output().map(|output| {
                    (
                        output,
                        OutputConfigChange {
                            current_mode: head.mode().map(|mode| {
                                Some(match mode {
                                    ConfiguredMode::Advertised(mode) => mode,
                                    ConfiguredMode::Custom { width, height, refresh } => smithay::output::Mode {
                                        size: (width, height).into(),
                                        refresh,
                                    },
                                })
                            }),
                            scale: head.scale().map(scale_from_fractional),
                            transform: head.transform(),
                            location: head.position(),
                            preferred_mode: None,
                        },
                    )
                }),
                OutputConfigurationUpdate::Disable(output) => output.upgrade().map(|output| (output, OutputConfigChange::new_disabled())),
            } && let Err(err) = self.apply_output_config_change(&output, config_change)
            {
                // TODO: roll back any prior successful updates
                tracing::warn!("Failed to apply output config change to output {}: {err}", output.name());
                failed = true;
                break;
            }
        }

        if !failed {
            configuration.send_succeeded();
        } else {
            configuration.send_failed();
        }
    }
}

delegate_wlr_output_management!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
