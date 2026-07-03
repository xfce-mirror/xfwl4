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

use std::{
    collections::HashMap,
    io::{Error as IoError, ErrorKind, Result as IoResult},
    os::fd::AsFd,
    sync::Arc,
};

use smithay::{
    input::{Seat, SeatHandler},
    output::Output,
    reexports::{
        wayland_protocols::ext::workspace::v1::server::ext_workspace_handle_v1::ExtWorkspaceHandleV1,
        wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::{ClientId, GlobalId},
        },
    },
    utils::SealedFile,
    wayland::{Dispatch2, GlobalDispatch2},
};

use crate::{
    core::shell::WindowState,
    protocols::{
        ClientFilter, GlobalData,
        ext_workspace::{ExtWorkspaceHandler, ExtWorkspaceState},
        foreign_toplevel_management::{
            ToplevelHandleData, ToplevelId,
            wlr_foreign_toplevel_management::WlrForeignToplevelHandler,
            xfce_foreign_toplevel_management::proto::{
                xfce_foreign_toplevel_handle_v1::{State, XfceForeignToplevelHandleV1},
                xfce_foreign_toplevel_icon_pixels_v1::{FailureReason, XfceForeignToplevelIconPixelsV1},
                xfce_foreign_toplevel_manager_private_v1::XfceForeignToplevelManagerPrivateV1,
            },
        },
    },
};

pub struct XfceForeignToplevelManagementState {
    _global: GlobalId,
    manager_instances: Vec<XfceForeignToplevelManagerPrivateV1>,
    toplevels: HashMap<Arc<ToplevelId>, XfceForeignToplevel>,
}

