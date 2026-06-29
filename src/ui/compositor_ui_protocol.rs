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
    collections::HashMap,
    fs,
    io::Read,
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::PathBuf,
    rc::Rc,
};

use anyhow::anyhow;
use glib::clone;
use gtk::{
    gdk::ModifierType,
    traits::{GtkWindowExt, WidgetExt},
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, event_created_child, protocol::wl_registry::WlRegistry};
use xkbcommon::xkb::Keysym;

use crate::{
    core::config::ShortcutKey,
    ui::{
        UiProcessState,
        compositor_ui_protocol::proto::{
            xfwl4_ui_manager_v1::{EVT_CREATE_TABWIN_OPCODE, EVT_CREATE_WINDOW_MENU_OPCODE, Xfwl4UiManagerV1},
            xfwl4_ui_tabwin_v1::{EVT_WINDOW_OPCODE, KeyAction, Xfwl4UiTabwinV1},
            xfwl4_ui_tabwin_window_v1::Xfwl4UiTabwinWindowV1,
            xfwl4_ui_window_menu_v1::{ActionType, Direction, StackingState, Xfwl4UiWindowMenuV1},
        },
        tabwin::{Tabwin, TabwinMode, TabwinWindow, TabwinWindowUpdate},
        wayland_client_gsource::WaylandClientSource,
        window_menu::{self, WindowMenuAction},
    },
    util::icon::{Icon, RgbaPixels},
};

#[derive(Debug)]
pub struct TabwinState {
    instance: Xfwl4UiTabwinV1,
    output_size: (u32, u32),
    output_scale: u32,
    mode: TabwinMode,
    show_window_previews: bool,
    window_opacity: f64,
    key_bindings: HashMap<KeyAction, ShortcutKey>,
    windows: Vec<TabwinWindowState>,
    initial_selection: Option<u32>,
}

#[derive(Debug)]
pub struct TabwinWindowState {
    instance: Xfwl4UiTabwinWindowV1,
    window_id: Option<u32>,
    pending: Option<TabwinPendingProperties>,
}

impl TabwinWindowState {
    fn pending_mut(&mut self) -> &mut TabwinPendingProperties {
        self.pending.get_or_insert_default()
    }
}

#[derive(Debug, Default)]
pub struct TabwinPendingProperties {
    app_name: Option<String>,
    title: Option<String>,
    minimized: bool,
    preview: Option<RgbaPixels>,
    app_icon: Option<Icon>,
}

#[derive(Debug)]
pub struct WindowMenuState {
    _instance: Xfwl4UiWindowMenuV1,
    window_id: Option<u32>,
    maximized: Option<bool>,
    can_minimize: bool,
    can_move: bool,
    can_resize: bool,
    stacking_state: StackingState,
    shaded: Option<bool>,
    fullscreen: Option<bool>,
    sticky: bool,
    current_workspace: Option<u32>,
    workspace_names: Vec<String>,
    adjacent_outputs: Vec<Direction>,
    can_close: bool,
}

impl Dispatch<WlRegistry, ()> for UiProcessState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: <WlRegistry as Proxy>::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<UiProcessState>,
    ) {
        use wayland_client::protocol::wl_registry::Event;

        if let Event::Global { name, interface, version } = event
            && interface == proto::__interfaces::XFWL4_UI_MANAGER_V1_INTERFACE.name
        {
            tracing::debug!("Binding to xfwl4_ui_manager_v1");
            state.ui_manager = Some(registry.bind(name, version, qh, ()));
        }
    }
}

impl Dispatch<Xfwl4UiManagerV1, ()> for UiProcessState {
    fn event(
        state: &mut Self,
        _proxy: &Xfwl4UiManagerV1,
        event: <Xfwl4UiManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use proto::xfwl4_ui_manager_v1::Event;

        match event {
            Event::CreateTabwin { tabwin } => {
                state.tabwin_state = Some(TabwinState {
                    instance: tabwin,
                    output_size: (0, 0),
                    output_scale: 0,
                    mode: TabwinMode::Grid,
                    show_window_previews: true,
                    window_opacity: 1.0,
                    key_bindings: HashMap::default(),
                    windows: Vec::default(),
                    initial_selection: None,
                });
            }
            Event::CreateWindowMenu { menu } => {
                state.window_menu_state = Some(WindowMenuState {
                    _instance: menu,
                    window_id: None,
                    maximized: None,
                    can_minimize: false,
                    can_move: false,
                    can_resize: false,
                    stacking_state: StackingState::Normal,
                    shaded: None,
                    fullscreen: None,
                    sticky: false,
                    current_workspace: None,
                    workspace_names: Vec::new(),
                    adjacent_outputs: Vec::new(),
                    can_close: false,
                });
            }
            Event::Quit => {
                let level = gtk::main_level();
                for _ in 0..level {
                    gtk::main_quit();
                }
            }
        }
    }

