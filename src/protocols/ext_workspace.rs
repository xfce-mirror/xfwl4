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

/// This implements the ext-workspace protocol for xfwl4.  Xfce traditionally has just had one set
/// of workspaces that is mapped across all outputs, so to simplify things, this just always
/// advertises a single workspace group, and all workspaces are advertised as members of that
/// group.  Whenever outputs come and go, output_enter/output_leave are emitted on the singleton
/// workspace group.
///
/// We also don't need to support the 'assign' request, as all workspaces are always assigned to
/// the singleton group.
use std::{collections::HashMap, marker::PhantomData, sync::Mutex};

use indexmap::IndexMap;
use smithay::{
    output::Output,
    reexports::{
        wayland_protocols::ext::workspace::v1::server::{
            ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1, GroupCapabilities},
            ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1, State as WorkspaceState, WorkspaceCapabilities},
            ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
        },
        wayland_server::{
            self, DataInit, Dispatch, DisplayHandle, GlobalDispatch, Resource, Weak,
            backend::{self, GlobalId, ObjectId},
        },
    },
    utils::{Logical, Point},
};

pub struct ExtWorkspaceState<H: ExtWorkspaceHandler> {
    dh: DisplayHandle,
    _global: GlobalId,
    manager_instances: Vec<ExtWorkspaceManagerV1>,
    group: WorkspaceGroup,
    workspaces: IndexMap<String, Workspace>,
    instance_to_workspace: HashMap<ObjectId, String>,
    _handler_marker: PhantomData<H>,
}

impl<H: ExtWorkspaceHandler> ExtWorkspaceState<H> {
    pub fn new(dh: &DisplayHandle) -> Self {
        let dh = dh.clone();
        let global = dh.create_global::<H, ExtWorkspaceManagerV1, _>(1, ());

        Self {
            dh,
            _global: global,
            manager_instances: Default::default(),
            group: WorkspaceGroup {
                instances: Default::default(),
                outputs: Default::default(),
            },
            workspaces: Default::default(),
            instance_to_workspace: Default::default(),
            _handler_marker: PhantomData::<H>,
        }
    }

    pub fn output_enter(&mut self, output: &Output) {
        self.group.outputs.push(output.clone());

        for instance in &self.group.instances {
            if let Some(client) = instance.client() {
                for output in output.client_outputs(&client) {
                    instance.output_enter(&output);
                }
            }
        }
    }

    pub fn output_leave(&mut self, output: &Output) {
        self.group.outputs.retain(|o| o != output);

        for instance in &self.group.instances {
            if let Some(client) = instance.client() {
                for output in output.client_outputs(&client) {
                    instance.output_leave(&output);
                }
            }
        }
    }

    pub fn workspace_created<'a>(&mut self, input: WorkspaceCreatedInput<'a>) {
        let id = input.id.to_owned();
        self.workspaces.insert(id.clone(), input.into());

        let Self {
            dh,
            manager_instances,
            workspaces,
            group,
            instance_to_workspace,
            ..
        } = self;
        let workspace = workspaces.get_mut(&id).unwrap();

        for manager in manager_instances.iter() {
            if let Some(object_id) = send_workspace::<H>(dh, manager, workspace, group) {
                instance_to_workspace.insert(object_id, id.clone());
            }
        }
    }

    pub fn workspace_changed<'a>(&mut self, workspace_id: &'a str, input: WorkspaceChangedInput<'a>) {
        if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
            if let Some(name) = input.name {
                workspace.name = name.to_owned();
                for instance in &workspace.instances {
                    instance.name(workspace.name.clone());
                }
            }

            if let Some(coordinates) = input.coordinates {
                workspace.coordinates = coords_from_point(coordinates);
                for instance in &workspace.instances {
                    instance.coordinates(workspace.coordinates.clone());
                }
            }

            if let Some(is_active) = input.is_active {
                let changed = if is_active && !workspace.state.contains(WorkspaceState::Active) {
                    workspace.state |= WorkspaceState::Active;
                    true
                } else if !is_active && workspace.state.contains(WorkspaceState::Active) {
                    workspace.state &= !WorkspaceState::Active;
                    true
                } else {
                    false
                };

                if changed {
                    for instance in &workspace.instances {
                        instance.state(workspace.state);
                    }
                }
            }
        }
    }

    pub fn workspace_destroyed(&mut self, workspace_id: &str) {
        if let Some(workspace) = self.workspaces.shift_remove(workspace_id) {
            for workspace_instance in &workspace.instances {
                self.instance_to_workspace.remove(&Resource::id(workspace_instance));
                for group_instance in &self.group.instances {
                    if group_instance.client().is_some() && group_instance.client() == workspace_instance.client() {
                        group_instance.workspace_leave(workspace_instance);
                        workspace_instance.removed();
                    }
                }
            }
        }
    }
}

