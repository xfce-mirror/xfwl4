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

use anyhow::anyhow;
use smithay::{
    backend::renderer::{BufferType, buffer_type},
    desktop::{WindowSurface, space::RenderZindex},
    output::{self, Output},
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    utils::{Logical, Point, Size},
    wayland::seat::WaylandFocus,
};

use crate::{
    backend::Backend,
    core::{
        shell::{
            WindowElement, WindowIcon,
            xdg::{
                XdgSurfaceProps, app_name_for_xdg_toplevel, desktop_app_info_for_xdg_toplevel, icon_for_xdg_toplevel,
                window_title_for_xdg_toplevel,
            },
        },
        state::Xfwl4State,
        util::{ImageData, shm_buffer_to_image_data},
    },
    ui::tabwin::{self, TABWIN_WINDOW_TITLE, TabwinClient},
};

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub(in crate::core) fn window_is_tabwin(&mut self, window: &WindowElement, surface: &WlSurface) -> bool {
        self.core.ui_thread_client.is_some()
            && surface.client() == self.core.ui_thread_client
            && window
                .0
                .toplevel()
                .and_then(window_title_for_xdg_toplevel)
                .is_some_and(|title| title == TABWIN_WINDOW_TITLE)
    }

    pub(in crate::core) fn place_tabwin(&mut self, window: &WindowElement, size: Size<i32, Logical>) {
        window.0.override_z_index(RenderZindex::Overlay as u8);

        if let Some(output) = self.output_under_pointer() {
            let workspace = self.core.workspace_manager.active_workspace_mut();
            if let Some(output_geo) = workspace.output_geometry(&output) {
                let window_size = size.to_f64();
                let output_size = output_geo.size.to_f64();
                let new_x = output_geo.loc.x as f64 + (output_size.w - window_size.w) / 2.;
                let new_y = output_geo.loc.y as f64 + (output_size.h - window_size.h) / 2.;
                let new_location = Point::new(new_x as i32, new_y as i32);

                let cur_location = workspace.element_location(window);

                if cur_location.is_none_or(|cur_location| cur_location != new_location) {
                    workspace.map_element(window.clone(), new_location, true);
                }
            }
        }
    }
    pub(in crate::core) fn collect_tabwin_clients(&mut self, output: &Output) -> Vec<TabwinClient> {
        let windows = if self.core.config.cycle_workspaces() {
            self.core
                .workspace_manager
                .workspaces()
                .iter()
                .flat_map(|workspace| workspace.windows().cloned())
                .collect::<Vec<_>>()
        } else {
            self.core
                .workspace_manager
                .active_workspace()
                .windows()
                .cloned()
                .collect::<Vec<_>>()
        };

        windows
            .into_iter()
            .flat_map(|window| {
                let client_data = match window.0.underlying_surface() {
                    WindowSurface::Wayland(toplevel_surface) => {
                        let is_minimized = window
                            .0
                            .user_data()
                            .get::<XdgSurfaceProps>()
                            .map(|props| props.0.lock().unwrap().is_minimized)
                            .unwrap_or(false);
                        if self.core.config.cycle_hidden() || !is_minimized {
                            let app_info = desktop_app_info_for_xdg_toplevel(toplevel_surface);
                            let app_name = app_name_for_xdg_toplevel(toplevel_surface, app_info.as_ref());
                            let title = window_title_for_xdg_toplevel(toplevel_surface);
                            let icon = icon_for_xdg_toplevel(toplevel_surface, output.current_scale().integer_scale(), app_info.as_ref())
                                .and_then(|icon| {
                                    self.window_icon_to_image_data(&icon)
                                        .inspect_err(|err| tracing::info!("Failed to get window icon: {err}"))
                                        .ok()
                                });

                            Some((app_name, title, icon, is_minimized))
                        } else {
                            None
                        }
                    }

                    #[cfg(feature = "xwayland")]
                    WindowSurface::X11(x11_surface) => {
                        if self.core.config.cycle_hidden() || !x11_surface.is_hidden() {
                            use crate::core::util::prettify_name;

                            let app_name = prettify_name(&x11_surface.class());
                            let icon = self.window_icon_for_x11_window(x11_surface);

                            Some((app_name, Some(x11_surface.title()), icon, x11_surface.is_hidden()))
                        } else {
                            None
                        }
                    }
                };

                let id = window.0.wl_surface().map(|surface| surface.id());
                match (id, client_data) {
                    (Some(id), Some((app_name, Some(title), app_icon, is_minimized))) => {
                        let output_scale = match output.current_scale() {
                            output::Scale::Integer(i) => i as f64,
                            output::Scale::Fractional(f) => f,
                            output::Scale::Custom { fractional, .. } => fractional,
                        }
                        .into();
                        let preview_icon = self
                            .window_to_image_data(&window.0, tabwin::WIN_PREVIEW_SIZE as u32, output_scale)
                            .inspect_err(|err| tracing::info!("Failed to get window preview: {err}"))
                            .ok();

                        Some(TabwinClient {
                            id,
                            app_name,
                            title,
                            preview_icon,
                            app_icon,
                            is_minimized,
                        })
                    }
                    _ => None,
                }
            })
            .collect()
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
