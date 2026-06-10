// xfwl4 -- Wayland compositor for the Xfce Desktop Environmen
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

use std::ops::Deref;

use anyhow::anyhow;
use smithay::{
    backend::renderer::{BufferType, buffer_type},
    desktop::WindowSurface,
    output::{self, Output},
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Size},
};

use crate::{
    backend::Backend,
    core::{
        drawing::wireframe::Wireframe,
        shell::{
            WindowElement, WindowFlags, WindowIcon, WorkspaceLocation,
            xdg::{
                XdgSurfaceProps, app_name_for_xdg_toplevel, desktop_app_info_for_xdg_toplevel, icon_for_xdg_toplevel,
                window_title_for_xdg_toplevel,
            },
        },
        state::Xfwl4State,
        util::{ImageData, shm_buffer_to_image_data},
        workspaces::WindowStackingLayer,
    },
    protocols::xfwl4_compositor_ui::{Icon, Pixels, TabwinWindow},
    ui::tabwin::{self, TABWIN_WINDOW_TITLE},
};

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct CycleFlags: u8 {
        const INCLUDE_HIDDEN = (1 << 0);
        const INCLUDE_SKIP_TASKBAR = (1 << 2);
        const INCLUDE_SKIP_PAGER = (1 << 3);
        const INCLUDE_TRANSIENTS = (1 << 4);
        const INCLUDE_MODAL_PARENTS = (1 << 5);
        const INCLUDE_UTILITY = (1 << 6);
        const INCLUDE_ALL_WORKSPACES = (1 << 7);
    }
}

#[derive(Debug, Default)]
pub(in crate::core) struct CycleList {
    windows: Vec<WindowElement>,
}

impl CycleList {
    pub fn add_new(&mut self, window: WindowElement) {
        self.windows.push(window);
    }

    pub fn focused(&mut self, window: &WindowElement) {
        if let Some(pos) = self.windows.iter().position(|a_window| a_window == window)
            && pos != 0
        {
            let window = self.windows.remove(pos);
            self.windows.insert(0, window);
        }
    }

    pub fn move_to_back(&mut self, window: &WindowElement) {
        if let Some(pos) = self.windows.iter().position(|a_window| a_window == window)
            && pos != self.windows.len() - 1
        {
            let window = self.windows.remove(pos);
            self.windows.push(window);
        }
    }

    pub fn remove(&mut self, window: &WindowElement) -> Option<WindowElement> {
        if let Some(pos) = self.windows.iter().position(|a_window| a_window == window) {
            Some(self.windows.remove(pos))
        } else {
            None
        }
    }
}

impl Deref for CycleList {
    type Target = [WindowElement];

