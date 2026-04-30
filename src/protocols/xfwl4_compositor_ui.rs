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
    collections::{HashMap, HashSet},
    os::fd::AsFd,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::anyhow;
use gtk::gdk;
use smithay::{
    reexports::{
        rustix::process::Pid,
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
            backend::{ClientId, GlobalId},
        },
    },
    utils::SealedFile,
    wayland::{Dispatch2, GlobalDispatch2},
};
use xkbcommon::xkb::Keysym;

use crate::protocols::{
    GlobalData,
    xfwl4_compositor_ui::proto::{
        xfwl4_ui_manager_v1::Xfwl4UiManagerV1,
        xfwl4_ui_tabwin_v1::{KeyAction, TabwinMode, Xfwl4UiTabwinV1},
        xfwl4_ui_tabwin_window_v1::Xfwl4UiTabwinWindowV1,
        xfwl4_ui_window_menu_v1::{ActionType, Direction, StackingState, Xfwl4UiWindowMenuV1},
    },
};

const PROTO_VERSION: u32 = proto::__interfaces::XFWL4_UI_MANAGER_V1_INTERFACE.version;

pub struct CompositorUiState {
    dh: DisplayHandle,
    _global: GlobalId,
    client_pid: Arc<Mutex<Option<Pid>>>,
    manager_instance: Option<Xfwl4UiManagerV1>,
    icon_size_hints: IconSizeHints,
    accumulated_theme_colors: HashMap<String, gtk::gdk::RGBA>,
    tabwin: Option<Tabwin>,
    window_menu: Option<WindowMenu>,

    shutting_down: bool,
}

pub struct CompositorUiManagerData {
    dh: DisplayHandle,
    client_pid: Arc<Mutex<Option<Pid>>>,
}

#[derive(Debug, PartialEq)]
pub struct IconSizeHints {
    pub tabwin_mode: TabwinMode,
    pub tabwin_show_window_previews: bool,
}

struct Tabwin {
    instance: Xfwl4UiTabwinV1,
    show_window_previews: bool,
    windows: Vec<(u32, Xfwl4UiTabwinWindowV1)>,
}

#[derive(Debug)]
pub struct Pixels {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug)]
pub enum Icon {
    Named(String),
    File(PathBuf),
    Pixels(Pixels),
}

#[derive(Debug)]
pub struct TabwinWindow {
    pub window_id: u32,
    pub app_name: Option<String>,
    pub title: String,
    pub preview: Option<Pixels>,
    pub app_icon: Option<Icon>,
    pub is_minimized: bool,
}

#[derive(Debug)]
pub struct TabwinConfig {
    pub mode: TabwinMode,
    pub show_window_previews: bool,
    pub window_opacity: f64,
    pub windows: Vec<TabwinWindow>,
    pub initial_selection: u32,
    pub next_shortcut: Option<(Keysym, gdk::ModifierType)>,
    pub prev_shortcut: Option<(Keysym, gdk::ModifierType)>,
    pub up_shortcut: Option<(Keysym, gdk::ModifierType)>,
    pub down_shortcut: Option<(Keysym, gdk::ModifierType)>,
    pub left_shortcut: Option<(Keysym, gdk::ModifierType)>,
    pub right_shortcut: Option<(Keysym, gdk::ModifierType)>,
    pub cancel_shortcut: Option<(Keysym, gdk::ModifierType)>,
}

struct WindowMenu {
    instance: Xfwl4UiWindowMenuV1,
    window_id: u32,
}

#[derive(Debug, Clone)]
pub struct WindowMenuState {
    pub window_id: u32,
    pub maximize_state: Option<bool>,
    pub can_minimize: bool,
    pub can_move: bool,
    pub can_resize: bool,
    pub stacking_state: StackingState,
    pub shaded_state: Option<bool>,
    pub fullscreen_state: Option<bool>,
    pub sticky: bool,
    pub workspace_names: Vec<String>,
    pub current_workspace: u32,
    pub adjacent_outputs: Vec<Direction>,
    pub can_close: bool,
}

