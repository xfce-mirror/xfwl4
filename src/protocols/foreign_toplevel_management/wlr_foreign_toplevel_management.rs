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
use rand::distr::{Alphanumeric, SampleString};
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
    wayland::{Dispatch2, GlobalDispatch2},
};

use crate::{
    core::shell::WindowState,
    protocols::{
        ClientFilter, GlobalData,
        foreign_toplevel_management::{ToplevelHandleData, ToplevelId},
    },
};

const USED_STATES: WindowState = WindowState::from_bits_truncate(
    WindowState::ACTIVATED.bits() | WindowState::MINIMIZED.bits() | WindowState::MAXIMIZED.bits() | WindowState::FULLSCREEN.bits(),
);
const USED_STATES_V1: WindowState =
    WindowState::from_bits_truncate(WindowState::ACTIVATED.bits() | WindowState::MINIMIZED.bits() | WindowState::MAXIMIZED.bits());

pub struct WlrForeignToplevelManagementGlobalData {
    filter: ClientFilter,
}

pub struct WlrForeignToplevelManagementState {
    dh: DisplayHandle,
    _global: GlobalId,
    manager_instances: Vec<ZwlrForeignToplevelManagerV1>,
    toplevels: IndexMap<Arc<ToplevelId>, WlrForeignToplevel>,
}

