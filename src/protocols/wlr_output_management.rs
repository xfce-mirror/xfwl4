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
    output::{Mode, Output, WeakOutput},
    reexports::{
        wayland_protocols_wlr::output_management::v1::server::{
            zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1,
            zwlr_output_configuration_v1::ZwlrOutputConfigurationV1,
            zwlr_output_head_v1::{
                AdaptiveSyncState, EVT_ADAPTIVE_SYNC_SINCE, EVT_MAKE_SINCE, EVT_MODEL_SINCE, EVT_SERIAL_NUMBER_SINCE, ZwlrOutputHeadV1,
            },
            zwlr_output_manager_v1::ZwlrOutputManagerV1,
            zwlr_output_mode_v1::ZwlrOutputModeV1,
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
            backend::{ClientId, GlobalId},
        },
    },
    utils::{Logical, Point, SERIAL_COUNTER, Serial, Transform},
};

pub struct OutputDiff {
    enabled: Option<bool>,
    modes_added: Vec<Mode>,
    modes_removed: Vec<Mode>,
    current_mode: Option<Option<Mode>>,
    preferred_mode: Option<Option<Mode>>,
    position: Option<Point<i32, Logical>>,
    transform: Option<Transform>,
    scale: Option<f64>,
    adaptive_sync: Option<AdaptiveSyncState>,
}

impl OutputDiff {
    fn new(output: &Output, is_enabled: bool, head: &WlrHead) -> Option<Self> {
        let mut modes_added = output.modes();
        modes_added.retain(|new_mode| !head.modes.iter().any(|old_mode| old_mode.mode == *new_mode));

        let mut modes_removed = head.modes.iter().map(|mode| mode.mode).collect::<Vec<_>>();
        modes_removed.retain(|old_mode| !output.modes().iter().any(|new_mode| old_mode == new_mode));

        let enabled = is_enabled && output.current_mode().is_some();

        let preferred_mode = output.preferred_mode().or_else(|| output.modes().first().cloned());

        let diff = OutputDiff {
            enabled: (head.last_is_enabled != enabled).then_some(enabled),
            modes_added,
            modes_removed,
            current_mode: (head.last_current_mode != output.current_mode()).then(|| output.current_mode()),
            preferred_mode: (head.last_preferred_mode != preferred_mode).then_some(preferred_mode),
            position: (head.last_position != output.current_location()).then(|| output.current_location()),
            transform: (head.last_transform != output.current_transform()).then(|| output.current_transform()),
            scale: (head.last_scale != output.current_scale().fractional_scale()).then(|| output.current_scale().fractional_scale()),
            adaptive_sync: (head.last_adaptive_sync != AdaptiveSyncState::Disabled).then_some(AdaptiveSyncState::Disabled),
        };

        if diff.enabled.is_some()
            || !diff.modes_added.is_empty()
            || !diff.modes_removed.is_empty()
            || diff.current_mode.is_some()
            || diff.preferred_mode.is_some()
            || diff.position.is_some()
            || diff.transform.is_some()
            || diff.scale.is_some()
            || diff.adaptive_sync.is_some()
        {
            Some(diff)
        } else {
            None
        }
    }
}

pub struct WlrOutputManagementState {
    dh: DisplayHandle,
    _global: GlobalId,
    cur_config_serial: Serial,
    manager_instances: Vec<ZwlrOutputManagerV1>,
    heads: Vec<WlrHead>,
    configurations: Vec<WlrOutputConfiguration>,
}

