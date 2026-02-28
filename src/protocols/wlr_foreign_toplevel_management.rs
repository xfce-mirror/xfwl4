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

use std::sync::Arc;

use indexmap::IndexMap;
use rand::distributions::{Alphanumeric, DistString};
use smithay::{
    output::Output,
    reexports::{
        wayland_protocols_wlr::foreign_toplevel::v1::server::{
            zwlr_foreign_toplevel_handle_v1::{EVT_PARENT_SINCE, State as ZwlrForeignToplevelHandleStateV1, ZwlrForeignToplevelHandleV1},
            zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::{ClientId, GlobalId},
            protocol::{wl_output::WlOutput, wl_seat::WlSeat, wl_surface::WlSurface},
        },
    },
    utils::{Logical, Rectangle},
};

use crate::core::shell::WindowState;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ToplevelId(String);

pub struct WlrForeignToplevelManagementState {
    dh: DisplayHandle,
    _global: GlobalId,
    manager_instances: Vec<ZwlrForeignToplevelManagerV1>,
    toplevels: IndexMap<Arc<ToplevelId>, WlrForeignToplevel>,
}

impl WlrForeignToplevelManagementState {
    pub fn new<H, F>(dh: &DisplayHandle, filter: F) -> Self
    where
        H: WlrForeignToplevelHandler,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global = dh.create_global::<H, ZwlrForeignToplevelManagerV1, _>(3, Box::new(filter));
        Self {
            dh: dh.clone(),
            _global: global,
            manager_instances: Vec::new(),
            toplevels: IndexMap::new(),
        }
    }

    pub fn toplevel_created<H: WlrForeignToplevelHandler>(
        &mut self,
        title: impl Into<String>,
        app_id: impl Into<String>,
        state: WindowState,
        outputs: Vec<Output>,
        parent: Option<ToplevelId>,
    ) -> ToplevelId {
        let toplevel_id = Arc::new(ToplevelId(Alphanumeric.sample_string(&mut rand::thread_rng(), 32)));
        let mut toplevel = WlrForeignToplevel {
            instances: Vec::new(),
            title: title.into(),
            app_id: app_id.into(),
            state,
            outputs,
            parent,
        };
        let parent_toplevel = toplevel.parent.as_ref().and_then(|parent_id| self.toplevels.get(parent_id));

        for manager in &self.manager_instances {
            if let Some(client) = manager.client()
                && let Ok(instance) =
                    client.create_resource::<ZwlrForeignToplevelHandleV1, _, H>(&self.dh, manager.version(), Arc::clone(&toplevel_id))
            {
                manager.toplevel(&instance);

                instance.title(toplevel.title.clone());
                instance.app_id(toplevel.app_id.clone());
                instance.state(toplevel_state_to_array(toplevel.state));

                for output in &toplevel.outputs {
                    for output_instance in output.client_outputs(&client) {
                        instance.output_enter(&output_instance);
                    }
                }

                if instance.version() >= EVT_PARENT_SINCE {
                    for parent_instance in parent_toplevel.iter().flat_map(|parent| &parent.instances) {
                        if parent_instance.client().as_ref() == Some(&client) {
                            instance.parent(Some(parent_instance));
                        }
                    }
                }

                instance.done();

                toplevel.instances.push(instance);
            }
        }

        let toplevel_id_ret = toplevel_id.as_ref().clone();
        self.toplevels.insert(toplevel_id, toplevel);
        toplevel_id_ret
    }

    #[allow(clippy::too_many_arguments)]
    pub fn toplevel_changed(
        &mut self,
        toplevel_id: &ToplevelId,
        title: Option<&str>,
        app_id: Option<&str>,
        states_added: WindowState,
        states_removed: WindowState,
        outputs_added: Vec<Output>,
        outputs_removed: Vec<Output>,
        parent: Option<Option<ToplevelId>>,
    ) {
        if let Some(toplevel) = self.toplevels.get(toplevel_id) {
            let new_state = (toplevel.state | states_added) & !states_removed;
            let new_state_v1 = new_state & !WindowState::FULLSCREEN;
            let state_changed = new_state != toplevel.state;
            let new_state = toplevel_state_to_array(new_state);

            let old_state_v1 = toplevel.state & !WindowState::FULLSCREEN;
            let state_v1_changed = new_state_v1 != old_state_v1;
            let new_state_v1 = toplevel_state_to_array(new_state_v1);

            for instance in &toplevel.instances {
                if let Some(client) = instance.client() {
                    if let Some(title) = &title {
                        instance.title((*title).to_owned());
                    }

                    if let Some(app_id) = &app_id {
                        instance.app_id((*app_id).to_owned());
                    }

                    if state_changed && instance.version() >= 2 {
                        instance.state(new_state.clone());
                    } else if state_v1_changed {
                        instance.state(new_state_v1.clone());
                    }

                    for output in &outputs_added {
                        if !toplevel.outputs.contains(output) {
                            for output_instance in output.client_outputs(&client) {
                                instance.output_enter(&output_instance);
                            }
                        }
                    }

                    for output in &outputs_removed {
                        if toplevel.outputs.contains(output) {
                            for output_instance in output.client_outputs(&client) {
                                instance.output_leave(&output_instance);
                            }
                        }
                    }

                    if let Some(parent) = &parent
                        && let Some(parent) = parent.as_ref().and_then(|parent| self.toplevels.get(parent))
                    {
                        for parent_instance in &parent.instances {
                            if parent_instance.client().as_ref() == Some(&client) {
                                instance.parent(Some(parent_instance));
                            }
                        }
                    }

                    instance.done();
                }
            }
        }

        if let Some(toplevel) = self.toplevels.get_mut(toplevel_id) {
            if let Some(title) = title {
                toplevel.title = title.to_owned();
            }
            if let Some(app_id) = app_id {
                toplevel.app_id = app_id.to_owned();
            }
            toplevel.state |= states_added;
            toplevel.state &= !states_removed;
            for output in outputs_added {
                if !toplevel.outputs.contains(&output) {
                    toplevel.outputs.push(output);
                }
            }
            toplevel.outputs.retain(|output| !outputs_removed.contains(output));
            if let Some(parent) = parent {
                toplevel.parent = parent;
            }
        }
    }

    pub fn toplevel_destroyed(&mut self, toplevel_id: &ToplevelId) {
        if let Some(toplevel) = self.toplevels.shift_remove(toplevel_id) {
            for instance in toplevel.instances {
                instance.closed();
            }
        }
    }
}