pub enum WindowMenuAction {
    Action(ActionType),
    MoveToWorkspace(u32),
    MoveToOutput(Direction),
}

pub trait CompositorUiHandler: 'static {
    fn compositor_ui_state(&mut self) -> &mut CompositorUiState;

    fn icon_sizes(&mut self, icon_sizes: HashSet<i32>);
    fn theme_colors(&mut self, theme_colors: HashMap<String, gtk::gdk::RGBA>);

    fn tabwin_hover(&mut self, hover_window_id: u32);
    fn tabwin_finished(&mut self, selected_window_id: Option<u32>);

    fn window_menu_ready(&mut self);
    fn window_menu_action(&mut self, window_id: u32, action: WindowMenuAction);
    fn window_menu_dismissed(&mut self);
}

impl CompositorUiState {
    pub fn new<H>(dh: &DisplayHandle, icon_size_hints: IconSizeHints) -> Self
    where
        H: CompositorUiHandler + GlobalDispatch<Xfwl4UiManagerV1, CompositorUiManagerData>,
    {
        let client_pid = Arc::new(Mutex::new(None));
        let data = CompositorUiManagerData {
            dh: dh.clone(),
            client_pid: Arc::clone(&client_pid),
        };
        let global = dh.create_global::<H, Xfwl4UiManagerV1, _>(PROTO_VERSION, data);
        Self {
            dh: dh.clone(),
            _global: global,
            client_pid,
            manager_instance: None,
            icon_size_hints,
            accumulated_theme_colors: HashMap::new(),
            tabwin: None,
            window_menu: None,
            shutting_down: false,
        }
    }

    pub fn client_pid(&self) -> Option<Pid> {
        *self.client_pid.lock().unwrap()
    }

    pub fn set_ui_client_pid(&mut self, client_pid: Option<Pid>) {
        *self.client_pid.lock().unwrap() = client_pid;
        self.manager_instance = None;
        self.accumulated_theme_colors.clear();
        self.tabwin = None;
        self.window_menu = None;
    }

    pub fn set_icon_size_hints(&mut self, hints: IconSizeHints) {
        self.icon_size_hints = hints;
        if let Some(manager) = &self.manager_instance {
            manager.provide_icon_sizes(
                self.icon_size_hints.tabwin_mode,
                self.icon_size_hints.tabwin_show_window_previews.into(),
            );
        }
    }

    pub fn create_tabwin<H>(&mut self, config: TabwinConfig) -> anyhow::Result<()>
    where
        H: CompositorUiHandler + Dispatch<Xfwl4UiTabwinV1, GlobalData> + Dispatch<Xfwl4UiTabwinWindowV1, GlobalData>,
    {
        if self.tabwin.is_none() {
            if let Some(manager) = &self.manager_instance
                && let Some(client) = manager.client()
            {
                let tabwin_instance = client.create_resource::<Xfwl4UiTabwinV1, _, H>(&self.dh, manager.version(), GlobalData)?;
                manager.create_tabwin(&tabwin_instance);

                tabwin_instance.mode(config.mode);
                tabwin_instance.window_opacity(config.window_opacity);
                tabwin_instance.show_window_previews(if config.show_window_previews { 1 } else { 0 });

                for (action, shortcut) in [
                    (KeyAction::Next, config.next_shortcut),
                    (KeyAction::Prev, config.prev_shortcut),
                    (KeyAction::Up, config.up_shortcut),
                    (KeyAction::Down, config.down_shortcut),
                    (KeyAction::Left, config.left_shortcut),
                    (KeyAction::Right, config.right_shortcut),
                    (KeyAction::Cancel, config.cancel_shortcut),
                ] {
                    if let Some((keysym, mask)) = shortcut {
                        tabwin_instance.key_binding(action, keysym.into(), mask.bits());
                    }
                }

                tracing::debug!("about to send {} windows", config.windows.len());
                let windows = config
                    .windows
                    .into_iter()
                    .map(|window| send_window::<H>(&self.dh, &tabwin_instance, &client, window, config.show_window_previews))
                    .collect::<Result<Vec<_>, _>>()?;
                tracing::debug!("finished sending {} windows", windows.len());

                tabwin_instance.initial_selection(config.initial_selection);

                tabwin_instance.done();
                self.tabwin = Some(Tabwin {
                    instance: tabwin_instance,
                    show_window_previews: config.show_window_previews,
                    windows,
                });

                Ok(())
            } else {
                Err(anyhow!("No UI process bound or client gone"))
            }
        } else {
            Err(anyhow!("Attempt to raise tabwin while it's already up"))
        }
    }