pub trait ExtWorkspaceHandler
where
    Self: GlobalDispatch<ExtWorkspaceManagerV1, ()>
        + Dispatch<ExtWorkspaceManagerV1, Mutex<WorkspaceManagerData>>
        + Dispatch<ExtWorkspaceGroupHandleV1, ()>
        + Dispatch<ExtWorkspaceHandleV1, WorkspaceData>
        + Sized
        + 'static,
{
    fn ext_workspace_state(&mut self) -> &mut ExtWorkspaceState<Self>;

    fn on_workspace_activate(&mut self, workspace_id: &str);
    fn on_workspace_deactivate(&mut self, workspace_id: &str);
}

enum ClientRequest {
    Activate(String),
    Deactivate(String),
}

#[derive(Default)]
pub(crate) struct WorkspaceManagerData {
    requests: Vec<ClientRequest>,
}

struct WorkspaceGroup {
    instances: Vec<ExtWorkspaceGroupHandleV1>,
    outputs: Vec<Output>,
}

struct Workspace {
    id: String,
    instances: Vec<ExtWorkspaceHandleV1>,
    name: String,
    coordinates: Vec<u8>,
    state: WorkspaceState,
}

pub(crate) struct WorkspaceData {
    manager: Weak<ExtWorkspaceManagerV1>,
}

pub struct WorkspaceCreatedInput<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub coordinates: Point<u32, Logical>,
    pub is_active: bool,
}

#[derive(Default)]
pub struct WorkspaceChangedInput<'a> {
    pub name: Option<&'a str>,
    pub coordinates: Option<Point<u32, Logical>>,
    pub is_active: Option<bool>,
}

impl<'a> From<WorkspaceCreatedInput<'a>> for Workspace {
    fn from(value: WorkspaceCreatedInput<'a>) -> Self {
        Self {
            id: value.id.to_owned(),
            instances: Default::default(),
            name: value.name.to_owned(),
            coordinates: coords_from_point(value.coordinates),
            state: if value.is_active {
                WorkspaceState::Active
            } else {
                WorkspaceState::empty()
            },
        }
    }
}

impl<H: ExtWorkspaceHandler> GlobalDispatch<ExtWorkspaceManagerV1, (), H> for ExtWorkspaceState<H> {
    fn bind(
        state: &mut H,
        handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: wayland_server::New<ExtWorkspaceManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, H>,
    ) {
        let state = state.ext_workspace_state();
        let manager = data_init.init(resource, Mutex::new(WorkspaceManagerData::default()));
        send_group::<H>(handle, &manager, &mut state.group);

        let ExtWorkspaceState {
            workspaces,
            group,
            instance_to_workspace,
            manager_instances,
            ..
        } = state;
        for workspace in workspaces.values_mut() {
            if let Some(object_id) = send_workspace::<H>(handle, &manager, workspace, group) {
                instance_to_workspace.insert(object_id, workspace.id.clone());
            }
        }
        manager.done();
        manager_instances.push(manager);
    }
}