pub struct WlrForeignToplevel {
    instances: Vec<ZwlrForeignToplevelHandleV1>,
    title: String,
    app_id: String,
    state: WindowState,
    outputs: Vec<Output>,
    parent: Option<ToplevelId>,
}

pub trait WlrForeignToplevelHandler
where
    Self: GlobalDispatch<ZwlrForeignToplevelManagerV1, Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>>
        + Dispatch<ZwlrForeignToplevelManagerV1, ()>
        + Dispatch<ZwlrForeignToplevelHandleV1, Arc<ToplevelId>>
        + Sized
        + 'static,
{
    fn wlr_foreign_toplevel_management_state(&mut self) -> &mut WlrForeignToplevelManagementState;

    fn on_toplevel_set_maximized(&mut self, toplevel_id: &ToplevelId);
    fn on_toplevel_unset_maximized(&mut self, toplevel_id: &ToplevelId);
    fn on_toplevel_set_minimized(&mut self, toplevel_id: &ToplevelId);
    fn on_toplevel_unset_minimized(&mut self, toplevel_id: &ToplevelId);
    fn on_toplevel_activate(&mut self, toplevel_id: &ToplevelId, wl_seat: &WlSeat);
    fn on_toplevel_close(&mut self, toplevel_id: &ToplevelId);
    fn on_toplevel_set_rectangle(&mut self, toplevel_id: &ToplevelId, wl_surface: &WlSurface, rect: Rectangle<i32, Logical>);
    fn on_toplevel_set_fullscreen(&mut self, toplevel_id: &ToplevelId, wl_output: Option<&WlOutput>);
    fn on_toplevel_unset_fullscreen(&mut self, toplevel_id: &ToplevelId);
}