pub trait XfceForeignToplevelHandler
where
    Self: SeatHandler + 'static,
{
    fn xfce_foreign_toplevel_management_state(&mut self) -> &mut XfceForeignToplevelManagementState;

    fn icon_pixels_for_toplevel(&mut self, toplevel_id: &ToplevelId, icon_size: u32, icon_scale: u32) -> Option<IconPixels>;

    fn on_toplevel_set_shaded(&mut self, toplevel_id: &ToplevelId);
    fn on_toplevel_unset_shaded(&mut self, toplevel_id: &ToplevelId);
    fn on_toplevel_set_sticky(&mut self, toplevel_id: &ToplevelId);
    fn on_toplevel_unset_sticky(&mut self, toplevel_id: &ToplevelId);
    fn on_toplevel_set_keep_above(&mut self, toplevel_id: &ToplevelId);
    fn on_toplevel_unset_keep_above(&mut self, toplevel_id: &ToplevelId);
    fn on_toplevel_set_keep_below(&mut self, toplevel_id: &ToplevelId);
    fn on_toplevel_unset_keep_below(&mut self, toplevel_id: &ToplevelId);

    fn on_toplevel_highlight(&mut self, toplevel_id: &ToplevelId, requesting_client: Client, seat: Seat<Self>);
    fn on_toplevel_unhighlight(&mut self, toplevel_id: &ToplevelId, requesting_client: Client);

    fn on_toplevel_move_to_output(&mut self, toplevel_id: &ToplevelId, output: Output);
    fn on_toplevel_move_to_workspace(&mut self, toplevel_id: &ToplevelId, workspace_id: String);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct IconSize {
    pub size: u32,
    pub scale: u32,
}

pub struct IconPixels {
    fd: SealedFile,
    width: u32,
    height: u32,
    stride: u32,
}

pub struct XfceForeignToplevelManagementGlobalData {
    filter: ClientFilter,
}

struct XfceForeignToplevelInstance {
    instance: XfceForeignToplevelHandleV1,
    wlr_instance: ZwlrForeignToplevelHandleV1,
    workspace_instances_entered: Vec<ExtWorkspaceHandleV1>,
}

struct XfceForeignToplevel {
    instances: Vec<XfceForeignToplevelInstance>,
    state: WindowState,
    workspace_id: Option<String>,
    icon_name: Option<String>,
    icon_sizes: Vec<IconSize>,
}

impl XfceForeignToplevelManagementState {
    pub(super) fn new<H, F>(dh: &DisplayHandle, filter: F) -> Self
    where
        H: XfceForeignToplevelHandler + GlobalDispatch<XfceForeignToplevelManagerPrivateV1, XfceForeignToplevelManagementGlobalData>,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global = dh.create_global::<H, XfceForeignToplevelManagerPrivateV1, _>(
            1,
            XfceForeignToplevelManagementGlobalData { filter: Box::new(filter) },
        );
        Self {
            _global: global,
            manager_instances: Vec::new(),
            toplevels: HashMap::new(),
        }
    }

    pub(super) fn toplevel_created(
        &mut self,
        toplevel_id: Arc<ToplevelId>,
        state: WindowState,
        workspace_id: Option<String>,
        icon_name: Option<String>,
        mut icon_sizes: Vec<IconSize>,
    ) {
        icon_sizes.sort_by_key(|size| size.size * size.scale);
        let toplevel = XfceForeignToplevel {
            instances: Vec::new(),
            state,
            workspace_id,
            icon_name,
            icon_sizes,
        };
        self.toplevels.insert(toplevel_id, toplevel);
    }

    pub(super) fn toplevel_changed<D: ExtWorkspaceHandler>(
        &mut self,
        workspace_state: &ExtWorkspaceState<D>,
        toplevel_id: &ToplevelId,
        state: Option<WindowState>,
        workspace_id: Option<Option<String>>,
        icon_name: Option<Option<String>>,
        mut icon_sizes: Option<Vec<IconSize>>,
    ) -> bool {
        if let Some(toplevel) = self.toplevels.get_mut(toplevel_id) {
            icon_sizes
                .iter_mut()
                .for_each(|icon_sizes| icon_sizes.sort_by_key(|size| size.size * size.scale));

            let changed_state = state.and_then(|state| (toplevel.state != state).then(|| toplevel_state_to_array(state)));
            let changed_workspace_id = workspace_id.filter(|workspace_id| toplevel.workspace_id != *workspace_id);
            let changed_icon_name = icon_name.filter(|icon_name| toplevel.icon_name != *icon_name);
            let changed_icon_sizes = icon_sizes.filter(|icon_sizes| toplevel.icon_sizes != *icon_sizes);

            let icon_changed = changed_icon_name.is_some() || changed_icon_sizes.is_some();

            if changed_state.is_some() || changed_workspace_id.is_some() || icon_changed {
                // Do these up front so we can send the new full state to each client, but defer the
                // updates to the other properties for later.
                if let Some(icon_name) = changed_icon_name {
                    toplevel.icon_name = icon_name;
                }
                if let Some(icon_sizes) = changed_icon_sizes {
                    toplevel.icon_sizes = icon_sizes;
                }

                for XfceForeignToplevelInstance { instance, .. } in &toplevel.instances {
                    if let Some(new_state) = &changed_state {
                        instance.state(new_state.clone());
                    }

                    if icon_changed {
                        send_icon_to_instance(toplevel, instance);
                    }
                }

                if let Some(changed_workspace_id) = &changed_workspace_id {
                    send_workspace_enter_leave(workspace_state, toplevel, changed_workspace_id.as_ref());
                }

                if let Some(state) = state {
                    toplevel.state = state;
                }
                if let Some(workspace_id) = changed_workspace_id {
                    toplevel.workspace_id = workspace_id;
                }

                true
            } else {
                false
            }
        } else {
            false
        }
    }

    pub(super) fn toplevel_destroyed(&mut self, toplevel_id: &ToplevelId) {
        self.toplevels.remove(toplevel_id);
    }

    pub(super) fn flush_client_workspace_events<D: ExtWorkspaceHandler>(
        &mut self,
        ext_workspace_state: &ExtWorkspaceState<D>,
        client: &Client,
    ) {
        for xfce_toplevel in self.toplevels.values_mut() {
            if xfce_toplevel.workspace_id.is_some() {
                for xfce_instance in xfce_toplevel.instances.iter_mut().filter(|xfce_instance| {
                    xfce_instance
                        .instance
                        .client()
                        .is_some_and(|instance_client| instance_client == *client)
                }) {
                    let new_workspace_instances_entered = send_workspace_enter(
                        ext_workspace_state,
                        &xfce_instance.instance,
                        &xfce_instance.workspace_instances_entered,
                        xfce_toplevel.workspace_id.as_ref(),
                    );
                    if !new_workspace_instances_entered.is_empty() {
                        xfce_instance.wlr_instance.done();
                    }
                    xfce_instance.workspace_instances_entered.extend(new_workspace_instances_entered);
                }
            }
        }
    }
}

impl IconSize {
    pub fn new(width: u32, height: u32, scale: u32) -> Self {
        Self {
            size: width.max(height) / scale,
            scale,
        }
    }
}

