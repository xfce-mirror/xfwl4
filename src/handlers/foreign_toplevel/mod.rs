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
    reexports::wayland_server::DisplayHandle,
    wayland::foreign_toplevel_list::{ForeignToplevelHandle, ForeignToplevelListState},
};

use crate::{
    Xfwl4State,
    backend::Backend,
    protocols::wlr_foreign_toplevel_management::{ToplevelId, WlrForeignToplevelHandler, WlrForeignToplevelManagementState},
    shell::{WindowElement, WindowState},
};

mod ext_foreign_toplevel_list;
mod wlr_foreign_toplevel_management;

pub struct ForeignToplevelState<BackendData> {
    pub(self) foreign_toplevel_list_state: ForeignToplevelListState,
    pub(self) wlr_foreign_toplevel_management_state: WlrForeignToplevelManagementState,
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
            foreign_toplevel_list_state: ForeignToplevelListState::new::<Xfwl4State<BackendData>>(dh),
            wlr_foreign_toplevel_management_state: WlrForeignToplevelManagementState::new::<Xfwl4State<BackendData>>(dh),
            toplevels: HashMap::new(),
            ext_windows: HashMap::new(),
            wlr_windows: HashMap::new(),
            _backend_data: PhantomData::<BackendData>,
        }
    }

    pub fn window_for_handle(&self, toplevel: &ForeignToplevelHandle) -> Option<WindowElement> {
        self.ext_windows.get(&toplevel.identifier()).cloned()
    }

    pub fn toplevel_created<H: WlrForeignToplevelHandler>(
        &mut self,
        window: &WindowElement,
        outputs: Vec<Output>,
        parent: Option<&WindowElement>,
    ) {
        let title = window.title().unwrap_or_default();
        let app_id = window.app_id().unwrap_or_default();
        let state = window.state();
        let parent = parent
            .and_then(|parent| self.toplevels.get(parent))
            .map(|parent| &parent.wlr_id)
            .cloned();

        let ext_handle = self
            .foreign_toplevel_list_state
            .new_toplevel::<Xfwl4State<BackendData>>(&title, &app_id);
        let wlr_id = self
            .wlr_foreign_toplevel_management_state
            .toplevel_created::<H>(title, app_id, state, outputs, parent);

        self.ext_windows.insert(ext_handle.identifier(), window.clone());
        self.wlr_windows.insert(wlr_id.clone(), window.clone());
        self.toplevels.insert(window.clone(), Toplevel { ext_handle, wlr_id });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn toplevel_changed(
        &mut self,
        window: &WindowElement,
        title: Option<&str>,
        app_id: Option<&str>,
        states_added: WindowState,
        states_removed: WindowState,
        outputs_added: Vec<Output>,
        outputs_removed: Vec<Output>,
        parent: Option<Option<&WindowElement>>,
    ) {
        let parent = parent.map(|parent| parent.and_then(|parent| self.toplevels.get(parent).map(|parent| parent.wlr_id.clone())));

        if let Some(toplevel) = self.toplevels.get(window)
            && (title.is_some()
                || app_id.is_some()
                || states_added != WindowState::empty()
                || states_removed != WindowState::empty()
                || !outputs_added.is_empty()
                || !outputs_removed.is_empty()
                || parent.is_some())
        {
            if let Some(title) = title {
                toplevel.ext_handle.send_title(title);
            }
            if let Some(app_id) = app_id {
                toplevel.ext_handle.send_app_id(app_id);
            }

            toplevel.ext_handle.send_done();

            self.wlr_foreign_toplevel_management_state.toplevel_changed(
                &toplevel.wlr_id,
                title,
                app_id,
                states_added,
                states_removed,
                outputs_added,
                outputs_removed,
                parent,
            );
        }
    }

    pub fn toplevel_destroyed(&mut self, window: &WindowElement) {
        if let Some(toplevel) = self.toplevels.remove(window) {
            self.ext_windows.remove(&toplevel.ext_handle.identifier());
            self.wlr_windows.remove(&toplevel.wlr_id);
            toplevel.ext_handle.send_closed();
            self.wlr_foreign_toplevel_management_state.toplevel_destroyed(&toplevel.wlr_id);
        }
    }
}
