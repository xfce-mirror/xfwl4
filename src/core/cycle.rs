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

use smithay::{
    desktop::{WindowSurface, space::SpaceElement},
    output::Output,
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    utils::{Logical, Point, Rectangle, Size},
};
use xkbcommon::xkb::{Keycode, Keysym};

use crate::{
    backend::Backend,
    core::{
        config::ActivateAction,
        drawing::wireframe::Wireframe,
        shell::{
            WindowElement, WindowFlags, WorkspaceLocation,
            xdg::{app_name_for_xdg_toplevel, desktop_app_info_for_xdg_toplevel, window_title_for_xdg_toplevel},
        },
        state::Xfwl4State,
        util::OutputExt,
        workspaces::WindowStackingLayer,
    },
    protocols::xfwl4_compositor_ui::{TabwinConfig, TabwinWindow},
    ui::tabwin::TABWIN_WINDOW_TITLE,
    util::icon::{Argb32Pixels, Icon},
};

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum CyclingPhase {
    #[default]
    None,
    Active,
    Finishing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::core) enum TabwinGrab {
    Keyboard,
    Pointer,
    Touch,
}

#[derive(Debug, Default)]
pub(in crate::core) struct CyclingState {
    cycle_list: Vec<WindowElement>,

    cycling_phase: CyclingPhase,
    tabwin_keyboard_grab_active: bool,
    tabwin_pointer_grab_active: bool,
    tabwin_touch_grab_active: bool,

    tabwin_output: Option<Output>,
    window_preview_size: Option<u32>,
    window_icon_size: Option<u32>,

    tabwin_window: Option<WindowElement>,
    pending_cycle_key: Option<(Keysym, Keycode)>,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::core) enum SwitchScope {
    SameApplication,
    DifferentApplication,
}

impl CyclingState {
    pub fn cycling_phase(&self) -> CyclingPhase {
        self.cycling_phase
    }

    pub fn enter_finishing_phase(&mut self) {
        self.cycling_phase = CyclingPhase::Finishing;
    }

    pub fn add_window(&mut self, window: WindowElement) {
        self.cycle_list.push(window);
    }

    // The cycle list runs most- to least-recently-focused.
    pub fn move_window_to_front(&mut self, window: &WindowElement) {
        if let Some(pos) = self.cycle_list.iter().position(|a_window| a_window == window)
            && pos != 0
        {
            let window = self.cycle_list.remove(pos);
            self.cycle_list.insert(0, window);
        }
    }

    pub fn move_window_to_back(&mut self, window: &WindowElement) {
        if let Some(pos) = self.cycle_list.iter().position(|a_window| a_window == window)
            && pos != self.cycle_list.len() - 1
        {
            let window = self.cycle_list.remove(pos);
            self.cycle_list.push(window);
        }
    }

    pub fn remove_window(&mut self, window: &WindowElement) {
        if let Some(pos) = self.cycle_list.iter().position(|a_window| a_window == window) {
            self.cycle_list.remove(pos);
        }
    }

    pub fn set_tabwin_image_sizes(&mut self, preview_size: Option<u32>, icon_size: Option<u32>) {
        self.window_preview_size = preview_size;
        self.window_icon_size = icon_size;
    }

    pub fn set_pending_cycle_key(&mut self, keysym: Keysym, keycode: Keycode) {
        self.pending_cycle_key = Some((keysym, keycode));
    }

    pub fn is_tabwin_window(&self, window: &WindowElement) -> bool {
        self.tabwin_window.as_ref().is_some_and(|tabwin_window| tabwin_window == window)
    }

    pub fn take_pending_cycle_key(&mut self) -> Option<(Keysym, Keycode)> {
        self.pending_cycle_key.take()
    }

    fn grab_active_mut(&mut self, grab: TabwinGrab) -> &mut bool {
        match grab {
            TabwinGrab::Keyboard => &mut self.tabwin_keyboard_grab_active,
            TabwinGrab::Pointer => &mut self.tabwin_pointer_grab_active,
            TabwinGrab::Touch => &mut self.tabwin_touch_grab_active,
        }
    }