impl WlrForeignToplevelManagementState {
    pub(super) fn new<H, F>(dh: &DisplayHandle, filter: F) -> Self
    where
        H: WlrForeignToplevelHandler + GlobalDispatch<ZwlrForeignToplevelManagerV1, WlrForeignToplevelManagementGlobalData>,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global =
            dh.create_global::<H, ZwlrForeignToplevelManagerV1, _>(3, WlrForeignToplevelManagementGlobalData { filter: Box::new(filter) });
        Self {
            dh: dh.clone(),
            _global: global,
            manager_instances: Vec::new(),
            toplevels: IndexMap::new(),
        }
    }

    pub(super) fn toplevel_created<H>(
        &mut self,
        title: impl Into<String>,
        app_id: impl Into<String>,
        state: WindowState,
        outputs: Vec<Output>,
        parent: Option<ToplevelId>,
    ) -> Arc<ToplevelId>
    where
        H: WlrForeignToplevelHandler + Dispatch<ZwlrForeignToplevelHandleV1, ToplevelHandleData>,
    {
        let toplevel_id = Arc::new(ToplevelId(Alphanumeric.sample_string(&mut rand::rng(), 32)));
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
                && let Ok(instance) = client.create_resource::<ZwlrForeignToplevelHandleV1, _, H>(
                    &self.dh,
                    manager.version(),
                    ToplevelHandleData(Arc::clone(&toplevel_id)),
                )
            {
                manager.toplevel(&instance);

                instance.title(toplevel.title.clone());
                instance.app_id(toplevel.app_id.clone());
                if instance.version() >= 2 {
                    instance.state(toplevel_state_to_array(toplevel.state));
                } else {
                    instance.state(toplevel_state_to_array(toplevel.state.intersection(USED_STATES_V1)));
                }

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

        self.toplevels.insert(Arc::clone(&toplevel_id), toplevel);
        toplevel_id
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn toplevel_changed(
        &mut self,
        toplevel_id: &ToplevelId,
        title: Option<String>,
        app_id: Option<String>,
        state: Option<WindowState>,
        outputs_added: Vec<Output>,
        outputs_removed: Vec<Output>,
        parent: Option<Option<ToplevelId>>,
    ) -> bool {
        let state = state.map(|state| state.intersection(USED_STATES));

        let (sent_changes, changed_title, changed_app_id, added_outputs, removed_outputs, changed_parent) =
            if let Some(toplevel) = self.toplevels.get(toplevel_id) {
                let changed_title = title.filter(|title| toplevel.title != *title);
                let changed_app_id = app_id.filter(|app_id| toplevel.app_id != *app_id);
                let added_outputs = outputs_added
                    .into_iter()
                    .filter(|output_to_add| !toplevel.outputs.contains(output_to_add))
                    .collect::<Vec<_>>();
                let removed_outputs = outputs_removed
                    .into_iter()
                    .filter(|output_to_remove| toplevel.outputs.contains(output_to_remove))
                    .collect::<Vec<_>>();
                let changed_parent = parent.filter(|parent| toplevel.parent != *parent);

                let state_changed = state.is_some_and(|state| state != toplevel.state);
                let state_v1_changed =
                    state.is_some_and(|state| state.intersection(USED_STATES_V1) != toplevel.state.intersection(USED_STATES_V1));

                if changed_title.is_some()
                    || changed_app_id.is_some()
                    || state_changed
                    || state_v1_changed
                    || !added_outputs.is_empty()
                    || !removed_outputs.is_empty()
                    || changed_parent.is_some()
                {
                    for instance in &toplevel.instances {
                        if let Some(client) = instance.client() {
                            if let Some(title) = &changed_title {
                                instance.title(title.clone());
                            }

                            if let Some(app_id) = &changed_app_id {
                                instance.app_id(app_id.clone());
                            }

                            if let Some(state) = state {
                                if state_changed && instance.version() >= 2 {
                                    instance.state(toplevel_state_to_array(state));
                                } else if state_v1_changed && instance.version() < 2 {
                                    instance.state(toplevel_state_to_array(state.intersection(USED_STATES_V1)));
                                }
                            }

                            for output in &added_outputs {
                                for output_instance in output.client_outputs(&client) {
                                    instance.output_enter(&output_instance);
                                }
                            }

                            for output in &removed_outputs {
                                for output_instance in output.client_outputs(&client) {
                                    instance.output_leave(&output_instance);
                                }
                            }

                            if let Some(parent) = &changed_parent
                                && let Some(parent) = parent.as_ref().and_then(|parent| self.toplevels.get(parent))
                            {
                                for parent_instance in &parent.instances {
                                    if parent_instance.client().as_ref() == Some(&client) {
                                        instance.parent(Some(parent_instance));
                                    }
                                }
                            }
                        }
                    }

                    (true, changed_title, changed_app_id, added_outputs, removed_outputs, changed_parent)
                } else {
                    (false, None, None, Vec::new(), Vec::new(), None)
                }
            } else {
                (false, None, None, Vec::new(), Vec::new(), None)
            };

        if let Some(toplevel) = self.toplevels.get_mut(toplevel_id) {
            if let Some(title) = changed_title {
                toplevel.title = title.to_owned();
            }
            if let Some(app_id) = changed_app_id {
                toplevel.app_id = app_id.to_owned();
            }
            if let Some(state) = state {
                toplevel.state = state;
            }
            for output in added_outputs {
                if !toplevel.outputs.contains(&output) {
                    toplevel.outputs.push(output);
                }
            }
            toplevel.outputs.retain(|output| !removed_outputs.contains(output));
            if let Some(parent) = changed_parent {
                toplevel.parent = parent;
            }
        }

        sent_changes
    }

    pub(super) fn toplevel_destroyed(&mut self, toplevel_id: &ToplevelId) {
        if let Some(toplevel) = self.toplevels.shift_remove(toplevel_id) {
            for instance in toplevel.instances {
                instance.closed();
            }
        }
    }

    pub(super) fn send_done(&mut self, toplevel_id: &ToplevelId) {
        if let Some(toplevel) = self.toplevels.get(toplevel_id) {
            for instance in &toplevel.instances {
                instance.done();
            }
        }
    }

    pub(super) fn toplevel_for_handle(&self, handle: &ZwlrForeignToplevelHandleV1) -> Option<Arc<ToplevelId>> {
        self.toplevels
            .iter()
            .find_map(|(id, toplevel)| toplevel.instances.contains(handle).then(|| Arc::clone(id)))
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

pub trait WlrForeignToplevelHandler: 'static {
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

impl<D: WlrForeignToplevelHandler> GlobalDispatch2<ZwlrForeignToplevelManagerV1, D> for WlrForeignToplevelManagementGlobalData
where
    D: Dispatch<ZwlrForeignToplevelManagerV1, GlobalData> + Dispatch<ZwlrForeignToplevelHandleV1, ToplevelHandleData>,
{
    fn bind(
        &self,
        state: &mut D,
        handle: &DisplayHandle,
        client: &Client,
        resource: New<ZwlrForeignToplevelManagerV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        let manager_instance = data_init.init(resource, GlobalData);
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
            if let Ok(instance) = client.create_resource::<ZwlrForeignToplevelHandleV1, _, D>(
                handle,
                manager_instance.version(),
                ToplevelHandleData(Arc::clone(toplevel_id)),
            ) {
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

    fn can_view(&self, client: &Client) -> bool {
        (self.filter)(client)
    }
}

impl<D: WlrForeignToplevelHandler> Dispatch2<ZwlrForeignToplevelManagerV1, D> for GlobalData {
    fn request(
        &self,
        state: &mut D,
        client: &Client,
        resource: &ZwlrForeignToplevelManagerV1,
        request: <ZwlrForeignToplevelManagerV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        use smithay::reexports::wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_manager_v1::Request;

        if let Request::Stop = request {
            self.destroyed(state, client.id(), resource);
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &ZwlrForeignToplevelManagerV1) {
        state
            .wlr_foreign_toplevel_management_state()
            .manager_instances
            .retain(|instance| instance != resource);
    }
}

impl<D: WlrForeignToplevelHandler> Dispatch2<ZwlrForeignToplevelHandleV1, D> for ToplevelHandleData {
    fn request(
        &self,
        state: &mut D,
        client: &Client,
        resource: &ZwlrForeignToplevelHandleV1,
        request: <ZwlrForeignToplevelHandleV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        use smithay::reexports::wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_handle_v1::Request;

        match request {
            Request::SetMaximized => state.on_toplevel_set_maximized(self.0.as_ref()),
            Request::UnsetMaximized => state.on_toplevel_unset_maximized(self.0.as_ref()),
            Request::SetMinimized => state.on_toplevel_set_minimized(self.0.as_ref()),
            Request::UnsetMinimized => state.on_toplevel_unset_minimized(self.0.as_ref()),
            Request::Activate { seat } => state.on_toplevel_activate(self.0.as_ref(), &seat),
            Request::Close => state.on_toplevel_close(self.0.as_ref()),
            Request::SetRectangle {
                surface,
                x,
                y,
                width,
                height,
            } => state.on_toplevel_set_rectangle(self.0.as_ref(), &surface, Rectangle::new((x, y).into(), (width, height).into())),
            Request::SetFullscreen { output } => state.on_toplevel_set_fullscreen(self.0.as_ref(), output.as_ref()),
            Request::UnsetFullscreen => state.on_toplevel_unset_fullscreen(self.0.as_ref()),
            Request::Destroy => {
                self.destroyed(state, client.id(), resource);
            }
            _ => (),
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &ZwlrForeignToplevelHandleV1) {
        let state = state.wlr_foreign_toplevel_management_state();
        if let Some(toplevel) = state.toplevels.get_mut(&self.0) {
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
