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

use smithay::{delegate_foreign_toplevel_list, wayland::foreign_toplevel_list::ForeignToplevelListHandler};

use crate::{backend::Backend, core::state::Xfwl4State};

impl<BackendData: Backend + 'static> ForeignToplevelListHandler for Xfwl4State<BackendData> {
    fn foreign_toplevel_list_state(&mut self) -> &mut smithay::wayland::foreign_toplevel_list::ForeignToplevelListState {
        &mut self.core.protocol_delegates.foreign_toplevel_state.foreign_toplevel_list_state
    }
}

delegate_foreign_toplevel_list!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