impl<H: WlrForeignToplevelHandler> GlobalDispatch<ZwlrForeignToplevelManagerV1, Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>, H>
    for WlrForeignToplevelManagementState
{
    fn bind(
        state: &mut H,
        handle: &DisplayHandle,
        client: &Client,
        resource: New<ZwlrForeignToplevelManagerV1>,
        _global_data: &Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
        data_init: &mut DataInit<'_, H>,
    ) {
        let manager_instance = data_init.init(resource, ());
        let state = state.wlr_foreign_toplevel_management_state();

        // Sending new toplevels is tricky.  We have no idea if the toplevel list is in an order
        // that is conducive to parent-child relationships; that is, a child toplevel may be in the
        // list before its parent, so when we try to send the parent relationship, we won't be able
        // to, because we haven't created the instance for it yet.  So we do this in three passes:
        // first we create a ZwlrForeignToplevelHandleV1 instance for each toplevel, and send all
        // the data we have about it except for the parent.  Then we gather up all the parents, and
        // send them, and finally send the 'done' event for each one.
        //
        // Note that this will sorta kind break in subtle ways if we fail to create a toplevel
        // resource for any toplevel.

        for (toplevel_id, toplevel) in state.toplevels.iter_mut() {
            if let Ok(instance) =
                client.create_resource::<ZwlrForeignToplevelHandleV1, _, H>(handle, manager_instance.version(), Arc::clone(toplevel_id))
            {
                manager_instance.toplevel(&instance);

                instance.title(toplevel.title.clone());
                instance.app_id(toplevel.app_id.clone());
                instance.state(toplevel_state_to_array(toplevel.state));

                for output in &toplevel.outputs {
                    for output_instance in output.client_outputs(client) {
                        instance.output_enter(&output_instance);
                    }
                }

                // Don't send 'done' yet, since we haven't sent the parent, if any.

                toplevel.instances.push(instance);
            }
        }

        let with_parents = state.toplevels.iter().flat_map(|(toplevel_id, toplevel)| {
            toplevel
                .parent
                .as_ref()
                .and_then(|parent_id| state.toplevels.get(parent_id))
                // Since we just created new instances, assume the last one in the list is the
                // one we just created, and the one we need to send the event for.
                .and_then(|parent| {
                    parent
                        .instances
                        .last()
                        .filter(|parent_instance| parent_instance.client().as_ref() == Some(client))
                })
                .map(|parent_instance| (toplevel_id, parent_instance))
        });

        for (child_id, parent_instance) in with_parents {
            if let Some(child) = state.toplevels.get(child_id)
                // Same as before, the last instance in the list should be the one we just
                // created.
                && let Some(child_instance) = child.instances.last()
                && child_instance.client().as_ref() == Some(client)
                && child_instance.version() >= EVT_PARENT_SINCE
            {
                child_instance.parent(Some(parent_instance));
            }
        }

        for toplevel in state.toplevels.values() {
            // Same as before, the last instance in the list should be the one we just
            // created.
            if let Some(instance) = toplevel.instances.last()
                && instance.client().as_ref() == Some(client)
            {
                instance.done();
            }
        }

        state.manager_instances.push(manager_instance);
    }

    fn can_view(client: Client, global_data: &Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>) -> bool {
        global_data(&client)
    }
}

impl<H: WlrForeignToplevelHandler> Dispatch<ZwlrForeignToplevelManagerV1, (), H> for WlrForeignToplevelManagementState {
    fn request(
        state: &mut H,
        client: &Client,
        resource: &ZwlrForeignToplevelManagerV1,
        request: <ZwlrForeignToplevelManagerV1 as Resource>::Request,
        data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, H>,
    ) {
        use smithay::reexports::wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_manager_v1::Request;

        if let Request::Stop = request {
            <Self as Dispatch<ZwlrForeignToplevelManagerV1, (), H>>::destroyed(state, client.id(), resource, data);
        }
    }

