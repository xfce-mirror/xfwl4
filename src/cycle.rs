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

use std::{cmp::Ordering, path::PathBuf, str::FromStr};

use anyhow::anyhow;
use glib::CastNone;
use gtk::gio::{
    self,
    traits::{AppInfoExt, FileExt},
};
use smithay::{
    backend::renderer::{BufferType, buffer_type, utils::Buffer},
    desktop::WindowSurface,
    output::{self, Output},
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    utils::{Logical, Point, Size},
    wayland::{compositor::with_states, seat::WaylandFocus, shm, xdg_toplevel_icon::ToplevelIconCachedState},
};

use crate::{
    Xfwl4State,
    backend::Backend,
    shell::{WindowElement, XdgSurfaceIcon, XdgSurfaceProps},
    ui::tabwin::{self, TABWIN_WINDOW_TITLE, TabwinClient},
    util::{ImageData, shm_buffer_to_image_data},
};

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub fn window_is_tabwin(&mut self, window: &WindowElement, surface: &WlSurface) -> bool {
        self.ui_thread_client.is_some()
            && surface.client() == self.ui_thread_client
            && window
                .0
                .toplevel()
                .and_then(|toplevel_surface| self.window_title_xdg(toplevel_surface))
                .is_some_and(|title| title == TABWIN_WINDOW_TITLE)
    }

    pub fn place_tabwin(&mut self, window: &WindowElement, size: Size<i32, Logical>) {
        if let Some(output) = self.output_under_pointer() {
            let workspace = self.workspace_manager.active_workspace_mut();
            if let Some(output_geo) = workspace.output_geometry(&output) {
                let window_size = size.to_f64();
                let output_size = output_geo.size.to_f64();
                let new_x = output_geo.loc.x as f64 + (output_size.w - window_size.w) / 2.;
                let new_y = output_geo.loc.y as f64 + (output_size.h - window_size.h) / 2.;
                let new_location = Point::new(new_x as i32, new_y as i32);

                let cur_location = workspace.element_location(window);

                if cur_location.is_none_or(|cur_location| cur_location != new_location) {
                    tracing::debug!(
                        "placing tabwin at ({new_x}, {new_y}), [output geo: {:?}, window size: ({}, {})]",
                        output_geo,
                        window_size.w,
                        window_size.h
                    );
                    workspace.map_element(window.clone(), new_location, true);
                }
            }
        }
    }
    pub fn collect_tabwin_clients(&mut self, output: &Output) -> Vec<TabwinClient> {
        let elems = if self.config.cycle_workspaces() {
            self.workspace_manager
                .workspaces()
                .iter()
                .flat_map(|workspace| workspace.elements().cloned())
                .collect::<Vec<_>>()
        } else {
            self.workspace_manager.active_workspace().elements().cloned().collect::<Vec<_>>()
        };

        elems
            .into_iter()
            .flat_map(|elem| {
                let client_data = match elem.0.underlying_surface() {
                    WindowSurface::Wayland(toplevel_surface) => elem.0.user_data().get::<XdgSurfaceProps>().and_then(|props| {
                        let inner = props.0.lock().unwrap();

                        if self.config.cycle_hidden() || !inner.is_minimized {
                            let app_info = inner.app_id.as_ref().and_then(|app_id| {
                                let desktop_name = if app_id.ends_with(".desktop") {
                                    app_id
                                } else {
                                    &format!("{app_id}.desktop")
                                };
                                gio::DesktopAppInfo::new(desktop_name)
                            });

                            let app_name = app_info
                                .as_ref()
                                .and_then(|app_info| {
                                    let name = app_info.name().to_string();
                                    (!name.is_empty()).then_some(name)
                                })
                                .or_else(|| inner.app_id.as_ref().and_then(|s| prettify_name(s)));

                            let icon = with_states(toplevel_surface.wl_surface(), |states| {
                                let mut icon_state = states.cached_state.get::<ToplevelIconCachedState>();
                                icon_state
                                    .current()
                                    .icon_name()
                                    .and_then(|name| {
                                        if name.starts_with('/') {
                                            PathBuf::from_str(name).ok().map(XdgSurfaceIcon::File)
                                        } else {
                                            Some(XdgSurfaceIcon::Named(name.to_owned()))
                                        }
                                    })
                                    .or_else(|| {
                                        let buffers_sorted = {
                                            let mut bufs = icon_state.current().buffers().iter().collect::<Vec<_>>();
                                            bufs.sort_by(|first, second| {
                                                let scale_cmp = first.1.cmp(&second.1);
                                                if scale_cmp != Ordering::Equal {
                                                    scale_cmp
                                                } else {
                                                    // xdg-toplevel-icon requires that buffers
                                                    // passed are SHM buffers.
                                                    let first_size =
                                                        shm::with_buffer_contents(&first.0, |_, _, data| data.width.max(data.height))
                                                            .unwrap_or(0);
                                                    let second_size =
                                                        shm::with_buffer_contents(&second.0, |_, _, data| data.width.max(data.height))
                                                            .unwrap_or(0);
                                                    first_size.cmp(&second_size)
                                                }
                                            });
                                            bufs
                                        };

                                        let target_scale = output.current_scale().integer_scale();
                                        buffers_sorted
                                            .iter()
                                            .find(|(_, scale)| *scale == target_scale)
                                            .or_else(|| buffers_sorted.first())
                                            .map(|(buffer, _)| XdgSurfaceIcon::Buffer(Buffer::with_implicit(buffer.clone())))
                                    })
                            })
                            .or_else(|| {
                                app_info.as_ref().and_then(|app_info| {
                                    app_info
                                        .icon()
                                        .and_downcast_ref::<gio::FileIcon>()
                                        .and_then(|icon| icon.file().path().map(XdgSurfaceIcon::File))
                                        .or_else(|| {
                                            app_info
                                                .icon()
                                                .and_downcast_ref::<gio::ThemedIcon>()
                                                .and_then(|icon| icon.names().first().map(|s| XdgSurfaceIcon::Named(s.to_string())))
                                        })
                                })
                            })
                            .and_then(|icon| {
                                self.xdg_surface_icon_to_icon_data(&icon)
                                    .inspect_err(|err| tracing::info!("Failed to get window icon: {err}"))
                                    .ok()
                            });

                            Some((app_name, inner.title.clone(), icon, inner.is_minimized))
                        } else {
                            None
                        }
                    }),

                    #[cfg(feature = "xwayland")]
                    WindowSurface::X11(x11_surface) => {
                        if self.config.cycle_hidden() || !x11_surface.is_hidden() {
                            let app_name = prettify_name(&x11_surface.class());

                            // TODO: check WmHints for icon as well
                            let icon = self
                                .x11conn
                                .as_ref()
                                .and_then(|(x11conn, _)| crate::util::x11_net_wm_icon_to_image_data(x11conn, x11_surface.window_id()).ok());

                            Some((app_name, Some(x11_surface.title()), icon, x11_surface.is_hidden()))
                        } else {
                            None
                        }
                    }
                };

                let id = elem.0.wl_surface().map(|surface| surface.id());
                match (id, client_data) {
                    (Some(id), Some((app_name, Some(title), app_icon, is_minimized))) => {
                        let output_scale = match output.current_scale() {
                            output::Scale::Integer(i) => i as f64,
                            output::Scale::Fractional(f) => f,
                            output::Scale::Custom { fractional, .. } => fractional,
                        }
                        .into();
                        let preview_icon = self
                            .window_to_image_data(&elem.0, tabwin::WIN_PREVIEW_SIZE, output_scale)
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

    fn xdg_surface_icon_to_icon_data(&mut self, xdg_surface_icon: &XdgSurfaceIcon) -> anyhow::Result<ImageData> {
        match xdg_surface_icon {
            XdgSurfaceIcon::Named(icon_name) => Ok(ImageData::NamedIcon(icon_name.clone())),
            XdgSurfaceIcon::File(path) => Ok(ImageData::File(path.clone())),
            XdgSurfaceIcon::Buffer(buffer) => match buffer_type(buffer) {
                Some(BufferType::Shm) => shm_buffer_to_image_data(buffer),
                Some(BufferType::Dma) => self.dmabuf_to_image_data(buffer),
                Some(ty) => Err(anyhow!("unsupported buffer type {ty:?} for icon")),
                None => Err(anyhow!("buffer somehow has no type")),
            },
        }
    }
}

fn prettify_name(name: &str) -> Option<String> {
    if name.is_empty() {
        None
    } else {
        use std::{collections::HashSet, sync::LazyLock};

        static VALID_CHARS: LazyLock<HashSet<char>> = LazyLock::new(|| {
            "[]()0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
                .chars()
                .collect()
        });

        Some(
            name.chars()
                .map(|c| if VALID_CHARS.contains(&c) { c } else { ' ' })
                .collect::<String>()
                .trim()
                .to_owned(),
        )
    }
}