impl IconPixels {
    pub fn new(pixels: &[u8], width: u32, height: u32, stride: u32) -> IoResult<Self> {
        if pixels.len() != stride as usize * height as usize {
            Err(IoError::new(
                ErrorKind::InvalidInput,
                "pixel data length does not match provided dimensions",
            ))
        } else if width * 4 > stride {
            Err(IoError::new(ErrorKind::InvalidInput, "width is too large for stride"))
        } else {
            let fd = SealedFile::with_data(c"toplevel-icon", pixels)?;
            Ok(Self { fd, width, height, stride })
        }
    }
}

impl<D> GlobalDispatch2<XfceForeignToplevelManagerPrivateV1, D> for XfceForeignToplevelManagementGlobalData
where
    D: XfceForeignToplevelHandler
        + Dispatch<XfceForeignToplevelManagerPrivateV1, GlobalData, D>
        + Dispatch<XfceForeignToplevelHandleV1, ToplevelHandleData, D>,
{
    fn bind(
        &self,
        state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<XfceForeignToplevelManagerPrivateV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        let instance = data_init.init::<XfceForeignToplevelManagerPrivateV1, _>(resource, GlobalData);
        state.xfce_foreign_toplevel_management_state().manager_instances.push(instance);
    }

    fn can_view(&self, client: &Client) -> bool {
        (self.filter)(client)
    }
}

