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
    cell::RefCell,
    ffi::OsStr,
    path::PathBuf,
    process::{Child, Command},
    rc::Rc,
};

use wayland_client::protocol::wl_registry::WlRegistry;

use crate::{
    build_config::BUILD_BINDIR,
    ui::{
        compositor_ui_protocol::{TabwinState, WindowMenuState, WindowMenuUi, proto::xfwl4_ui_manager_v1::Xfwl4UiManagerV1},
        dialog::DialogState,
        tabwin::{TABWIN_DEFAULT_CSS, TABWIN_WIDGET_NAME},
        wayland_client_gsource::WaylandClientSource,
    },
};

mod color_ops;
mod compositor_ui_protocol;
mod dialog;
mod gtk_settings;
mod gtk_settings_sync;
mod image_menu_item;
pub mod tabwin;
mod theme;
mod ui_main;
mod util;
mod wayland_client_gsource;
pub mod window_menu;

pub use ui_main::ui_main;

#[derive(Debug, Default)]
struct UiProcessState {
    source: Option<WaylandClientSource>,
    registry: Option<WlRegistry>,
    pub ui_manager: Option<Xfwl4UiManagerV1>,

    pub tabwin_state: Option<TabwinState>,
    pub tabwin_style_provider: Option<gtk::CssProvider>,

    pub window_menu_anchor: gtk::Window,
    pub window_menu_state: Option<WindowMenuState>,
    pub window_menu: Rc<RefCell<Option<WindowMenuUi>>>,

    pub dialog_states: Vec<DialogState>,
}

pub fn start<I, K, V>(env: I) -> anyhow::Result<Child>
where
    I: Iterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let self_exe = match std::env::current_exe() {
        Ok(self_exe) => self_exe,
        Err(err) => {
            tracing::warn!("Unable to determine xfwl4 executable path: {err}");
            let mut self_exe = PathBuf::from(BUILD_BINDIR);
            self_exe.push(env!("CARGO_PKG_NAME"));
            tracing::warn!("Falling back to {}", self_exe.display());
            self_exe
        }
    };
    let child = Command::new(self_exe).arg("--ui-process").envs(env).spawn()?;
    Ok(child)
}