impl<H: ExtWorkspaceHandler> Dispatch<ExtWorkspaceManagerV1, Mutex<WorkspaceManagerData>, H> for ExtWorkspaceState<H> {
    fn request(
        state: &mut H,
        _client: &wayland_server::Client,
        resource: &ExtWorkspaceManagerV1,
        request: <ExtWorkspaceManagerV1 as Resource>::Request,
        data: &Mutex<WorkspaceManagerData>,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, H>,
    ) {
        match request {
            ext_workspace_manager_v1::Request::Commit => {
                let requests = {
                    let mut data = data.lock().unwrap();
                    std::mem::take(&mut data.requests)
                };

                for request in requests {
                    match request {
                        ClientRequest::Activate(workspace_id) => state.on_workspace_activate(&workspace_id),
                        ClientRequest::Deactivate(workspace_id) => state.on_workspace_deactivate(&workspace_id),
                    }
                }
            }

            ext_workspace_manager_v1::Request::Stop => {
                state
                    .ext_workspace_state()
                    .manager_instances
                    .retain(|instance| instance != resource);
            }

            _ => (),
        }
    }

    fn destroyed(state: &mut H, _client: backend::ClientId, resource: &ExtWorkspaceManagerV1, _data: &Mutex<WorkspaceManagerData>) {
        state
            .ext_workspace_state()
            .manager_instances
            .retain(|instance| instance != resource);
    }
}

impl<H: ExtWorkspaceHandler> Dispatch<ExtWorkspaceGroupHandleV1, (), H> for ExtWorkspaceState<H> {
    fn request(
        state: &mut H,
        _client: &wayland_server::Client,
        resource: &ExtWorkspaceGroupHandleV1,
        request: <ExtWorkspaceGroupHandleV1 as Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, H>,
    ) {
        match request {
            ext_workspace_group_handle_v1::Request::CreateWorkspace { .. } => {
                // ignored; send error?
            }
            ext_workspace_group_handle_v1::Request::Destroy => {
                state.ext_workspace_state().group.instances.retain(|instance| instance != resource);
            }
            _ => (),
        }
    }

    fn destroyed(state: &mut H, _client: backend::ClientId, resource: &ExtWorkspaceGroupHandleV1, _data: &()) {
        state.ext_workspace_state().group.instances.retain(|instance| instance != resource);
    }
}

impl<H: ExtWorkspaceHandler> Dispatch<ExtWorkspaceHandleV1, WorkspaceData, H> for ExtWorkspaceState<H> {
    fn request(
        state: &mut H,
        _client: &wayland_server::Client,
        resource: &ExtWorkspaceHandleV1,
        request: <ExtWorkspaceHandleV1 as Resource>::Request,
        data: &WorkspaceData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, H>,
    ) {
        match request {
            ext_workspace_handle_v1::Request::Activate => {
                let ext_state = state.ext_workspace_state();
                if let Some(workspace_id) = ext_state.instance_to_workspace.get(&Resource::id(resource))
                    && let Some(workspace) = ext_state.workspaces.get(workspace_id)
                    && !workspace.state.contains(WorkspaceState::Active)
                    && let Ok(manager_instance) = data.manager.upgrade()
                {
                    let mut data = manager_instance.data::<Mutex<WorkspaceManagerData>>().unwrap().lock().unwrap();
                    data.requests.push(ClientRequest::Activate(workspace_id.clone()));
                }
            }

            ext_workspace_handle_v1::Request::Deactivate => {
                let ext_state = state.ext_workspace_state();
                if let Some(workspace_id) = ext_state.instance_to_workspace.get(&Resource::id(resource))
                    && let Some(workspace) = ext_state.workspaces.get(workspace_id)
                    && workspace.state.contains(WorkspaceState::Active)
                    && let Ok(manager_instance) = data.manager.upgrade()
                {
                    let mut data = manager_instance.data::<Mutex<WorkspaceManagerData>>().unwrap().lock().unwrap();
                    data.requests.push(ClientRequest::Deactivate(workspace_id.clone()));
                }
            }

            ext_workspace_handle_v1::Request::Remove => {
                // ignored; send error?
            }

            ext_workspace_handle_v1::Request::Assign { .. } => {
                // ignored; send error?
            }

            ext_workspace_handle_v1::Request::Destroy => {
                let ext_state = state.ext_workspace_state();
                if let Some(workspace_id) = ext_state.instance_to_workspace.remove(&Resource::id(resource))
                    && let Some(workspace) = ext_state.workspaces.get_mut(&workspace_id)
                {
                    workspace.instances.retain(|instance| instance != resource);
                }
            }

            _ => (),
        }
    }