impl WlrOutputManagementState {
    pub fn new<H, F>(dh: &DisplayHandle, filter: F) -> Self
    where
        H: WlrOutputManagementHandler,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global = dh.create_global::<H, ZwlrOutputManagerV1, _>(4, Box::new(filter));
        Self {
            dh: dh.clone(),
            _global: global,
            cur_config_serial: SERIAL_COUNTER.next_serial(),
            manager_instances: Vec::new(),
            heads: Vec::new(),
            configurations: Vec::new(),
        }
    }

    pub fn output_created<H: WlrOutputManagementHandler>(&mut self, output: &Output) {
        self.cur_config_serial = SERIAL_COUNTER.next_serial();

        let mut head = WlrHead {
            instances: Vec::new(),
            output: output.clone(),
            modes: output
                .modes()
                .into_iter()
                .map(|mode| WlrMode {
                    instances: Vec::new(),
                    mode,
                })
                .collect(),
            last_is_enabled: false,
            last_current_mode: output.current_mode(),
            last_preferred_mode: output.preferred_mode().or_else(|| output.modes().first().cloned()),
            last_position: output.current_location(),
            last_transform: output.current_transform(),
            last_scale: output.current_scale().fractional_scale(),
            last_adaptive_sync: AdaptiveSyncState::Disabled,
        };

        for instance in &self.manager_instances {
            if let Some(client) = instance.client() {
                if let Err(err) = send_head::<H>(&self.dh, &client, instance, &mut head) {
                    tracing::info!("Failed to send new head to client {:?}: {err}", client.id());
                }
                instance.done(self.cur_config_serial.into());
            }
        }

        self.heads.push(head);
        self.cancel_configs();
    }

    pub fn output_changed<H: WlrOutputManagementHandler>(&mut self, output: &Output, is_enabled: bool) {
        if let Some(head) = self.heads.iter_mut().find(|head| &head.output == output)
            && let Some(diff) = OutputDiff::new(output, is_enabled, head)
        {
            self.cur_config_serial = SERIAL_COUNTER.next_serial();

            if !diff.modes_added.is_empty() {
                for mode in diff.modes_added {
                    let is_current = output.current_mode().as_ref() == Some(&mode);
                    let is_preferred = output.preferred_mode().as_ref() == Some(&mode);

                    let mut mode = WlrMode {
                        instances: Vec::new(),
                        mode,
                    };

                    for instance in &head.instances {
                        if let Some(client) = instance.client()
                            && let Err(err) = send_mode::<H>(&self.dh, &client, instance, &mut mode, is_current, is_preferred)
                        {
                            tracing::info!("Failed to send new mode to client {:?}: {err}", client.id());
                        }
                    }

                    head.modes.push(mode);
                }
            }

            if !diff.modes_removed.is_empty() {
                for mode in diff.modes_removed {
                    if let Some(pos) = head.modes.iter().position(|wlr_mode| wlr_mode.mode == mode) {
                        let mode = head.modes.remove(pos);
                        for instance in mode.instances {
                            instance.finished();
                        }
                    }
                }
            }

            let current_mode = diff.current_mode.flatten().or(head.last_current_mode);
            let is_currently_enabled = diff.enabled.unwrap_or(head.last_is_enabled) && current_mode.is_some();

            if let Some(position) = diff.position {
                if head.last_is_enabled {
                    for instance in &head.instances {
                        instance.position(position.x, position.y);
                    }
                }

                head.last_position = position;
            }

            if let Some(transform) = diff.transform {
                if head.last_is_enabled {
                    let prot_transform = transform.into();
                    for instance in &head.instances {
                        instance.transform(prot_transform);
                    }
                }

                head.last_transform = transform;
            }

            if let Some(scale) = diff.scale {
                if head.last_is_enabled {
                    for instance in &head.instances {
                        instance.scale(scale);
                    }
                }

                head.last_scale = scale;
            }

            if is_currently_enabled
                && let Some(current_mode) = current_mode
                && let Some(wlr_mode) = head.modes.iter().find(|wlr_mode| wlr_mode.mode == current_mode)
            {
                let newly_enabled = diff.enabled.unwrap_or(false);
                let mode_changed = diff.current_mode.is_some();

                if newly_enabled || mode_changed {
                    for instance in &head.instances {
                        if let Some(client) = instance.client() {
                            for mode_instance in &wlr_mode.instances {
                                if mode_instance.client().as_ref() == Some(&client) {
                                    if newly_enabled {
                                        instance.enabled(1);
                                    }

                                    instance.current_mode(mode_instance);

                                    if newly_enabled {
                                        // XXX: is Logical the "global compositor space"?
                                        instance.position(head.last_position.x, head.last_position.y);
                                        instance.transform(head.last_transform.into());
                                        instance.scale(head.last_scale);
                                    }
                                }
                            }
                        }
                    }
                }

                head.last_is_enabled = true;
                head.last_current_mode = Some(current_mode);
            } else if !diff.enabled.unwrap_or(true) {
                for instance in &head.instances {
                    instance.enabled(0);
                }

                head.last_is_enabled = false;
            }

            if let Some(preferred_mode) = diff.preferred_mode {
                if let Some(preferred_mode) = preferred_mode
                    && let Some(wlr_mode) = head.modes.iter().find(|wlr_mode| wlr_mode.mode == preferred_mode)
                {
                    for instance in &head.instances {
                        if let Some(client) = instance.client() {
                            for mode_instance in &wlr_mode.instances {
                                if mode_instance.client().as_ref() == Some(&client) {
                                    mode_instance.preferred();
                                }
                            }
                        }
                    }
                }

                head.last_preferred_mode = preferred_mode;
            }

            if let Some(adaptive_sync) = diff.adaptive_sync {
                for instance in &head.instances {
                    instance.adaptive_sync(adaptive_sync);
                }

                head.last_adaptive_sync = adaptive_sync;
            }

            for instance in &self.manager_instances {
                instance.done(self.cur_config_serial.into());
            }
            self.cancel_configs();
        }
    }

    pub fn output_destroyed(&mut self, output: &Output) {
        self.cur_config_serial = SERIAL_COUNTER.next_serial();

        if let Some(pos) = self.heads.iter().position(|head| &head.output == output) {
            let head = self.heads.remove(pos);
            for instance in head.instances {
                instance.finished();
            }
        }

        for instance in &self.manager_instances {
            instance.done(self.cur_config_serial.into());
        }

        self.cancel_configs();
    }

    fn cancel_configs(&mut self) {
        for config in std::mem::take(&mut self.configurations) {
            if !config.used {
                config.instance.cancelled();
            }
        }
    }
}