    fn deref(&self) -> &Self::Target {
        &self.windows
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub(in crate::core) fn window_is_tabwin(&self, window: &WindowElement, surface: &WlSurface) -> bool {
        self.core.client_is_ui_thread(surface.client())
            && window
                .0
                .toplevel()
                .and_then(window_title_for_xdg_toplevel)
                .is_some_and(|title| title == TABWIN_WINDOW_TITLE)
    }

    fn find_tabwin(&self) -> Option<WindowElement> {
        let workspace = self.core.workspace_manager.active_workspace();
        workspace.find_window(|elem| {
            self.core.client_is_ui_thread(elem.wl_surface().and_then(|surf| surf.client()))
                && elem
                    .0
                    .toplevel()
                    .and_then(window_title_for_xdg_toplevel)
                    .is_some_and(|title| title == TABWIN_WINDOW_TITLE)
        })
    }

    pub(in crate::core) fn place_tabwin(&mut self, window: &WindowElement, size: Size<i32, Logical>) {
        if let Some(output) = self.output_under_pointer()
            && let Some(output_geo) = self.core.workspace_manager.output_geometry(&output)
        {
            let window_size = size.to_f64();
            let output_size = output_geo.size.to_f64();
            let new_x = output_geo.loc.x as f64 + (output_size.w - window_size.w) / 2.;
            let new_y = output_geo.loc.y as f64 + (output_size.h - window_size.h) / 2.;
            let new_location = Point::new(new_x as i32, new_y as i32);

            self.core.cycling_windows = true;

            window.props().flags |= WindowFlags::NO_CYCLE;
            self.set_window_stacking_layer(window, WindowStackingLayer::System);
            self.new_window(window.clone(), new_location, true, None);
            self.focus_target(window.clone(), SERIAL_COUNTER.next_serial(), None);

            let tabwin_geo = Rectangle::new(new_location, size);
            self.start_tabwin_grab(window.clone(), self.core.seat.clone(), tabwin_geo);
        }
    }

    pub(in crate::core) fn collect_tabwin_windows(&mut self, output: &Output) -> Vec<TabwinWindow> {
        let cycle_flags = self.build_cycle_flags();
        let cycle_list = self.core.cycle_list.windows.clone();
        cycle_list
            .into_iter()
            .flat_map(|window| self.window_to_tabwin_window(&window, output, cycle_flags))
            .collect::<Vec<_>>()
    }

    pub(in crate::core) fn add_window_to_tabwin(&mut self, window: &WindowElement) {
        if let Some(tabwin_window) = self
            .core
            .workspace_manager
            .active_workspace()
            .find_window(|elem| elem.wl_surface().is_some_and(|surface| self.window_is_tabwin(window, &surface)))
            && let Some(output) = self
                .core
                .workspace_manager
                .active_workspace()
                .outputs_for_window(&tabwin_window)
                .first()
        {
            let cycle_flags = self.build_cycle_flags();
            if let Some(tabwin_window) = self.window_to_tabwin_window(window, output, cycle_flags)
                && let Err(err) = self.core.compositor_ui_state.tabwin_add_window::<Self>(tabwin_window)
            {
                tracing::warn!("Failed to add new window to tabwin: {err}");
            }
        }
    }

    fn build_cycle_flags(&self) -> CycleFlags {
        let mut cycle_flags = CycleFlags::empty();
        if self.core.config.cycle_hidden() {
            cycle_flags |= CycleFlags::INCLUDE_HIDDEN;
        }
        if !self.core.config.cycle_minimum() {
            cycle_flags |= CycleFlags::INCLUDE_SKIP_PAGER;
            cycle_flags |= CycleFlags::INCLUDE_SKIP_TASKBAR;
        }
        if !self.core.config.cycle_apps_only() {
            cycle_flags |= CycleFlags::INCLUDE_TRANSIENTS;
            cycle_flags |= CycleFlags::INCLUDE_MODAL_PARENTS;
            cycle_flags |= CycleFlags::INCLUDE_UTILITY;
        }
        if self.core.config.cycle_workspaces() {
            cycle_flags |= CycleFlags::INCLUDE_ALL_WORKSPACES;
        }
        cycle_flags
    }

    fn window_to_tabwin_window(&mut self, window: &WindowElement, output: &Output, cycle_flags: CycleFlags) -> Option<TabwinWindow> {
        Some(window)
            .filter(|window| !window.props().flags.contains(WindowFlags::NO_CYCLE))
            .filter(|window| {
                let workspace_loc = window.props().workspace_loc;
                cycle_flags.contains(CycleFlags::INCLUDE_ALL_WORKSPACES)
                    || workspace_loc == WorkspaceLocation::Single(self.core.workspace_manager.active_workspace_index())
                    || workspace_loc == WorkspaceLocation::All
            })
            .filter(|window| cycle_flags.contains(CycleFlags::INCLUDE_HIDDEN) || !window.minimized())
            .filter(|window| cycle_flags.contains(CycleFlags::INCLUDE_TRANSIENTS) || window.modal() || !window.has_parent())
            .filter(|window| {
                cycle_flags.contains(CycleFlags::INCLUDE_MODAL_PARENTS)
                    || !window.has_children()
                    || !window.children().iter().any(|child| child.modal())
            })
            .filter(|window| match window.0.underlying_surface() {
                WindowSurface::Wayland(_) => true,
                #[cfg(feature = "xwayland")]
                WindowSurface::X11(surface) => {
                    use smithay::xwayland::xwm::WmWindowType;

                    let wmtype = surface.window_type();
                    !surface.is_override_redirect()
                        && (cycle_flags.contains(CycleFlags::INCLUDE_UTILITY) || wmtype.is_none_or(|ty| ty != WmWindowType::Utility))
                        && (cycle_flags.contains(CycleFlags::INCLUDE_SKIP_PAGER) || !surface.is_skip_pager())
                        && (cycle_flags.contains(CycleFlags::INCLUDE_SKIP_TASKBAR) || !surface.is_skip_taskbar())
                        && wmtype.is_none_or(|wmtype| {
                            !matches!(
                                wmtype,
                                WmWindowType::Combo
                                    | WmWindowType::Desktop
                                    | WmWindowType::Dnd
                                    | WmWindowType::Dock
                                    | WmWindowType::DropdownMenu
                                    | WmWindowType::Menu
                                    | WmWindowType::Notification
                                    | WmWindowType::PopupMenu
                                    | WmWindowType::Splash
                                    | WmWindowType::Toolbar
                                    | WmWindowType::Tooltip
                            )
                        })
                    // TODO: check _NET_WM_STATE_SKIP_TASKBAR and _NET_WM_STATE_SKIP_PAGER
                    // once smithay exposes those atoms
                }
            })
            .and_then(|window| {
                let client_data = match window.0.underlying_surface() {
                    WindowSurface::Wayland(toplevel_surface) => {
                        let is_minimized = window
                            .0
                            .user_data()
                            .get::<XdgSurfaceProps>()
                            .map(|props| props.0.lock().unwrap().is_minimized)
                            .unwrap_or(false);
                        let app_info = desktop_app_info_for_xdg_toplevel(toplevel_surface);
                        let app_name = app_name_for_xdg_toplevel(toplevel_surface, app_info.as_ref());
                        let title = window_title_for_xdg_toplevel(toplevel_surface);
                        let icon = icon_for_xdg_toplevel(toplevel_surface, output.current_scale().integer_scale(), app_info.as_ref())
                            .and_then(|icon| {
                                self.window_icon_to_image_data(&icon)
                                    .inspect_err(|err| tracing::info!("Failed to get window icon: {err}"))
                                    .ok()
                            });

                        (app_name, title, icon, is_minimized)
                    }

                    #[cfg(feature = "xwayland")]
                    WindowSurface::X11(x11_surface) => {
                        use crate::core::util::prettify_name;

                        let app_name = prettify_name(&x11_surface.class());
                        let icon = self.window_icon_for_x11_window(x11_surface);

                        (app_name, Some(x11_surface.title()), icon, x11_surface.is_hidden())
                    }
                };

                match client_data {
                    (app_name, Some(title), app_icon, is_minimized) => {
                        let output_scale = match output.current_scale() {
                            output::Scale::Integer(i) => i as f64,
                            output::Scale::Fractional(f) => f,
                            output::Scale::Custom { fractional, .. } => fractional,
                        }
                        .into();
                        let preview = self
                            .window_to_image_data(&window.0, tabwin::WIN_PREVIEW_SIZE as u32, output_scale)
                            .inspect_err(|err| tracing::info!("Failed to get window preview: {err}"))
                            .ok();
                        let app_icon = app_icon.map(|image_data| match image_data {
                            ImageData::NamedIcon(name) => Icon::Named(name),
                            ImageData::File(path) => Icon::File(path),
                            ImageData::RgbaPixels { bytes, width, height } => Icon::Pixels(Pixels { bytes, width, height }),
                        });

                        Some(TabwinWindow {
                            window_id: window.window_id(),
                            app_name,
                            title,
                            preview,
                            app_icon,
                            is_minimized,
                        })
                    }
                    _ => None,
                }
            })
    }

    pub(in crate::core) fn show_tabwin_window_wireframe(&mut self, window: &WindowElement) {
        if self.find_tabwin().is_some()
            && let Some(workspace) = self.core.workspace_manager.workspace_for_window(window)
            && let Some(geometry) = workspace
                .window_geometry(window)
                .or_else(|| workspace.minimized_window_geometry(window))
        {
            let mut wireframe = self
                .core
                .wireframe
                .take()
                .unwrap_or_else(|| Wireframe::new(Rectangle::zero(), &self.core.config));
            wireframe.update_location(geometry.loc);
            wireframe.update_size(geometry.size);
            self.core.wireframe = Some(wireframe);
        } else {
            self.core.wireframe = None;
        }
    }

    pub(in crate::core) fn end_window_cycling(&mut self) {
        self.core.cycling_windows = false;
        self.core.wireframe = None;
    }

    pub(in crate::core) fn window_icon_to_image_data(&mut self, window_icon: &WindowIcon) -> anyhow::Result<ImageData> {
        match window_icon {
            WindowIcon::Named(icon_name) => Ok(ImageData::NamedIcon(icon_name.clone())),
            WindowIcon::File(path) => Ok(ImageData::File(path.clone())),
            WindowIcon::Buffer(buffer) => match buffer_type(buffer) {
                Some(BufferType::Shm) => shm_buffer_to_image_data(buffer),
                Some(BufferType::Dma) => self.dmabuf_to_image_data(buffer),
                Some(ty) => Err(anyhow!("unsupported buffer type {ty:?} for icon")),
                None => Err(anyhow!("buffer somehow has no type")),
            },
        }
    }
}