    pub fn tabwin_add_window<H>(&mut self, window: TabwinWindow) -> anyhow::Result<()>
    where
        H: CompositorUiHandler + Dispatch<Xfwl4UiTabwinWindowV1, GlobalData>,
    {
        if let Some(tabwin) = &mut self.tabwin {
            if let Some(client) = tabwin.instance.client() {
                tabwin.windows.push(send_window::<H>(
                    &self.dh,
                    &tabwin.instance,
                    &client,
                    window,
                    tabwin.show_window_previews,
                )?);
                Ok(())
            } else {
                self.tabwin = None;
                Err(anyhow!("Attempt to add window to tabwin, but the UI process client is gone"))
            }
        } else {
            Err(anyhow!("Attempt to add window to tabwin when it's not up"))
        }
    }

    pub fn tabwin_remove_window(&mut self, window_id: u32) {
        if let Some(tabwin) = &mut self.tabwin {
            tabwin.instance.window_removed(window_id);
        }
    }

    pub fn tabwin_closed(&mut self) {
        self.tabwin = None;
    }

    pub fn create_window_menu<H>(&mut self, state: WindowMenuState) -> anyhow::Result<()>
    where
        H: CompositorUiHandler + Dispatch<Xfwl4UiWindowMenuV1, GlobalData>,
    {
        if self.window_menu.is_none() {
            if let Some(manager) = &self.manager_instance
                && let Some(client) = manager.client()
            {
                let window_menu_instance = client.create_resource::<Xfwl4UiWindowMenuV1, _, H>(&self.dh, manager.version(), GlobalData)?;
                manager.create_window_menu(&window_menu_instance);

                window_menu_instance.window_id(state.window_id);

                if let Some(maximized) = state.maximize_state {
                    window_menu_instance.is_maximized(maximized.into());
                }

                if state.can_minimize {
                    window_menu_instance.can_minimize();
                }

                if state.can_move {
                    window_menu_instance.can_move();
                }

                if state.can_resize {
                    window_menu_instance.can_resize();
                }

                window_menu_instance.stacking_state(state.stacking_state);

                if let Some(shaded) = state.shaded_state {
                    window_menu_instance.is_shaded(shaded.into());
                }

                if let Some(fullscreen) = state.fullscreen_state {
                    window_menu_instance.is_fullscreen(fullscreen.into());
                }

                if state.sticky {
                    window_menu_instance.sticky();
                }

                for (i, name) in state.workspace_names.into_iter().enumerate() {
                    window_menu_instance.workspace(name, (i == (state.current_workspace as usize)).into());
                }

                for direction in state.adjacent_outputs {
                    window_menu_instance.adjacent_monitor(direction);
                }

                if state.can_close {
                    window_menu_instance.can_close();
                }

                window_menu_instance.done();
                self.window_menu = Some(WindowMenu {
                    instance: window_menu_instance,
                    window_id: state.window_id,
                });

                Ok(())
            } else {
                Err(anyhow!("No UI process bound or client gone"))
            }
        } else {
            Err(anyhow!("Attempt to create the window menu when it's already up"))
        }
    }

    pub fn send_quit(&mut self) {
        self.shutting_down = true;
        if let Some(manager) = &self.manager_instance {
            manager.quit();
        }
    }
}