    fn destroyed(state: &mut H, _client: ClientId, resource: &ZwlrForeignToplevelManagerV1, _data: &()) {
        state
            .wlr_foreign_toplevel_management_state()
            .manager_instances
            .retain(|instance| instance != resource);
    }
}

impl<H: WlrForeignToplevelHandler> Dispatch<ZwlrForeignToplevelHandleV1, Arc<ToplevelId>, H> for WlrForeignToplevelManagementState {
    fn request(
        state: &mut H,
        client: &Client,
        resource: &ZwlrForeignToplevelHandleV1,
        request: <ZwlrForeignToplevelHandleV1 as Resource>::Request,
        data: &Arc<ToplevelId>,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, H>,
    ) {
        use smithay::reexports::wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_handle_v1::Request;

        match request {
            Request::SetMaximized => state.on_toplevel_set_maximized(data.as_ref()),
            Request::UnsetMaximized => state.on_toplevel_unset_maximized(data.as_ref()),
            Request::SetMinimized => state.on_toplevel_set_minimized(data.as_ref()),
            Request::UnsetMinimized => state.on_toplevel_unset_minimized(data.as_ref()),
            Request::Activate { seat } => state.on_toplevel_activate(data.as_ref(), &seat),
            Request::Close => state.on_toplevel_close(data.as_ref()),
            Request::SetRectangle {
                surface,
                x,
                y,
                width,
                height,
            } => state.on_toplevel_set_rectangle(data.as_ref(), &surface, Rectangle::new((x, y).into(), (width, height).into())),
            Request::SetFullscreen { output } => state.on_toplevel_set_fullscreen(data.as_ref(), output.as_ref()),
            Request::UnsetFullscreen => state.on_toplevel_unset_fullscreen(data.as_ref()),
            Request::Destroy => {
                <Self as Dispatch<ZwlrForeignToplevelHandleV1, Arc<ToplevelId>, H>>::destroyed(state, client.id(), resource, data)
            }
            _ => (),
        }
    }

    fn destroyed(state: &mut H, _client: ClientId, resource: &ZwlrForeignToplevelHandleV1, data: &Arc<ToplevelId>) {
        let state = state.wlr_foreign_toplevel_management_state();
        if let Some(toplevel) = state.toplevels.get_mut(data) {
            toplevel.instances.retain(|instance| instance != resource);
        }
    }
}

fn toplevel_state_to_array(value: WindowState) -> Vec<u8> {
    [
        (WindowState::MAXIMIZED, ZwlrForeignToplevelHandleStateV1::Maximized),
        (WindowState::MINIMIZED, ZwlrForeignToplevelHandleStateV1::Minimized),
        (WindowState::ACTIVATED, ZwlrForeignToplevelHandleStateV1::Activated),
        (WindowState::FULLSCREEN, ZwlrForeignToplevelHandleStateV1::Fullscreen),
    ]
    .into_iter()
    .flat_map(|(flag, state)| value.contains(flag).then_some(state))
    .flat_map(|v| (v as u32).to_ne_bytes())
    .collect()
}

macro_rules! delegate_wlr_foreign_toplevel_management {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1: Box<dyn for<'c> Fn(&'c smithay::reexports::wayland_server::Client) -> bool + Send + Sync>
        ] => $crate::protocols::wlr_foreign_toplevel_management::WlrForeignToplevelManagementState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1: ()
        ] => $crate::protocols::wlr_foreign_toplevel_management::WlrForeignToplevelManagementState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1: std::sync::Arc<$crate::protocols::wlr_foreign_toplevel_management::ToplevelId>
        ] => $crate::protocols::wlr_foreign_toplevel_management::WlrForeignToplevelManagementState);
    };
}

pub(crate) use delegate_wlr_foreign_toplevel_management;
