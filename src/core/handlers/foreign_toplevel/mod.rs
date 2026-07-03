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

use std::{collections::HashMap, marker::PhantomData};

use smithay::{
    output::Output,
    reexports::{
        wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
        wayland_server::{Client, Dispatch, DisplayHandle},
    },
    wayland::foreign_toplevel_list::{ForeignToplevelHandle, ForeignToplevelListState},
};

use crate::{
    backend::Backend,
    core::{
        shell::{WindowElement, WindowState},
        state::Xfwl4State,
        util::ClientExt,
    },
    protocols::{
        ext_workspace::{ExtWorkspaceHandler, ExtWorkspaceState},
        foreign_toplevel_management::{
            ForeignToplevelManagementState, ToplevelChangedInput, ToplevelCreatedInput, ToplevelHandleData, ToplevelId,
            wlr_foreign_toplevel_management::WlrForeignToplevelHandler, xfce_foreign_toplevel_management::IconSize,
        },
    },
    util::icon::Argb32Pixels,
};

mod ext_foreign_toplevel_list;
mod wlr_foreign_toplevel_management;
mod xfce_foreign_toplevel_management;

pub struct ForeignToplevelState<BackendData> {
    pub(self) foreign_toplevel_list_state: ForeignToplevelListState,
    pub(self) foreign_toplevel_management_state: ForeignToplevelManagementState,
    toplevels: HashMap<WindowElement, Toplevel>,
    ext_windows: HashMap<String, WindowElement>,
    wlr_windows: HashMap<ToplevelId, WindowElement>,
    _backend_data: PhantomData<BackendData>,
}

struct Toplevel {
    ext_handle: ForeignToplevelHandle,
    wlr_id: ToplevelId,
}

impl<BackendData: Backend + 'static> ForeignToplevelState<BackendData> {
    pub fn new(dh: &DisplayHandle) -> Self {
        Self {
            foreign_toplevel_list_state: ForeignToplevelListState::new_with_filter::<Xfwl4State<BackendData>>(dh, |client| {
                !client.has_security_context()
            }),
            foreign_toplevel_management_state: ForeignToplevelManagementState::new::<Xfwl4State<BackendData>, _>(dh, |client| {
                !client.has_security_context()
            }),
            toplevels: HashMap::new(),
            ext_windows: HashMap::new(),
            wlr_windows: HashMap::new(),
            _backend_data: PhantomData::<BackendData>,
        }
    }

    pub fn window_for_handle(&self, toplevel: &ForeignToplevelHandle) -> Option<WindowElement> {
        self.ext_windows.get(&toplevel.identifier()).cloned()
    }

    pub fn toplevel_created<H>(&mut self, window: &WindowElement, outputs: Vec<Output>, workspace_id: Option<String>)
    where
        H: WlrForeignToplevelHandler + Dispatch<ZwlrForeignToplevelHandleV1, ToplevelHandleData>,
    {
        let title = window.title().unwrap_or_default();
        let app_id = window.app_id().unwrap_or_default();
        let state = window.state();
        let parent = window
            .parent()
            .and_then(|parent| self.toplevels.get(&parent))
            .map(|parent| &parent.wlr_id)
            .cloned();
        let (icon_name, icon_sizes) = {
            let props = window.props();
            let name = props.window_icon.window_icon_name().map(ToOwned::to_owned);
            let sizes = props
                .window_icon
                .window_icon_rasters()
                .iter()
                .map(|raster| IconSize::new(raster.size.w, raster.size.h, raster.scale))
                .collect();
            (name, sizes)
        };

        let ext_handle = self
            .foreign_toplevel_list_state
            .new_toplevel::<Xfwl4State<BackendData>>(&title, &app_id);
        let wlr_id = self.foreign_toplevel_management_state.toplevel_created::<H>(ToplevelCreatedInput {
            title,
            app_id,
            state,
            outputs,
            parent,
            workspace_id,
            icon_name,
            icon_sizes,
        });

        self.ext_windows.insert(ext_handle.identifier(), window.clone());
        self.wlr_windows.insert(wlr_id.clone(), window.clone());
        self.toplevels.insert(window.clone(), Toplevel { ext_handle, wlr_id });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn toplevel_changed<D: ExtWorkspaceHandler>(
        &mut self,
        window: &WindowElement,
        workspace_state: &ExtWorkspaceState<D>,
        title: Option<&str>,
        app_id: Option<&str>,
        state: Option<WindowState>,
        outputs_added: Vec<Output>,
        outputs_removed: Vec<Output>,
        parent: Option<Option<&WindowElement>>,
        workspace_id: Option<Option<&str>>,
        icon_name: Option<Option<&str>>,
        icon_rasters: Option<&[Argb32Pixels]>,
    ) {
        let parent = parent.map(|parent| parent.and_then(|parent| self.toplevels.get(parent).map(|parent| parent.wlr_id.clone())));

        if let Some(toplevel) = self.toplevels.get(window) {
            if title.is_some() || app_id.is_some() {
                if let Some(title) = title {
                    toplevel.ext_handle.send_title(title);
                }
                if let Some(app_id) = app_id {
                    toplevel.ext_handle.send_app_id(app_id);
                }

                toplevel.ext_handle.send_done();
            }

            self.foreign_toplevel_management_state.toplevel_changed(
                &toplevel.wlr_id,
                ToplevelChangedInput {
                    title: title.map(ToOwned::to_owned),
                    app_id: app_id.map(ToOwned::to_owned),
                    state,
                    outputs_added,
                    outputs_removed,
                    parent,
                    workspace_id: workspace_id.map(|wid| wid.map(ToOwned::to_owned)),
                    icon_name: icon_name.map(|name| name.map(ToOwned::to_owned)),
                    icon_sizes: icon_rasters.map(|rasters| {
                        rasters
                            .iter()
                            .map(|raster| IconSize::new(raster.size.w, raster.size.h, raster.scale))
                            .collect()
                    }),
                },
                workspace_state,
            );
        }
    }

    pub fn toplevel_destroyed(&mut self, window: &WindowElement) {
        if let Some(toplevel) = self.toplevels.remove(window) {
            self.ext_windows.remove(&toplevel.ext_handle.identifier());
            self.wlr_windows.remove(&toplevel.wlr_id);
            toplevel.ext_handle.send_closed();
            self.foreign_toplevel_management_state.toplevel_destroyed(&toplevel.wlr_id);
        }
    }

    pub(super) fn flush_client_workspace_events(
        &mut self,
        ext_workspace_state: &ExtWorkspaceState<Xfwl4State<BackendData>>,
        client: &Client,
    ) {
        self.foreign_toplevel_management_state
            .flush_client_workspace_events(ext_workspace_state, client);
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    fn window_for_toplevel_id(&self, toplevel_id: &ToplevelId) -> Option<WindowElement> {
        self.core
            .protocol_delegates
            .foreign_toplevel_state
            .wlr_windows
            .get(toplevel_id)
            .cloned()
    }
}