    event_created_child!(UiProcessState, Xfwl4UiManagerV1, [
        EVT_CREATE_TABWIN_OPCODE => (Xfwl4UiTabwinV1, ()),
        EVT_CREATE_WINDOW_MENU_OPCODE => (Xfwl4UiWindowMenuV1, ()),
    ]);
}

impl Dispatch<Xfwl4UiTabwinV1, ()> for UiProcessState {
    fn event(
        state: &mut Self,
        proxy: &Xfwl4UiTabwinV1,
        event: <Xfwl4UiTabwinV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use proto::xfwl4_ui_tabwin_v1::Event;

        match event {
            Event::Output { width, height, scale } => {
                if let Some(tabwin) = &mut state.tabwin_state {
                    tabwin.output_size = (width, height);
                    tabwin.output_scale = scale;
                }
            }
            Event::Mode { mode } => {
                if let Some(tabwin) = &mut state.tabwin_state
                    && let WEnum::Value(mode) = mode
                {
                    tabwin.mode = mode.into();
                }
            }
            Event::ShowWindowPreviews { show_previews } => {
                if let Some(tabwin) = &mut state.tabwin_state {
                    tabwin.show_window_previews = show_previews != 0;
                }
            }
            Event::WindowOpacity { opacity } => {
                if let Some(tabwin) = &mut state.tabwin_state {
                    tabwin.window_opacity = opacity;
                }
            }
            Event::KeyBinding { action, keysym, modifiers } => {
                if let Some(tabwin) = &mut state.tabwin_state
                    && let WEnum::Value(action) = action
                {
                    tabwin.key_bindings.insert(
                        action,
                        ShortcutKey::new(Keysym::from(keysym), ModifierType::from_bits_truncate(modifiers)),
                    );
                }
            }
            Event::InitialSelection { window_id } => {
                if let Some(tabwin) = &mut state.tabwin_state {
                    tabwin.initial_selection = Some(window_id);
                }
            }
            Event::Window { window } => {
                tracing::debug!("got tabwin window event");
                if let Some(tabwin) = &mut state.tabwin_state {
                    tracing::debug!("adding tabwin window");
                    tabwin.windows.push(TabwinWindowState {
                        instance: window,
                        window_id: None,
                        pending: None,
                    });
                }
            }
            Event::Done => {
                if let Some(tabwin_state) = &mut state.tabwin_state {
                    if let Some(tabwin) = state.tabwin.as_ref() {
                        tracing::debug!("got updated tabwin done message, have {} windows", tabwin_state.windows.len());

                        // TODO: handle new windows, not just updated ones
                        let window_updates = tabwin_state
                            .windows
                            .iter_mut()
                            .flat_map(|window| {
                                if let Some(window_id) = window.window_id
                                    && let Some(pending) = window.pending.take()
                                    && (pending.preview.is_some() || pending.app_icon.is_some())
                                {
                                    // TODO: handle other property updates, even though i probably
                                    // won't use this
                                    Some((
                                        window_id,
                                        TabwinWindowUpdate {
                                            preview_icon: pending.preview,
                                            app_icon: pending.app_icon,
                                        },
                                    ))
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>();
                        tracing::debug!("after filtering, have {} windows", window_updates.len());

                        for (window_id, update) in window_updates {
                            tabwin.update_client(window_id, update);
                        }
                    } else {
                        tracing::debug!("got initial tabwin done message, have {} windows", tabwin_state.windows.len());

                        let windows = tabwin_state
                            .windows
                            .iter_mut()
                            .flat_map(|window| {
                                if let Some(window_id) = window.window_id
                                    && let Some(pending) = window.pending.take()
                                    && let Some(title) = pending.title
                                {
                                    Some(TabwinWindow {
                                        id: window_id,
                                        app_name: pending.app_name,
                                        title,
                                        preview_icon: pending.preview,
                                        app_icon: pending.app_icon,
                                        is_minimized: pending.minimized,
                                    })
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>();
                        tracing::debug!("after filtering, have {} windows", windows.len());

                        if let Some(fallback_selected_id) = windows.get(1).or_else(|| windows.first()).map(|window| window.id) {
                            let initial_selection = tabwin_state.initial_selection.unwrap_or(fallback_selected_id);
                            let tabwin = Tabwin::new(
                                (tabwin_state.output_size.0 as i32, tabwin_state.output_size.1 as i32),
                                tabwin_state.output_scale as i32,
                                tabwin_state.mode,
                                tabwin_state.show_window_previews,
                                tabwin_state.window_opacity,
                                state.tabwin_style_provider.as_ref(),
                                tabwin_state
                                    .key_bindings
                                    .get(&KeyAction::Next)
                                    .cloned()
                                    .unwrap_or(ShortcutKey::DEFAULT_CYCLE_WINDOWS),
                                tabwin_state
                                    .key_bindings
                                    .get(&KeyAction::Prev)
                                    .cloned()
                                    .unwrap_or(ShortcutKey::DEFAULT_CYCLE_REVERSE_WINDOWS),
                                tabwin_state
                                    .key_bindings
                                    .get(&KeyAction::Up)
                                    .cloned()
                                    .unwrap_or(ShortcutKey::DEFAULT_UP),
                                tabwin_state
                                    .key_bindings
                                    .get(&KeyAction::Down)
                                    .cloned()
                                    .unwrap_or(ShortcutKey::DEFAULT_DOWN),
                                tabwin_state
                                    .key_bindings
                                    .get(&KeyAction::Left)
                                    .cloned()
                                    .unwrap_or(ShortcutKey::DEFAULT_LEFT),
                                tabwin_state
                                    .key_bindings
                                    .get(&KeyAction::Right)
                                    .cloned()
                                    .unwrap_or(ShortcutKey::DEFAULT_RIGHT),
                                tabwin_state
                                    .key_bindings
                                    .get(&KeyAction::Cancel)
                                    .cloned()
                                    .unwrap_or(ShortcutKey::DEFAULT_CANCEL),
                            );

                            tabwin.connect_image_sizes(clone!(@strong proxy => move |_, preview_size, icon_size| {
                                if proxy.is_alive() {
                                    proxy.image_sizes(preview_size, icon_size);
                                }
                            }));
                            tabwin.connect_hover_window(clone!(@strong proxy => move |_, selected| {
                                if proxy.is_alive() {
                                    proxy.hover(selected);
                                }
                            }));
                            tabwin.connect_activated(clone!(@strong proxy => move |_, selected| {
                                if proxy.is_alive() {
                                    proxy.finished(selected);
                                }
                            }));
                            tabwin.connect_cancelled(clone!(@strong proxy => move |_| {
                                if proxy.is_alive() {
                                    proxy.dismissed();
                                }
                            }));

                            tabwin.init_clients(windows, initial_selection);

                            tracing::debug!("showing tabwin");
                            tabwin.show_all();

                            state.tabwin = Some(tabwin);
                        } else {
                            tracing::debug!("couldn't get initial selection");
                            proxy.dismissed();
                        }
                    }
                }
            }
            Event::Close => {
                if let Some(tabwin_state) = state.tabwin_state.take() {
                    tabwin_state.instance.destroy();
                }
                if let Some(tabwin) = state.tabwin.take() {
                    tabwin.close();
                }
            }
        }
    }

    event_created_child!(UiProcessState, Xfwl4UiTabwinV1, [
        EVT_WINDOW_OPCODE => (Xfwl4UiTabwinWindowV1, ()),
    ]);
}

impl Dispatch<Xfwl4UiTabwinWindowV1, ()> for UiProcessState {
    fn event(
        state: &mut Self,
        proxy: &Xfwl4UiTabwinWindowV1,
        event: <Xfwl4UiTabwinWindowV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use proto::xfwl4_ui_tabwin_window_v1::Event;

        match event {
            Event::WindowId { id } => {
                if let Some(tabwin) = &mut state.tabwin_state
                    && let Some(window) = tabwin.windows.iter_mut().find(|window| window.instance == *proxy)
                {
                    window.window_id = Some(id);
                }
            }
            Event::AppName { name } => {
                if let Some(tabwin) = &mut state.tabwin_state
                    && let Some(window) = tabwin.windows.iter_mut().find(|window| window.instance == *proxy)
                {
                    window.pending_mut().app_name = Some(name);
                }
            }
            Event::Title { title } => {
                if let Some(tabwin) = &mut state.tabwin_state
                    && let Some(window) = tabwin.windows.iter_mut().find(|window| window.instance == *proxy)
                {
                    window.pending_mut().title = Some(title);
                }
            }
            Event::Minimized => {
                if let Some(tabwin) = &mut state.tabwin_state
                    && let Some(window) = tabwin.windows.iter_mut().find(|window| window.instance == *proxy)
                {
                    window.pending_mut().minimized = true;
                }
            }
            Event::Preview { fd, width, height, scale } => {
                if let Some(tabwin) = &mut state.tabwin_state
                    && let Some(window) = tabwin.windows.iter_mut().find(|window| window.instance == *proxy)
                    && let Some(pixels) = read_image_fd(fd, width, height, scale)
                {
                    window.pending_mut().preview = Some(pixels);
                }
            }
            Event::AppIconNamed { name } => {
                if let Some(tabwin) = &mut state.tabwin_state
                    && let Some(window) = tabwin.windows.iter_mut().find(|window| window.instance == *proxy)
                {
                    window.pending_mut().app_icon = Some(Icon::Named(name));
                }
            }
            Event::AppIconFile { path } => {
                if let Some(tabwin) = &mut state.tabwin_state
                    && let Some(window) = tabwin.windows.iter_mut().find(|window| window.instance == *proxy)
                {
                    window.pending_mut().app_icon = Some(Icon::File(PathBuf::from(path)));
                }
            }
            Event::AppIconPixels { fd, width, height, scale } => {
                if let Some(tabwin) = &mut state.tabwin_state
                    && let Some(window) = tabwin.windows.iter_mut().find(|window| window.instance == *proxy)
                    && let Some(pixels) = read_image_fd(fd, width, height, scale)
                {
                    window.pending_mut().app_icon = Some(Icon::Pixels(pixels));
                }
            }
            Event::Done => (),
            Event::Removed => {
                if let Some(tabwin_state) = &mut state.tabwin_state
                    && let Some(pos) = tabwin_state.windows.iter().position(|window| window.instance == *proxy)
                {
                    let window = tabwin_state.windows.remove(pos);

                    if let Some(tabwin) = &state.tabwin
                        && let Some(window_id) = window.window_id
                    {
                        tabwin.remove_client(window_id);
                    }
                }
            }
        }
    }
}

impl Dispatch<Xfwl4UiWindowMenuV1, ()> for UiProcessState {
    fn event(
        state: &mut Self,
        proxy: &Xfwl4UiWindowMenuV1,
        event: <Xfwl4UiWindowMenuV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use proto::xfwl4_ui_window_menu_v1::Event;

        match event {
            Event::WindowId { window_id } => {
                if let Some(window) = &mut state.window_menu_state {
                    window.window_id = Some(window_id);
                }
            }
            Event::IsMaximized { maximized } => {
                if let Some(window) = &mut state.window_menu_state {
                    window.maximized = Some(maximized != 0);
                }
            }
            Event::CanMinimize => {
                if let Some(window) = &mut state.window_menu_state {
                    window.can_minimize = true;
                }
            }
            Event::CanMove => {
                if let Some(window) = &mut state.window_menu_state {
                    window.can_move = true;
                }
            }
            Event::CanResize => {
                if let Some(window) = &mut state.window_menu_state {
                    window.can_resize = true;
                }
            }
            Event::StackingState { state: stacking_state } => {
                if let Some(window) = &mut state.window_menu_state
                    && let WEnum::Value(stacking_state) = stacking_state
                {
                    window.stacking_state = stacking_state;
                }
            }
            Event::IsShaded { shaded } => {
                if let Some(window) = &mut state.window_menu_state {
                    window.shaded = Some(shaded != 0);
                }
            }
            Event::IsFullscreen { fullscreen } => {
                if let Some(window) = &mut state.window_menu_state {
                    window.fullscreen = Some(fullscreen != 0);
                }
            }
            Event::Sticky => {
                if let Some(window) = &mut state.window_menu_state {
                    window.sticky = true;
                }
            }
            Event::CanClose => {
                if let Some(window) = &mut state.window_menu_state {
                    window.can_close = true;
                }
            }
            Event::Workspace { name, is_current } => {
                if let Some(window) = &mut state.window_menu_state {
                    if is_current != 0 {
                        let cur_num = window.workspace_names.len() as u32;
                        window.current_workspace = Some(cur_num);
                    }
                    window.workspace_names.push(name);
                }
            }
            Event::AdjacentMonitor { direction } => {
                if let Some(window) = &mut state.window_menu_state
                    && let WEnum::Value(direction) = direction
                {
                    window.adjacent_outputs.push(direction);
                }
            }
            Event::Done => {
                if let Some(window) = state.window_menu_state.take() {
                    let window_menu = window_menu::create_menu(
                        window.maximized,
                        window.can_minimize,
                        window.can_move,
                        window.can_resize,
                        window.stacking_state,
                        window.shaded,
                        window.fullscreen,
                        window.sticky,
                        window.current_workspace,
                        window.workspace_names,
                        window.adjacent_outputs,
                        window.can_close,
                        &state.window_menu_anchor,
                        clone!(@strong proxy => move |action| {
                            match action {
                                WindowMenuAction::ToggleMaximize => proxy.action(ActionType::ToggleMaximize),
                                WindowMenuAction::Minimize => proxy.action(ActionType::Minimize),
                                WindowMenuAction::MinimizeOtherWindows => proxy.action(ActionType::MinimizeOtherWindows),
                                WindowMenuAction::Move => proxy.action(ActionType::Move),
                                WindowMenuAction::Resize => proxy.action(ActionType::Resize),
                                WindowMenuAction::StackOnTop => proxy.action(ActionType::StackOnTop),
                                WindowMenuAction::StackNormal => proxy.action(ActionType::StackNormal),
                                WindowMenuAction::StackBelow => proxy.action(ActionType::StackBelow),
                                WindowMenuAction::ToggleShade => proxy.action(ActionType::ToggleShade),
                                WindowMenuAction::Fullscreen => proxy.action(ActionType::ToggleFullscreen),
                                WindowMenuAction::ToggleSticky => proxy.action(ActionType::ToggleSticky),
                                WindowMenuAction::Close => proxy.action(ActionType::Close),
                                WindowMenuAction::MoveToWorkspace(idx) => proxy.move_to_workspace(idx),
                                WindowMenuAction::MoveToOutput(direction) => proxy.move_to_output(direction),
                            }
                        }),
                        clone!(@strong proxy => move || proxy.dismissed()),
                    );
                    proxy.ready();
                    state.window_menu = Some(window_menu);
                }
            }
        }
    }
}

pub fn connect(socket_name: &str, mut state: UiProcessState) -> anyhow::Result<Rc<RefCell<UiProcessState>>> {
    // Annoying I have to duplicate this from wayland-client, as it doesn't have an API that takes
    // a socket name/path.

    let socket_name = PathBuf::from(socket_name);
    let socket_path = if socket_name.is_absolute() {
        Ok(socket_name)
    } else {
        let mut socket_path = std::env::var_os("XDG_RUNTIME_DIR")
            .map(Into::<PathBuf>::into)
            .ok_or_else(|| anyhow!("Can't get XDG_RUNTIME_DIR"))?;
        if !socket_path.is_absolute() {
            Err(anyhow!("Can't make absolute socket path"))
        } else {
            socket_path.push(socket_name);
            Ok(socket_path)
        }
    }?;

    let stream = UnixStream::connect(socket_path)?;
    let conn = Connection::from_socket(stream)?;
    let display = conn.display();
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let registry = display.get_registry(&qh, ());
    state.registry = Some(registry);

    event_queue.roundtrip(&mut state)?;

    let state = Rc::new(RefCell::new(state));
    let source = WaylandClientSource::attach(conn, Rc::new(RefCell::new(event_queue)), Rc::clone(&state), None);
    state.borrow_mut().source = Some(source);

    Ok(state)
}

fn read_image_fd(fd: OwnedFd, width: u32, height: u32, scale: u32) -> Option<RgbaPixels> {
    let mut f = fs::File::from(fd);
    let size = width * height * 4;
    let mut bytes = vec![0; size as usize];
    f.read_exact(&mut bytes).ok()?;
    Some(RgbaPixels {
        bytes,
        size: (width, height).into(),
        scale,
    })
}

pub mod proto {
    use wayland_client;

    pub mod __interfaces {
        use wayland_client::backend as wayland_backend;

        wayland_scanner::generate_interfaces!("./resources/xfwl4-compositor-ui-private-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("./resources/xfwl4-compositor-ui-private-v1.xml");
}