struct WlrHead {
    instances: Vec<ZwlrOutputHeadV1>,
    output: Output,
    modes: Vec<WlrMode>,

    last_is_enabled: bool,
    last_current_mode: Option<Mode>,
    last_preferred_mode: Option<Mode>,
    last_position: Point<i32, Logical>,
    last_transform: Transform,
    last_scale: f64,
    last_adaptive_sync: AdaptiveSyncState,
}

struct WlrMode {
    instances: Vec<ZwlrOutputModeV1>,
    mode: Mode,
}

#[derive(Debug, Clone)]
pub enum OutputConfigurationUpdate {
    Enable(WlrOutputConfigurationHead),
    Disable(WeakOutput),
}

impl OutputConfigurationUpdate {
    fn enable_mut(&mut self) -> Option<&mut WlrOutputConfigurationHead> {
        match self {
            Self::Enable(head) => Some(head),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WlrOutputConfiguration {
    instance: ZwlrOutputConfigurationV1,
    updates: Vec<OutputConfigurationUpdate>,
    used: bool,
}

impl WlrOutputConfiguration {
    pub fn updates(&self) -> &[OutputConfigurationUpdate] {
        &self.updates
    }

    pub fn update_for_output(&self, output: &Output) -> Option<&OutputConfigurationUpdate> {
        self.updates.iter().find(|update| match update {
            OutputConfigurationUpdate::Enable(head) => &head.output == output,
            OutputConfigurationUpdate::Disable(weak_output) => weak_output == output,
        })
    }

    pub fn send_succeeded(&self) {
        self.instance.succeeded();
    }

    pub fn send_failed(&self) {
        self.instance.failed();
    }

    pub fn send_cancelled(&self) {
        self.instance.cancelled();
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConfiguredMode {
    Advertised(Mode),
    Custom { width: i32, height: i32, refresh: i32 },
}

#[derive(Debug, Clone)]
pub struct WlrOutputConfigurationHead {
    instance: ZwlrOutputConfigurationHeadV1,
    output: WeakOutput,

    mode: Option<ConfiguredMode>,
    position: Option<Point<i32, Logical>>,
    transform: Option<Transform>,
    scale: Option<f64>,
    adaptive_sync: Option<AdaptiveSyncState>,
}

impl WlrOutputConfigurationHead {
    pub fn output(&self) -> Option<Output> {
        self.output.upgrade()
    }

    pub fn mode(&self) -> Option<ConfiguredMode> {
        self.mode
    }

    pub fn position(&self) -> Option<Point<i32, Logical>> {
        self.position
    }

    pub fn transform(&self) -> Option<Transform> {
        self.transform
    }

    pub fn scale(&self) -> Option<f64> {
        self.scale
    }

    pub fn adaptive_sync(&self) -> Option<AdaptiveSyncState> {
        self.adaptive_sync
    }
}

pub trait WlrOutputManagementHandler
where
    Self: GlobalDispatch<ZwlrOutputManagerV1, Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>>
        + Dispatch<ZwlrOutputManagerV1, ()>
        + Dispatch<ZwlrOutputHeadV1, ()>
        + Dispatch<ZwlrOutputModeV1, ()>
        + Dispatch<ZwlrOutputConfigurationV1, ()>
        + Dispatch<ZwlrOutputConfigurationHeadV1, ()>
        + Sized
        + 'static,
{
    fn wlr_output_management_state(&mut self) -> &mut WlrOutputManagementState;

    fn on_test_configuration(&mut self, configuration: WlrOutputConfiguration);
    fn on_apply_configuration(&mut self, configuration: WlrOutputConfiguration);
}

impl<H: WlrOutputManagementHandler> GlobalDispatch<ZwlrOutputManagerV1, Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>, H>
    for WlrOutputManagementState
{
    fn bind(
        state: &mut H,
        handle: &DisplayHandle,
        client: &Client,
        resource: New<ZwlrOutputManagerV1>,
        _global_data: &Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
        data_init: &mut DataInit<'_, H>,
    ) {
        let instance = data_init.init(resource, ());

        let state = state.wlr_output_management_state();

        for head in state.heads.iter_mut() {
            if let Err(err) = send_head::<H>(handle, client, &instance, head) {
                tracing::info!("Failed to send head to client on new bind: {err}");
            }
        }
        instance.done(state.cur_config_serial.into());

        state.manager_instances.push(instance);
    }

    fn can_view(client: Client, global_data: &Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>) -> bool {
        global_data(&client)
    }
}

impl<H: WlrOutputManagementHandler> Dispatch<ZwlrOutputManagerV1, (), H> for WlrOutputManagementState {
    fn request(
        state: &mut H,
        client: &Client,
        resource: &ZwlrOutputManagerV1,
        request: <ZwlrOutputManagerV1 as Resource>::Request,
        data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, H>,
    ) {
        use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_manager_v1::Request;

        match request {
            Request::CreateConfiguration { id, serial } => {
                let instance = data_init.init(id, ());
                if serial != state.wlr_output_management_state().cur_config_serial.into() {
                    instance.cancelled();
                } else {
                    let config = WlrOutputConfiguration {
                        instance,
                        updates: Vec::new(),
                        used: false,
                    };
                    state.wlr_output_management_state().configurations.push(config);
                }
            }

            Request::Stop => <Self as Dispatch<ZwlrOutputManagerV1, (), H>>::destroyed(state, client.id(), resource, data),

            _ => (),
        }
    }

    fn destroyed(state: &mut H, _client: ClientId, resource: &ZwlrOutputManagerV1, _data: &()) {
        state
            .wlr_output_management_state()
            .manager_instances
            .retain(|instance| instance != resource);
    }
}

impl<H: WlrOutputManagementHandler> Dispatch<ZwlrOutputHeadV1, (), H> for WlrOutputManagementState {
    fn request(
        state: &mut H,
        client: &Client,
        resource: &ZwlrOutputHeadV1,
        request: <ZwlrOutputHeadV1 as Resource>::Request,
        data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, H>,
    ) {
        use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_head_v1::Request;

        if let Request::Release = request {
            <Self as Dispatch<ZwlrOutputHeadV1, (), H>>::destroyed(state, client.id(), resource, data)
        }
    }

    fn destroyed(state: &mut H, _client: ClientId, resource: &ZwlrOutputHeadV1, _data: &()) {
        for head in state.wlr_output_management_state().heads.iter_mut() {
            let len = head.instances.len();
            head.instances.retain(|instance| instance != resource);
            if len != head.instances.len() {
                break;
            }
        }
    }
}

impl<H: WlrOutputManagementHandler> Dispatch<ZwlrOutputModeV1, (), H> for WlrOutputManagementState {
    fn request(
        state: &mut H,
        client: &Client,
        resource: &ZwlrOutputModeV1,
        request: <ZwlrOutputModeV1 as Resource>::Request,
        data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, H>,
    ) {
        use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_mode_v1::Request;

        if let Request::Release = request {
            <Self as Dispatch<ZwlrOutputModeV1, (), H>>::destroyed(state, client.id(), resource, data)
        }
    }

    fn destroyed(state: &mut H, _client: ClientId, resource: &ZwlrOutputModeV1, _data: &()) {
        'outer: for head in state.wlr_output_management_state().heads.iter_mut() {
            for mode in head.modes.iter_mut() {
                let len = mode.instances.len();
                mode.instances.retain(|instance| instance != resource);
                if len != mode.instances.len() {
                    break 'outer;
                }
            }
        }
    }
}

impl<H: WlrOutputManagementHandler> Dispatch<ZwlrOutputConfigurationV1, (), H> for WlrOutputManagementState {
    fn request(
        state: &mut H,
        client: &Client,
        resource: &ZwlrOutputConfigurationV1,
        request: <ZwlrOutputConfigurationV1 as Resource>::Request,
        data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, H>,
    ) {
        use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_configuration_v1::{Error, Request};

        match request {
            Request::EnableHead { id, head } => {
                let instance = data_init.init(id, ());
                let state = state.wlr_output_management_state();

                if let Some(config) = state.configurations.iter_mut().find(|config| &config.instance == resource)
                    && let Some(head) = state
                        .heads
                        .iter()
                        .find(|wlr_head| wlr_head.instances.iter().any(|instance| instance == &head))
                {
                    if config.used {
                        resource.post_error(Error::AlreadyUsed, "this configuration has already been tested or applied");
                    } else if config.update_for_output(&head.output).is_some() {
                        resource.post_error(
                            Error::AlreadyConfiguredHead,
                            "enabled_head() or disable_head() already called for this head on this configuration",
                        );
                    } else {
                        let config_head = WlrOutputConfigurationHead {
                            instance,
                            output: head.output.downgrade(),
                            mode: None,
                            position: None,
                            transform: None,
                            scale: None,
                            adaptive_sync: None,
                        };
                        config.updates.push(OutputConfigurationUpdate::Enable(config_head));
                    }
                } else {
                    state.configurations.retain(|config| &config.instance != resource);
                    resource.cancelled();
                }
            }

            Request::DisableHead { head } => {
                let state = state.wlr_output_management_state();

                if let Some(config) = state.configurations.iter_mut().find(|config| &config.instance == resource)
                    && let Some(head) = state
                        .heads
                        .iter()
                        .find(|wlr_head| wlr_head.instances.iter().any(|instance| instance == &head))
                {
                    if config.used {
                        resource.post_error(Error::AlreadyUsed, "this configuration has already been tested or applied");
                    } else if config.update_for_output(&head.output).is_some() {
                        resource.post_error(
                            Error::AlreadyConfiguredHead,
                            "enabled_head() or disable_head() already called for this head on this configuration",
                        );
                    } else {
                        config.updates.push(OutputConfigurationUpdate::Disable(head.output.downgrade()));
                    }
                } else {
                    state.configurations.retain(|config| &config.instance != resource);
                    resource.cancelled();
                }
            }

            Request::Test => {
                if let Some(config) = state
                    .wlr_output_management_state()
                    .configurations
                    .iter_mut()
                    .find(|config| &config.instance == resource)
                {
                    if config.used {
                        resource.post_error(Error::AlreadyUsed, "this configuration has already been tested or applied");
                    } else {
                        config.used = true;
                        let config = config.clone();
                        state.on_test_configuration(config);
                    }
                } else {
                    resource.cancelled();
                }
            }

            Request::Apply => {
                if let Some(config) = state
                    .wlr_output_management_state()
                    .configurations
                    .iter_mut()
                    .find(|config| &config.instance == resource)
                {
                    if config.used {
                        resource.post_error(Error::AlreadyUsed, "this configuration has already been tested or applied");
                    } else {
                        config.used = true;
                        let config = config.clone();
                        state.on_apply_configuration(config);
                    }
                } else {
                    resource.cancelled();
                }
            }

            Request::Destroy => <Self as Dispatch<ZwlrOutputConfigurationV1, (), H>>::destroyed(state, client.id(), resource, data),

            _ => (),
        }
    }

    fn destroyed(state: &mut H, _client: ClientId, resource: &ZwlrOutputConfigurationV1, _data: &()) {
        state
            .wlr_output_management_state()
            .configurations
            .retain(|config| &config.instance != resource);
    }
}

impl<H: WlrOutputManagementHandler> Dispatch<ZwlrOutputConfigurationHeadV1, (), H> for WlrOutputManagementState {
    fn request(
        state: &mut H,
        _client: &Client,
        resource: &ZwlrOutputConfigurationHeadV1,
        request: <ZwlrOutputConfigurationHeadV1 as Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, H>,
    ) {
        use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_configuration_head_v1::{Error, Request};
        use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_configuration_v1::Error as ConfigError;

        let handler = state;
        let state = handler.wlr_output_management_state();

        if let Some(config) = state.configurations.iter_mut().find(|config| {
            config
                .updates
                .iter()
                .any(|update| matches!(update, OutputConfigurationUpdate::Enable(head) if &head.instance == resource))
        }) && config.used
        {
            config
                .instance
                .post_error(ConfigError::AlreadyUsed, "this configuration has already been tested or applied");
        } else if let Some(config_head) = state.configurations.iter_mut().find_map(|config| {
            config
                .updates
                .iter_mut()
                .find_map(|update| update.enable_mut().filter(|head| &head.instance == resource))
        }) {
            match request {
                Request::SetMode { mode } => {
                    if config_head.mode.is_some() {
                        resource.post_error(Error::AlreadySet, "mode has already been set");
                    } else if let Some(wlr_mode) = state.heads.iter().find(|head| head.output == config_head.output).and_then(|head| {
                        head.modes
                            .iter()
                            .find(|head_mode| head_mode.instances.iter().any(|mode_instance| mode_instance == &mode))
                    }) {
                        config_head.mode = Some(ConfiguredMode::Advertised(wlr_mode.mode));
                    } else {
                        resource.post_error(Error::InvalidMode, "mode does not belong to head");
                    }
                }

                Request::SetCustomMode { width, height, refresh } => {
                    if config_head.mode.is_some() {
                        resource.post_error(Error::AlreadySet, "mode has already been set");
                    } else if width <= 0 || height <= 0 || refresh <= 1000 {
                        resource.post_error(Error::InvalidCustomMode, "custom mode had invalid values");
                    } else {
                        config_head.mode = Some(ConfiguredMode::Custom { width, height, refresh });
                    }
                }

                Request::SetPosition { x, y } => {
                    if config_head.position.is_some() {
                        resource.post_error(Error::AlreadySet, "position has already been set");
                    } else if x < 0 && y < 0 {
                        // Strange that there's no error for this case...
                    } else {
                        config_head.position = Some((x, y).into());
                    }
                }

                Request::SetScale { scale } => {
                    if config_head.scale.is_some() {
                        resource.post_error(Error::AlreadySet, "scale has already been set");
                    } else if scale <= 0. || scale >= 10. {
                        resource.post_error(Error::InvalidScale, "scale must be > 0 and <= 10");
                    } else {
                        config_head.scale = Some(scale);
                    }
                }

                Request::SetTransform { transform } => {
                    if config_head.scale.is_some() {
                        resource.post_error(Error::AlreadySet, "transform has already been set");
                    } else {
                        match transform {
                            WEnum::Unknown(n) => resource.post_error(Error::InvalidTransform, format!("unknown transform value {n}")),
                            WEnum::Value(transform) => config_head.transform = Some(transform.into()),
                        }
                    }
                }

                Request::SetAdaptiveSync { state } => {
                    if config_head.scale.is_some() {
                        resource.post_error(Error::AlreadySet, "adaptive sync has already been set");
                    } else {
                        match state {
                            WEnum::Unknown(n) => {
                                resource.post_error(Error::InvalidAdaptiveSyncState, format!("unknown adaptive sync state value {n}"))
                            }
                            WEnum::Value(state) => config_head.adaptive_sync = Some(state),
                        }
                    }
                }

                _ => (),
            }
        }
    }

    fn destroyed(state: &mut H, _client: ClientId, resource: &ZwlrOutputConfigurationHeadV1, _data: &()) {
        for config in state.wlr_output_management_state().configurations.iter_mut() {
            let len = config.updates.len();
            config
                .updates
                .retain(|update| !matches!(update, OutputConfigurationUpdate::Enable(head) if &head.instance != resource));
            if len != config.updates.len() {
                break;
            }
        }
    }
}

fn send_head<H: WlrOutputManagementHandler>(
    dh: &DisplayHandle,
    client: &Client,
    manager_instance: &ZwlrOutputManagerV1,
    head: &mut WlrHead,
) -> anyhow::Result<()> {
    let instance = client.create_resource::<ZwlrOutputHeadV1, _, H>(dh, manager_instance.version(), ())?;
    manager_instance.head(&instance);

    let phys_props = head.output.physical_properties();

    instance.name(head.output.name());
    instance.description(head.output.description());
    if instance.version() >= EVT_MAKE_SINCE {
        instance.make(phys_props.make);
    }
    if instance.version() >= EVT_MODEL_SINCE {
        instance.model(phys_props.model);
    }
    if instance.version() >= EVT_SERIAL_NUMBER_SINCE {
        instance.serial_number(phys_props.serial_number);
    }
    instance.physical_size(phys_props.size.w, phys_props.size.h);

    for mode in head.modes.iter_mut() {
        let is_current = head.last_current_mode.as_ref() == Some(&mode.mode);
        let is_preferred = head.last_preferred_mode.as_ref() == Some(&mode.mode);
        send_mode::<H>(dh, client, &instance, mode, is_current, is_preferred)?;
    }

    // XXX: is this a good way of deciding if the output is enabled?
    if head.last_is_enabled && head.last_current_mode.is_some() {
        instance.enabled(1);
        // XXX: is Logical the "global compositor space"?
        instance.position(head.last_position.x, head.last_position.y);
        instance.transform(head.last_transform.into());
        instance.scale(head.last_scale);
    } else {
        instance.enabled(0);
    }

    if instance.version() >= EVT_ADAPTIVE_SYNC_SINCE {
        instance.adaptive_sync(head.last_adaptive_sync);
    }

    head.instances.push(instance);

    Ok(())
}

fn send_mode<H: WlrOutputManagementHandler>(
    dh: &DisplayHandle,
    client: &Client,
    head_instance: &ZwlrOutputHeadV1,
    mode: &mut WlrMode,
    is_current: bool,
    is_preferred: bool,
) -> anyhow::Result<()> {
    let instance = client.create_resource::<ZwlrOutputModeV1, _, H>(dh, head_instance.version(), ())?;
    head_instance.mode(&instance);

    instance.size(mode.mode.size.w, mode.mode.size.h);
    instance.refresh(mode.mode.refresh);
    if is_preferred {
        instance.preferred();
    }

    if is_current {
        head_instance.current_mode(&instance);
    }

    mode.instances.push(instance.clone());

    Ok(())
}

macro_rules! delegate_wlr_output_management {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_manager_v1::ZwlrOutputManagerV1: Box<dyn for<'c> Fn(&'c smithay::reexports::wayland_server::Client) -> bool + Send + Sync>
        ] => $crate::protocols::wlr_output_management::WlrOutputManagementState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_manager_v1::ZwlrOutputManagerV1: ()
        ] => $crate::protocols::wlr_output_management::WlrOutputManagementState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_head_v1::ZwlrOutputHeadV1: ()
        ] => $crate::protocols::wlr_output_management::WlrOutputManagementState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_mode_v1::ZwlrOutputModeV1: ()
        ] => $crate::protocols::wlr_output_management::WlrOutputManagementState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_configuration_v1::ZwlrOutputConfigurationV1: ()
        ] => $crate::protocols::wlr_output_management::WlrOutputManagementState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1: ()
        ] => $crate::protocols::wlr_output_management::WlrOutputManagementState);
    };
}

pub(crate) use delegate_wlr_output_management;
