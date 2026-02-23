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
    reexports::wayland_server::DisplayHandle,
    wayland::foreign_toplevel_list::{ForeignToplevelHandle, ForeignToplevelListState},
};

use crate::{Xfwl4State, backend::Backend, shell::WindowElement};

mod ext_foreign_toplevel_list;

pub struct ForeignToplevelState<BackendData> {
    pub(self) foreign_toplevel_list_state: ForeignToplevelListState,
    toplevels: HashMap<WindowElement, Toplevel>,
    _backend_data: PhantomData<BackendData>,
}

struct Toplevel {
    ext_handle: ForeignToplevelHandle,
}

impl<BackendData: Backend + 'static> ForeignToplevelState<BackendData> {
    pub fn new(dh: &DisplayHandle) -> Self {
        Self {
            foreign_toplevel_list_state: ForeignToplevelListState::new::<Xfwl4State<BackendData>>(dh),
            toplevels: HashMap::new(),
            _backend_data: PhantomData::<BackendData>,
        }
    }

    pub fn toplevel_created(&mut self, window: &WindowElement) {
        let title = window.title().unwrap_or_default();
        let app_id = window.app_id().unwrap_or_default();
        let ext_handle = self
            .foreign_toplevel_list_state
            .new_toplevel::<Xfwl4State<BackendData>>(title, app_id);
        self.toplevels.insert(window.clone(), Toplevel { ext_handle });
    }

    pub fn toplevel_changed(&self, window: &WindowElement, title: Option<&str>, app_id: Option<&str>) {
        if let Some(toplevel) = self.toplevels.get(window)
            && (title.is_some() || app_id.is_some())
        {
            if let Some(title) = title {
                toplevel.ext_handle.send_title(title);
            }
            if let Some(app_id) = app_id {
                toplevel.ext_handle.send_app_id(app_id);
            }

            toplevel.ext_handle.send_done();
        }
    }

    pub fn toplevel_destroyed(&mut self, window: &WindowElement) {
        if let Some(toplevel) = self.toplevels.remove(window) {
            toplevel.ext_handle.send_closed();
        }
    }
}