impl<D> Dispatch2<XfceForeignToplevelManagerPrivateV1, D> for GlobalData
where
    D: WlrForeignToplevelHandler
        + XfceForeignToplevelHandler
        + Dispatch<XfceForeignToplevelHandleV1, ToplevelHandleData>
        + ExtWorkspaceHandler,
{
    fn request(
        &self,
        state: &mut D,
        client: &Client,
        resource: &XfceForeignToplevelManagerPrivateV1,
        request: <XfceForeignToplevelManagerPrivateV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use proto::xfce_foreign_toplevel_manager_private_v1::{Error, Request};

        match request {
            Request::GetXfceForeignToplevelHandle {
                id,
                foreign_toplevel_handle,
            } => {
                if let Some(toplevel_id) = state
                    .wlr_foreign_toplevel_management_state()
                    .toplevel_for_handle(&foreign_toplevel_handle)
                {
                    let instance = data_init.init(id, ToplevelHandleData(Arc::clone(&toplevel_id)));

                    if let Some(xfce_toplevel) = state.xfce_foreign_toplevel_management_state().toplevels.get_mut(&toplevel_id) {
                        if xfce_toplevel
                            .instances
                            .iter()
                            .any(|XfceForeignToplevelInstance { wlr_instance, .. }| *wlr_instance == foreign_toplevel_handle)
                        {
                            resource.post_error(
                                Error::AlreadyBound,
                                "passed wlr handle is already associated with an xfce_foreign_toplevel_handle_v1 instance",
                            );
                        } else {
                            let xfce_instance = XfceForeignToplevelInstance {
                                instance: instance.clone(),
                                wlr_instance: foreign_toplevel_handle.clone(),
                                workspace_instances_entered: Vec::new(),
                            };
                            xfce_toplevel.instances.push(xfce_instance);
                            let workspace_id = xfce_toplevel.workspace_id.clone();

                            instance.state(toplevel_state_to_array(xfce_toplevel.state));
                            send_icon_to_instance(xfce_toplevel, &instance);
                            let workspace_instances_entered =
                                send_workspace_enter::<D>(state.ext_workspace_state(), &instance, &[], workspace_id.as_ref());

                            if let Some(toplevel) = state.xfce_foreign_toplevel_management_state().toplevels.get_mut(&toplevel_id)
                                && let Some(xfce_instance) = toplevel
                                    .instances
                                    .iter_mut()
                                    .find(|xfce_instance| xfce_instance.instance == instance)
                            {
                                xfce_instance.workspace_instances_entered = workspace_instances_entered;
                            }

                            foreign_toplevel_handle.done();
                        }
                    } else {
                        resource.post_error(Error::InvalidHandle, "passed wlr handle is invalid");
                    }
                } else {
                    let _instance = data_init.init(id, ToplevelHandleData(Arc::new(ToplevelId("dummy".to_owned()))));
                    resource.post_error(Error::InvalidHandle, "passed wlr handle is invalid");
                }
            }

            Request::Destroy => self.destroyed(state, client.id(), resource),
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &XfceForeignToplevelManagerPrivateV1) {
        state
            .xfce_foreign_toplevel_management_state()
            .manager_instances
            .retain(|instance| instance != resource);
    }
}

impl<D> Dispatch2<XfceForeignToplevelHandleV1, D> for ToplevelHandleData
where
    D: XfceForeignToplevelHandler + Dispatch<XfceForeignToplevelIconPixelsV1, ToplevelHandleData> + ExtWorkspaceHandler,
{
    fn request(
        &self,
        state: &mut D,
        client: &Client,
        resource: &XfceForeignToplevelHandleV1,
        request: <XfceForeignToplevelHandleV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use proto::xfce_foreign_toplevel_handle_v1::Request;

        match request {
            Request::SetShaded => state.on_toplevel_set_shaded(self.0.as_ref()),
            Request::UnsetShaded => state.on_toplevel_unset_shaded(self.0.as_ref()),
            Request::SetSticky => state.on_toplevel_set_sticky(self.0.as_ref()),
            Request::UnsetSticky => state.on_toplevel_unset_sticky(self.0.as_ref()),
            Request::SetAbove => state.on_toplevel_set_keep_above(self.0.as_ref()),
            Request::UnsetAbove => state.on_toplevel_unset_keep_above(self.0.as_ref()),
            Request::SetBelow => state.on_toplevel_set_keep_below(self.0.as_ref()),
            Request::UnsetBelow => state.on_toplevel_unset_keep_below(self.0.as_ref()),

            Request::Highlight { seat } => {
                if let Some(seat) = Seat::from_resource(&seat) {
                    state.on_toplevel_highlight(self.0.as_ref(), client.clone(), seat)
                }
            }
            Request::Unhighlight => state.on_toplevel_unhighlight(self.0.as_ref(), client.clone()),

            Request::MoveToOutput { output } => {
                if let Some(output) = Output::from_resource(&output) {
                    state.on_toplevel_move_to_output(self.0.as_ref(), output);
                }
            }

            Request::MoveToWorkspace { workspace } => {
                if let Some(workspace_id) = state
                    .ext_workspace_state()
                    .workspace_id_for_handle(&workspace)
                    .map(ToOwned::to_owned)
                {
                    state.on_toplevel_move_to_workspace(self.0.as_ref(), workspace_id);
                }
            }

            Request::GetIconPixels { id, size, scale } => {
                let pixels_instance = data_init.init(id, self.clone());

                if !state
                    .xfce_foreign_toplevel_management_state()
                    .toplevels
                    .get(&self.0)
                    .map(|toplevel| !toplevel.icon_sizes.is_empty())
                    .unwrap_or(false)
                {
                    pixels_instance.failed(FailureReason::NoPixelData);
                } else if let Some(pixels) = state.icon_pixels_for_toplevel(&self.0, size, scale) {
                    pixels_instance.pixels(pixels.fd.as_fd(), pixels.width, pixels.height, pixels.stride);
                } else {
                    pixels_instance.failed(FailureReason::InvalidArgs);
                }
            }

            Request::Destroy => self.destroyed(state, client.id(), resource),
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &XfceForeignToplevelHandleV1) {
        for toplevel in state.xfce_foreign_toplevel_management_state().toplevels.values_mut() {
            if let Some(pos) = toplevel
                .instances
                .iter()
                .position(|XfceForeignToplevelInstance { instance, .. }| instance == resource)
            {
                toplevel.instances.remove(pos);
                break;
            }
        }
    }
}

impl<D> Dispatch2<XfceForeignToplevelIconPixelsV1, D> for ToplevelHandleData
where
    D: XfceForeignToplevelHandler,
{
    fn request(
        &self,
        state: &mut D,
        client: &Client,
        resource: &XfceForeignToplevelIconPixelsV1,
        request: <XfceForeignToplevelIconPixelsV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        use proto::xfce_foreign_toplevel_icon_pixels_v1::Request;

        match request {
            Request::Destroy => self.destroyed(state, client.id(), resource),
        }
    }

    fn destroyed(&self, _state: &mut D, _client: ClientId, _resource: &XfceForeignToplevelIconPixelsV1) {
        // We don't store any state related to these instances, so nothing to do here.
    }
}

fn send_icon_to_instance(toplevel: &XfceForeignToplevel, instance: &XfceForeignToplevelHandleV1) {
    let has_icon = toplevel.icon_name.is_some() || !toplevel.icon_sizes.is_empty();
    if has_icon {
        if let Some(icon_name) = &toplevel.icon_name {
            instance.icon_name(icon_name.clone());
        }
        for IconSize { size, scale } in &toplevel.icon_sizes {
            instance.icon_size(*size, *scale);
        }
    } else {
        instance.no_icon();
    }
}

/// Sends workspace_leave and workspace_enter when the workspace for the toplevel changes.
///
/// This drains the `workspace_instances_entered` vec inside the each instance data struct, and
/// sends workspace_leave to each one.  Then sends workspace_enter to all matching client handles
/// for the new workspace, and stores them in `workspace_instances_entered`.
fn send_workspace_enter_leave<D: ExtWorkspaceHandler>(
    workspace_state: &ExtWorkspaceState<D>,
    xfce_toplevel: &mut XfceForeignToplevel,
    new_workspace_id: Option<&String>,
) {
    for XfceForeignToplevelInstance {
        instance,
        workspace_instances_entered,
        ..
    } in &mut xfce_toplevel.instances
    {
        if let Some(client) = instance.client() {
            let new_workspace_instances = new_workspace_id
                .as_ref()
                .map(|id| workspace_state.workspace_handles_for_id(id).iter().collect::<Vec<_>>())
                .unwrap_or_default();

            for workspace_instance in std::mem::take(workspace_instances_entered) {
                if workspace_instance.is_alive() {
                    instance.workspace_leave(&workspace_instance);
                }
            }

            for workspace_instance in new_workspace_instances
                .iter()
                .filter(|workspace_instance| workspace_instance.client().as_ref() == Some(&client))
            {
                instance.workspace_enter(workspace_instance);
                workspace_instances_entered.push((*workspace_instance).clone());
            }
        }
    }
}

/// Sends workspace_enter events.
///
/// This is only for use for a new instance, or to flush changes when a new client binds to
/// ext-workspace.
///
/// Does not need to send workspace_leave, as it's a new instance and has not had any events sent
/// on it yet.
///
/// Returns the list of handles it sent workspace_enter on.
fn send_workspace_enter<D: ExtWorkspaceHandler>(
    ext_workspace_state: &ExtWorkspaceState<D>,
    instance: &XfceForeignToplevelHandleV1,
    workspace_instances_entered: &[ExtWorkspaceHandleV1],
    workspace_id: Option<&String>,
) -> Vec<ExtWorkspaceHandleV1> {
    if let Some(instance_client) = instance.client()
        && let Some(new_workspace_id) = workspace_id
    {
        let mut new_workspace_instances_entered = Vec::new();

        for workspace_instance in ext_workspace_state.workspace_handles_for_id(new_workspace_id) {
            if workspace_instance.client().as_ref() == Some(&instance_client) && !workspace_instances_entered.contains(workspace_instance) {
                instance.workspace_enter(workspace_instance);
                new_workspace_instances_entered.push(workspace_instance.clone());
            }
        }

        new_workspace_instances_entered
    } else {
        Vec::new()
    }
}

fn toplevel_state_to_array(value: WindowState) -> Vec<u8> {
    [
        (WindowState::SHADED, State::Shaded),
        (WindowState::STICKY, State::Sticky),
        (WindowState::SKIP_PAGER, State::SkipPager),
        (WindowState::SKIP_TASKBAR, State::SkipTasklist),
        (WindowState::KEEP_ABOVE, State::Above),
        (WindowState::KEEP_BELOW, State::Below),
        (WindowState::DEMANDS_ATTENTION, State::DemandsAttention),
    ]
    .into_iter()
    .flat_map(|(flag, state)| value.contains(flag).then_some(state))
    .flat_map(|v| (v as u32).to_ne_bytes())
    .collect()
}

pub mod proto {
    use smithay::reexports::{
        wayland_protocols::ext::workspace::v1::server::ext_workspace_handle_v1,
        wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_handle_v1,
        wayland_server::{
            self,
            protocol::{wl_output, wl_seat},
        },
    };

    pub mod __interfaces {
        use smithay::reexports::{
            wayland_protocols::ext::workspace::v1::server::__interfaces::{
                EXT_WORKSPACE_HANDLE_V1_INTERFACE, ext_workspace_handle_v1_interface,
            },
            wayland_protocols_wlr::foreign_toplevel::v1::server::__interfaces::{
                ZWLR_FOREIGN_TOPLEVEL_HANDLE_V1_INTERFACE, zwlr_foreign_toplevel_handle_v1_interface,
            },
            wayland_server::{
                backend as wayland_backend,
                protocol::__interfaces::{WL_OUTPUT_INTERFACE, WL_SEAT_INTERFACE, wl_output_interface, wl_seat_interface},
            },
        };

        wayland_scanner::generate_interfaces!("./resources/xfce-wayland-protocols/xfce-foreign-toplevel-management-private-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("./resources/xfce-wayland-protocols/xfce-foreign-toplevel-management-private-v1.xml");
}