impl<D: CompositorUiHandler> GlobalDispatch2<Xfwl4UiManagerV1, D> for CompositorUiManagerData
where
    D: Dispatch<Xfwl4UiManagerV1, GlobalData>,
{
    fn bind(
        &self,
        state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<Xfwl4UiManagerV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        let instance = data_init.init(resource, GlobalData);
        let state = state.compositor_ui_state();
        if state.manager_instance.is_none() {
            tracing::debug!("Got UI client binding");
            instance.provide_icon_sizes(
                state.icon_size_hints.tabwin_mode,
                state.icon_size_hints.tabwin_show_window_previews.into(),
            );
            state.manager_instance = Some(instance);
        } else {
            tracing::warn!("Got a bind attempt, but we already have a manager bound");
        }
    }

    fn can_view(&self, client: &Client) -> bool {
        if let Some(client_pid) = self.client_pid.lock().unwrap().as_ref() {
            match client.get_credentials(&self.dh) {
                Err(err) => {
                    tracing::info!("Unable to authenticate possible UI thread client: {err}");
                    false
                }
                Ok(creds) => creds.pid == client_pid.as_raw_pid(),
            }
        } else {
            false
        }
    }
}

impl<D: CompositorUiHandler> Dispatch2<Xfwl4UiManagerV1, D> for GlobalData {
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        _resource: &Xfwl4UiManagerV1,
        request: <Xfwl4UiManagerV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        use proto::xfwl4_ui_manager_v1::Request;

        match request {
            Request::IconSizes { sizes } => {
                let icon_sizes = sizes
                    .chunks_exact(4)
                    .flat_map(|chunk| <[u8; 4]>::try_from(chunk).map(i32::from_ne_bytes))
                    .collect();
                state.icon_sizes(icon_sizes);
            }
            Request::ThemeColor {
                name,
                red,
                green,
                blue,
                alpha,
            } => {
                tracing::debug!("got a theme color named {name} ({red}, {green}, {blue}, {alpha})");
                state
                    .compositor_ui_state()
                    .accumulated_theme_colors
                    .insert(name, gdk::RGBA::new(red, green, blue, alpha));
            }
            Request::ThemeColorsDone => {
                let theme_colors = std::mem::take(&mut state.compositor_ui_state().accumulated_theme_colors);
                tracing::debug!("got done, applying {} theme colors", theme_colors.len());
                state.theme_colors(theme_colors);
            }
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &Xfwl4UiManagerV1) {
        let state = state.compositor_ui_state();
        if state.manager_instance.as_ref() == Some(resource) {
            state.manager_instance = None;
        }
        if !state.shutting_down {
            tracing::warn!("UI client has unexpectedly disconnected");
        }
    }
}

impl<D: CompositorUiHandler> Dispatch2<Xfwl4UiTabwinV1, D> for GlobalData {
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        _resource: &Xfwl4UiTabwinV1,
        request: <Xfwl4UiTabwinV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        use proto::xfwl4_ui_tabwin_v1::Request;

        match request {
            Request::Hover { window_id } => state.tabwin_hover(window_id),
            Request::Finished { selected_window_id } => {
                state.compositor_ui_state().tabwin = None;
                state.tabwin_finished(Some(selected_window_id));
            }
            Request::Dismissed => {
                state.compositor_ui_state().tabwin = None;
                state.tabwin_finished(None);
            }
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &Xfwl4UiTabwinV1) {
        let handler = state;
        let state = handler.compositor_ui_state();

        if let Some(tabwin) = &state.tabwin
            && tabwin.instance == *resource
        {
            tracing::warn!("Got tabwin destroyed without a finished request");
            state.tabwin = None;
            handler.tabwin_finished(None);
        }
    }
}

impl<D: CompositorUiHandler> Dispatch2<Xfwl4UiTabwinWindowV1, D> for GlobalData {
    fn request(
        &self,
        _state: &mut D,
        _client: &Client,
        _resource: &Xfwl4UiTabwinWindowV1,
        _request: <Xfwl4UiTabwinWindowV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &Xfwl4UiTabwinWindowV1) {
        if let Some(tabwin) = &mut state.compositor_ui_state().tabwin {
            tabwin.windows.retain(|(_, instance)| instance != resource);
        }
    }
}

impl<D: CompositorUiHandler> Dispatch2<Xfwl4UiWindowMenuV1, D> for GlobalData {
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        resource: &Xfwl4UiWindowMenuV1,
        request: <Xfwl4UiWindowMenuV1 as Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        use proto::xfwl4_ui_window_menu_v1::Request;