    pub fn grab_active(&self, grab: TabwinGrab) -> bool {
        match grab {
            TabwinGrab::Keyboard => self.tabwin_keyboard_grab_active,
            TabwinGrab::Pointer => self.tabwin_pointer_grab_active,
            TabwinGrab::Touch => self.tabwin_touch_grab_active,
        }
    }

    pub fn set_grab_active(&mut self, grab: TabwinGrab, active: bool) {
        *self.grab_active_mut(grab) = active;
    }

    pub fn take_grab_active(&mut self, grab: TabwinGrab) -> bool {
        std::mem::take(self.grab_active_mut(grab))
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

    pub(in crate::core) fn place_tabwin(&mut self, window: &WindowElement, size: Size<i32, Logical>) {
        if self.core.cycling_state.tabwin_window.is_none()
            && let Some(output) = self.core.cycling_state.tabwin_output.as_ref()
            && let Some(output_geo) = self.core.workspace_manager.output_geometry(output)
        {
            let window_size = size.to_f64();
            let output_size = output_geo.size.to_f64();
            let new_x = output_geo.loc.x as f64 + (output_size.w - window_size.w) / 2.;
            let new_y = output_geo.loc.y as f64 + (output_size.h - window_size.h) / 2.;
            let new_location = Point::new(new_x as i32, new_y as i32);

            window.props().flags |= WindowFlags::NO_CYCLE;
            self.set_window_stacking_layer(window, WindowStackingLayer::System);
            self.new_window(window.clone(), new_location, true, None);
            window.set_activate(true);

            self.core.cycling_state.tabwin_window = Some(window.clone());

            let tabwin_geo = Rectangle::new(new_location, size);
            self.start_tabwin_pointer_touch_grabs(window.clone(), self.core.seat.clone(), tabwin_geo);
        }
    }

    pub(in crate::core) fn create_tabwin(&mut self, reverse: bool) {
        if self.core.cycling_state.cycling_phase() == CyclingPhase::None
            && let Some(output) = self.output_under_pointer()
        {
            let windows = self
                .collect_cycle_list()
                .into_iter()
                .flat_map(|window| {
                    self.window_to_tabwin_window(
                        &window,
                        &output,
                        self.core.cycling_state.window_preview_size,
                        self.core.cycling_state.window_icon_size,
                    )
                    .map(|tabwin_window| (window.clone(), tabwin_window))
                })
                .collect::<Vec<_>>();

            let initial_selection = if reverse { windows.last() } else { windows.get(1) }
                .or_else(|| windows.first())
                .map(|(_, tabwin_window)| tabwin_window.window_id);

            if let Some(initial_selection) = initial_selection {
                let tabwin_config = TabwinConfig {
                    output_size: output.geometry().map(|geom| geom.size).unwrap_or_else(|| (1920, 1080).into()),
                    output_scale: output.current_scale().integer_scale().max(1) as u32,
                    mode: self.core.config.cycle_tabwin_mode().into(),
                    window_opacity: (self.core.config.popup_opacity() as f64 / 100.).clamp(0., 1.),
                    show_window_previews: self.core.config.cycle_preview(),
                    windows: windows.into_iter().map(|(_, tabwin_window)| tabwin_window).collect(),
                    initial_selection,
                };

                if let Err(err) = self.core.compositor_ui_state.create_tabwin::<Self>(tabwin_config) {
                    tracing::warn!("Failed to create tabwin: {err}");
                } else {
                    self.core.cycling_state.tabwin_output = Some(output);
                    self.core.cycling_state.cycling_phase = CyclingPhase::Active;
                    self.start_tabwin_keyboard_grab(self.core.seat.clone());
                }
            }
        }
    }

    fn window_should_cycle(&self, window: &WindowElement, cycle_flags: CycleFlags) -> bool {
        Some(window)
            .filter(|window| !window.props().flags.contains(WindowFlags::NO_CYCLE))
            .filter(|window| {
                let workspace_loc = window.props().workspace_loc;
                cycle_flags.contains(CycleFlags::INCLUDE_ALL_WORKSPACES)
                    || workspace_loc == WorkspaceLocation::Single(self.core.workspace_manager.active_workspace_index())
                    || workspace_loc == WorkspaceLocation::All
            })
            .filter(|window| window.accepts_focus())
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
                }
            })
            .is_some()
    }

    fn collect_cycle_list(&mut self) -> Vec<WindowElement> {
        let cycle_flags = self.build_cycle_flags();
        let cycle_list = self.core.cycling_state.cycle_list.clone();
        cycle_list
            .into_iter()
            .filter(|window| self.window_should_cycle(window, cycle_flags))
            .collect::<Vec<_>>()
    }

    pub(in crate::core) fn add_window_to_tabwin(&mut self, window: &WindowElement) {
        if let Some(tabwin_window) = self.core.cycling_state.tabwin_window.as_ref()
            && let Some(output) = self
                .core
                .workspace_manager
                .active_workspace()
                .outputs_for_window(tabwin_window)
                .first()
            && self.window_should_cycle(window, self.build_cycle_flags())
            && let Some(win) = self.window_to_tabwin_window(
                window,
                output,
                self.core.cycling_state.window_preview_size,
                self.core.cycling_state.window_icon_size,
            )
            && let Err(err) = { self.core.compositor_ui_state.tabwin_add_window::<Self>(win) }
        {
            tracing::warn!("Failed to add new window to tabwin: {err}");
        }
    }

    fn window_images(
        &mut self,
        window: &WindowElement,
        output: &Output,
        window_preview_size: Option<u32>,
        window_icon_size: Option<u32>,
    ) -> (Option<Argb32Pixels>, Option<Icon>) {
        let scale = output.current_scale().integer_scale().max(1) as u32;

        let preview = window_preview_size.and_then(|size| {
            self.window_to_image_data(&window.0, size, scale as f64)
                .inspect_err(|err| tracing::info!("Failed to get window preview: {err}"))
                .ok()
        });
        let app_icon = window_icon_size.map(|size| {
            window
                .props()
                .window_icon
                .choose_best(self.core.decorations_resources.icon_theme(), size, scale)
        });

        (preview, app_icon)
    }

    pub(in crate::core) fn send_window_images_to_tabwin(&mut self) {
        if let Some(output) = self.core.cycling_state.tabwin_output.clone() {
            let windows = self.collect_cycle_list();
            for window in windows {
                if self.core.compositor_ui_state.tabwin_contains_window(window.window_id()) {
                    let (preview, app_icon) = self.window_images(
                        &window,
                        &output,
                        self.core.cycling_state.window_preview_size,
                        self.core.cycling_state.window_icon_size,
                    );
                    self.core
                        .compositor_ui_state
                        .tabwin_window_update_images(window.window_id(), preview, app_icon);
                }
            }

            self.core.compositor_ui_state.tabwin_send_done();
        }
    }

    fn build_cycle_range_flags(&self) -> CycleFlags {
        let mut cycle_flags = CycleFlags::empty();
        if self.core.config.cycle_hidden() {
            cycle_flags |= CycleFlags::INCLUDE_HIDDEN;
        }
        if !self.core.config.cycle_minimum() {
            cycle_flags |= CycleFlags::INCLUDE_SKIP_PAGER;
            cycle_flags |= CycleFlags::INCLUDE_SKIP_TASKBAR;
        }
        if self.core.config.cycle_workspaces() {
            cycle_flags |= CycleFlags::INCLUDE_ALL_WORKSPACES;
        }
        cycle_flags
    }

    fn build_cycle_flags(&self) -> CycleFlags {
        let mut cycle_flags = self.build_cycle_range_flags();
        if !self.core.config.cycle_apps_only() {
            cycle_flags |= CycleFlags::INCLUDE_TRANSIENTS;
            cycle_flags |= CycleFlags::INCLUDE_MODAL_PARENTS;
            cycle_flags |= CycleFlags::INCLUDE_UTILITY;
        }
        cycle_flags
    }

    fn window_to_tabwin_window(
        &mut self,
        window: &WindowElement,
        output: &Output,
        window_preview_size: Option<u32>,
        window_icon_size: Option<u32>,
    ) -> Option<TabwinWindow> {
        let client_data = match window.0.underlying_surface() {
            WindowSurface::Wayland(toplevel_surface) => {
                let is_minimized = window.props().is_minimized;
                let app_info = desktop_app_info_for_xdg_toplevel(toplevel_surface);
                let app_name = app_name_for_xdg_toplevel(toplevel_surface, app_info.as_ref());
                let title = window_title_for_xdg_toplevel(toplevel_surface);

                (app_name, title, is_minimized)
            }

            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x11_surface) => {
                use crate::core::util::prettify_name;

                let app_name = prettify_name(&x11_surface.class());

                (app_name, Some(x11_surface.title()), x11_surface.is_hidden())
            }
        };

        match client_data {
            (app_name, Some(title), is_minimized) => {
                let (preview, app_icon) = self.window_images(window, output, window_preview_size, window_icon_size);

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
    }

    pub(in crate::core) fn show_tabwin_window_wireframe(&mut self, window: &WindowElement) {
        if self.core.cycling_state.cycling_phase() == CyclingPhase::Active
            && let Some(workspace) = self.core.workspace_manager.workspace_for_window(window)
            && let Some(geometry) = workspace
                .window_geometry(window)
                .or_else(|| workspace.minimized_window_geometry(window))
        {
            if self.core.grab_state.wireframe().is_none_or(|wireframe| !wireframe.is_unowned()) {
                self.core
                    .grab_state
                    .set_wireframe(Wireframe::new(None, Rectangle::zero(), &self.core.config));
            }
            if let Some(wireframe) = self.core.grab_state.wireframe_mut() {
                wireframe.update_location(geometry.loc);
                wireframe.update_size(geometry.size);
            }
        } else {
            self.core.grab_state.clear_wireframe();
        }
    }

    fn switch_target(&self, focused: &WindowElement, scope: SwitchScope) -> Option<WindowElement> {
        let cycle_flags = match scope {
            SwitchScope::SameApplication => {
                self.build_cycle_range_flags()
                    | CycleFlags::INCLUDE_TRANSIENTS
                    | CycleFlags::INCLUDE_MODAL_PARENTS
                    | CycleFlags::INCLUDE_UTILITY
            }
            SwitchScope::DifferentApplication => self.build_cycle_range_flags(),
        };

        let candidates = self
            .core
            .cycling_state
            .cycle_list
            .iter()
            .filter(|window| *window != focused)
            .filter(|window| self.window_should_cycle(window, cycle_flags))
            .collect::<Vec<_>>();

        match scope {
            // The cycle list runs most- to least-recently-focused, so taking the last match
            // rotates through every window of the application on repeated presses, rather than
            // bouncing between the two most recent ones.
            SwitchScope::SameApplication => candidates
                .into_iter()
                .rev()
                .find(|window| window.same_application_as(focused))
                .cloned(),

            // Reduce each of the other applications to its most-recently-focused window before
            // picking one, so that an application is visited once per rotation rather than once
            // per window it happens to have open.
            SwitchScope::DifferentApplication => candidates
                .into_iter()
                .filter(|window| !window.same_application_as(focused) && window.is_main_window())
                .fold(Vec::new(), |mut representatives: Vec<&WindowElement>, window| {
                    if !representatives
                        .iter()
                        .any(|representative| representative.same_application_as(window))
                    {
                        representatives.push(window);
                    }
                    representatives
                })
                .last()
                .copied()
                .cloned(),
        }
    }

    pub(in crate::core) fn switch_to_window(&mut self, focused: &WindowElement, scope: SwitchScope) {
        if let Some(target) = self.switch_target(focused, scope) {
            self.cycle_activate_window(&target);
        }
    }

    pub(in crate::core) fn cycle_activate_window(&mut self, window: &WindowElement) {
        if window.shaded() {
            self.set_window_shaded(window, false);
        }
        self.activate_window(window, true, ActivateAction::Switch, None);
    }

    pub(in crate::core) fn clear_window_cycling_state(&mut self) {
        self.core.cycling_state.cycling_phase = CyclingPhase::None;
        self.core.cycling_state.window_preview_size = None;
        self.core.cycling_state.window_icon_size = None;
        self.core.cycling_state.tabwin_output = None;
        self.core.cycling_state.pending_cycle_key = None;
        self.core.grab_state.clear_wireframe();
        if let Some(window) = self.core.cycling_state.tabwin_window.take() {
            self.close_window(&window);
        }
    }
}