    fn destroyed(state: &mut H, _client: backend::ClientId, resource: &ExtWorkspaceHandleV1, _data: &WorkspaceData) {
        let ext_state = state.ext_workspace_state();
        if let Some(workspace_id) = ext_state.instance_to_workspace.remove(&Resource::id(resource))
            && let Some(workspace) = ext_state.workspaces.get_mut(&workspace_id)
        {
            workspace.instances.retain(|instance| instance != resource);
        }
    }
}

fn send_group<H: ExtWorkspaceHandler>(handle: &DisplayHandle, manager: &ExtWorkspaceManagerV1, group: &mut WorkspaceGroup) {
    if let Some(client) = manager.client()
        && let Ok(instance) = client.create_resource::<ExtWorkspaceGroupHandleV1, _, H>(handle, manager.version(), ())
    {
        manager.workspace_group(&instance);

        instance.capabilities(GroupCapabilities::empty());
        for output in &group.outputs {
            for wl_output in output.client_outputs(&client) {
                instance.output_enter(&wl_output);
            }
        }

        group.instances.push(instance);
    }
}

fn send_workspace<H: ExtWorkspaceHandler>(
    handle: &DisplayHandle,
    manager: &ExtWorkspaceManagerV1,
    workspace: &mut Workspace,
    group: &WorkspaceGroup,
) -> Option<ObjectId> {
    if let Some(client) = manager.client()
        && let Ok(instance) = client.create_resource::<ExtWorkspaceHandleV1, _, H>(
            handle,
            manager.version(),
            WorkspaceData {
                manager: manager.downgrade(),
            },
        )
    {
        manager.workspace(&instance);

        instance.id(workspace.id.clone());
        instance.name(workspace.name.clone());
        instance.coordinates(workspace.coordinates.clone());
        instance.state(workspace.state);
        instance.capabilities(WorkspaceCapabilities::Activate | WorkspaceCapabilities::Deactivate);

        for group_instance in &group.instances {
            if let Some(group_client) = group_instance.client()
                && group_client == client
            {
                group_instance.workspace_enter(&instance);
            }
        }

        let object_id = Resource::id(&instance);
        workspace.instances.push(instance);
        Some(object_id)
    } else {
        None
    }
}

fn coords_from_point(point: Point<u32, Logical>) -> Vec<u8> {
    // The wayland protocol XML spec unfortunately doesn't tell what type is contained in array
    // types, so wayland-scanner can't generate optimal Vec<T> types for the APIs that use them.
    // Instead, we have to read the protocol docs to determine the right type, and then serialize
    // it into an array of bytes in the system's native byte order.
    [point.x, point.y].iter().flat_map(|coord| coord.to_ne_bytes()).collect()
}

macro_rules! delegate_ext_workspace {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_manager_v1::ExtWorkspaceManagerV1: ()
        ] => $crate::protocols::ext_workspace::ExtWorkspaceState<Self>);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_manager_v1::ExtWorkspaceManagerV1: std::sync::Mutex<$crate::protocols::ext_workspace::WorkspaceManagerData>
        ] => $crate::protocols::ext_workspace::ExtWorkspaceState<Self>);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1: ()
        ] => $crate::protocols::ext_workspace::ExtWorkspaceState<Self>);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_handle_v1::ExtWorkspaceHandleV1: $crate::protocols::ext_workspace::WorkspaceData
        ] => $crate::protocols::ext_workspace::ExtWorkspaceState<Self>);
    };
}

pub(crate) use delegate_ext_workspace;