        if let Some(window_id) = state
            .compositor_ui_state()
            .window_menu
            .as_ref()
            .filter(|menu| menu.instance == *resource)
            .map(|menu| menu.window_id)
        {
            match request {
                Request::Ready => state.window_menu_ready(),
                Request::Action { action } => match action {
                    WEnum::Value(action) => state.window_menu_action(window_id, WindowMenuAction::Action(action)),
                    WEnum::Unknown(v) => tracing::warn!("Got unknown enum value for window menu action: {v}"),
                },
                Request::MoveToWorkspace { workspace_index } => {
                    state.window_menu_action(window_id, WindowMenuAction::MoveToWorkspace(workspace_index))
                }
                Request::MoveToOutput { direction } => match direction {
                    WEnum::Value(direction) => state.window_menu_action(window_id, WindowMenuAction::MoveToOutput(direction)),
                    WEnum::Unknown(v) => tracing::warn!("Got unknown enum value for window menu move to output direction: {v}"),
                },
                Request::Dismissed => {
                    state.compositor_ui_state().window_menu = None;
                    state.window_menu_dismissed();
                }
            }
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &Xfwl4UiWindowMenuV1) {
        let handler = state;
        let state = handler.compositor_ui_state();

        if let Some(window_menu) = &state.window_menu
            && window_menu.instance == *resource
        {
            tracing::warn!("Got window menu destroyed without a finished request");
            state.window_menu = None;
            handler.window_menu_dismissed();
        }
    }
}

fn send_window<H>(
    dh: &DisplayHandle,
    tabwin_instance: &Xfwl4UiTabwinV1,
    client: &Client,
    window: TabwinWindow,
    show_window_previews: bool,
) -> anyhow::Result<(u32, Xfwl4UiTabwinWindowV1)>
where
    H: CompositorUiHandler + Dispatch<Xfwl4UiTabwinWindowV1, GlobalData>,
{
    let window_instance = client
        .create_resource::<Xfwl4UiTabwinWindowV1, _, H>(dh, tabwin_instance.version(), GlobalData)
        .map_err(|err| anyhow!("Failed to create tabwin window: {err}"))?;
    tabwin_instance.window(&window_instance);

    tracing::debug!("sending window {}, '{}'", window.window_id, window.title);

    window_instance.window_id(window.window_id);
    if let Some(app_name) = window.app_name {
        window_instance.app_name(app_name);
    }
    window_instance.title(window.title);
    if window.is_minimized {
        window_instance.minimized();
    }

    if show_window_previews && let Some(preview) = window.preview {
        match SealedFile::with_data(c"preview", &preview.bytes) {
            Err(err) => tracing::warn!("Failed to create FD for tabwin preview image (continuing anyway): {err}"),
            Ok(fd) => window_instance.preview(fd.as_fd(), preview.width, preview.height),
        }
    }

    if let Some(app_icon) = window.app_icon {
        match app_icon {
            Icon::Named(name) => window_instance.app_icon_named(name),
            Icon::File(path) => {
                if let Some(path_str) = path.to_str() {
                    window_instance.app_icon_file(path_str.to_owned());
                } else {
                    tracing::warn!(
                        "Failed to convert path for app icon '{}' into a string (continuing anyway)",
                        path.display()
                    );
                }
            }
            Icon::Pixels(pixels) => match SealedFile::with_data(c"app_icon", &pixels.bytes) {
                Err(err) => tracing::warn!("Failed to create FD for tabwin app icon (continuing anyway): {err}"),
                Ok(fd) => window_instance.app_icon_pixels(fd.as_fd(), pixels.width, pixels.height),
            },
        }
    }

    window_instance.done();

    Ok((window.window_id, window_instance))
}

pub mod proto {
    use smithay::reexports::wayland_server;

    pub mod __interfaces {
        use smithay::reexports::wayland_server::backend as wayland_backend;

        wayland_scanner::generate_interfaces!("./resources/xfwl4-compositor-ui-private-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("./resources/xfwl4-compositor-ui-private-v1.xml");
}
