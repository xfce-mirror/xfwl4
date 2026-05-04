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

use std::collections::{HashMap, HashSet};

use smithay::{
    backend::input::ButtonState,
    input::{
        Seat, SeatHandler,
        pointer::{ButtonEvent, MotionEvent},
    },
    utils::{Logical, Point, SERIAL_COUNTER, Serial},
};

use crate::{
    backend::Backend,
    core::{
        config::adjacent_monitor_in_direction,
        focus::PointerFocusTarget,
        shell::{GrabTrigger, ResizeEdge, WindowElement},
        state::Xfwl4State,
        util::{BTN_RIGHT, Direction, OutputExt},
    },
    protocols::xfwl4_compositor_ui::{
        CompositorUiHandler, CompositorUiState, WindowMenuAction, WindowMenuState,
        proto::xfwl4_ui_window_menu_v1::{ActionType, Direction as WindowMenuDirection, StackingState},
    },
};

pub struct PendingWindowMenuState<H: SeatHandler> {
    pub focus: PointerFocusTarget,
    pub location: Point<i32, Logical>,
    pub seat: Seat<H>,
    pub serial: Serial,
}

pub enum ActionLocation {
    WindowRelative(Point<i32, Logical>),
    Absolute(Point<i32, Logical>),
}

impl<BackendData: Backend + 'static> CompositorUiHandler for Xfwl4State<BackendData> {
    fn compositor_ui_state(&mut self) -> &mut CompositorUiState {
        &mut self.core.compositor_ui_state
    }

    fn icon_sizes(&mut self, icon_sizes: HashSet<i32>) {
        for icon_size in icon_sizes {
            self.core.add_toplevel_icon_size(icon_size);
        }
    }

    fn theme_colors(&mut self, theme_colors: HashMap<String, gtk::gdk::RGBA>) {
        self.core.config.update_color_names(theme_colors);
        if let Err(err) = self.load_decoration_theme() {
            tracing::warn!("Failed to reload decoration theme after theme color change: {err}");
        }
    }

    fn tabwin_hover(&mut self, hover_window_id: u32) {
        let predicate = |elem: &WindowElement| elem.window_id() == hover_window_id;

        let workspace_and_window = if let Some(window) = self.core.workspace_manager.active_workspace().find_window(predicate) {
            Some((window, self.core.workspace_manager.active_workspace_mut()))
        } else {
            self.core
                .workspace_manager
                .find_window_and_workspace_mut(predicate)
                .map(|(window, _, workspace)| (window, workspace))
        };

        if let Some((window, workspace)) = workspace_and_window {
            if self.core.config.cycle_raise() {
                workspace.raise_window(&window, false);
            }

            if self.core.config.cycle_draw_frame() {
                self.show_tabwin_window_wireframe(&window);
            }
        }
    }

    fn tabwin_finished(&mut self, selected_window_id: Option<u32>) {
        if let Some(selected_window_id) = selected_window_id
            && let Some(window) = self
                .core
                .workspace_manager
                .find_window(|elem: &WindowElement| elem.window_id() == selected_window_id)
        {
            if window.minimized() {
                self.set_window_unminimized(&window, SERIAL_COUNTER.next_serial(), true);
            } else {
                self.activate_window(&window, true, true, None);
            }
        }

        if self.core.tabwin_grabs_active {
            self.core.tabwin_grabs_active = false;
            if let Some(keyboard) = self.core.seat.get_keyboard() {
                keyboard.unset_grab(self);
            }
            let serial = SERIAL_COUNTER.next_serial();
            let time = self.core.clock.now().as_millis();
            let pointer = self.core.pointer.clone();
            pointer.unset_grab(self, serial, time);
            if let Some(touch) = self.core.seat.clone().get_touch() {
                touch.unset_grab(self);
            }
        }

        self.core.cycling_windows = false;
    }

    fn window_menu_ready(&mut self) {
        if let Some(state) = self.core.pending_window_menu_state.take()
            && let Some(window_menu_anchor) = self.core.window_menu_anchor.as_ref()
            && let Some(pointer) = state.seat.get_pointer()
        {
            // Map the anchor window so rendering and hit-testing will work
            // without hacks.
            self.new_window(window_menu_anchor.clone(), state.location, false, None);

            // Release any active grab (e.g. ClickGrab from the button press
            // that triggered show_window_menu).  ClickGrab ignores the focus
            // parameter in motion events, so we must release it before
            // synthesizing events to the anchor window.
            pointer.unset_grab(self, state.serial, self.core.clock.now().as_millis());

            // Next send motion to the anchor window to give it pointer focus.
            let pointer_loc = pointer.current_location();
            let motion_event = MotionEvent {
                location: pointer_loc,
                serial: SERIAL_COUNTER.next_serial(),
                time: self.core.clock.now().as_millis(),
            };
            pointer.motion(self, Some((state.focus, pointer_loc)), &motion_event);
            pointer.frame(self);

            // Then synthesize a right-click so GTK will pop up the menu.
            let button_event = ButtonEvent {
                state: ButtonState::Pressed,
                serial: SERIAL_COUNTER.next_serial(),
                time: self.core.clock.now().as_millis(),
                button: BTN_RIGHT,
            };
            pointer.button(self, &button_event);
            pointer.frame(self);
        }
    }

    fn window_menu_action(&mut self, window_id: u32, action: WindowMenuAction) {
        if let Some(window) = self.core.workspace_manager.find_window(|elem| elem.window_id() == window_id) {
            match action {
                WindowMenuAction::Action(action) => self.handle_window_menu_action(window, action),
                WindowMenuAction::MoveToWorkspace(index) => self.handle_window_menu_move_to_workspace(window, index),
                WindowMenuAction::MoveToOutput(direction) => self.handle_window_menu_move_to_output_in_direction(window, direction),
            }
        }
    }

    fn window_menu_dismissed(&mut self) {
        if let Some(window_menu_anchor) = self.core.window_menu_anchor.clone() {
            self.remove_window(&window_menu_anchor);

            let pointer = self.core.pointer.clone();

            // Synthesize a button release on the anchor window.  If the original trigger
            // for the menu popping up was indeed the right mouse button, this will be a
            // spurious release (which hopefully any app/toolkit should ignore), but if the
            // trigger was a different mouse button, or a touch event, not synthesizing the
            // release will cause the anchor window to think that our synthesized right
            // Then synthesize a right-click so GTK will pop up the menu.
            let button_event = ButtonEvent {
                state: ButtonState::Released,
                serial: SERIAL_COUNTER.next_serial(),
                time: self.core.clock.now().as_millis(),
                button: BTN_RIGHT,
            };
            pointer.button(self, &button_event);
            pointer.frame(self);

            // Pointer focus will still be on the anchor window at this point, so let's
            // move it back to whatever surface is under the pointer.
            let pointer_loc = pointer.current_location();
            let focus_surface = self.surface_under(pointer_loc);
            pointer.motion(
                self,
                focus_surface,
                &MotionEvent {
                    location: pointer_loc,
                    serial: SERIAL_COUNTER.next_serial(),
                    time: self.core.clock.now().as_millis(),
                },
            );
            pointer.frame(self);
        }
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub(in crate::core) fn pop_up_window_menu(
        &mut self,
        window: &WindowElement,
        seat: &Seat<Self>,
        serial: Serial,
        location: ActionLocation,
    ) {
        if let Some(window_location) = self.core.workspace_manager.active_workspace().window_location(window)
            && let Some(window_menu_anchor) = self.core.window_menu_anchor.as_ref()
            && let Some(window_menu_anchor_focus_target) = window_menu_anchor
                .wl_surface()
                .map(|surf| PointerFocusTarget::WlSurface(surf.into_owned()))
        {
            let location = match location {
                ActionLocation::Absolute(location) => location,
                ActionLocation::WindowRelative(location) => {
                    if let Some(window_decorations) = window.decoration_state().window_decorations() {
                        window_location + location + window_decorations.decorations_offset()
                    } else {
                        window_location + location - window.content_geometry().loc
                    }
                }
            };

            let workspace_names = if !window.sticky() {
                self.core
                    .workspace_manager
                    .workspaces()
                    .iter()
                    .map(|workspace| workspace.name().to_owned())
                    .collect()
            } else {
                vec![]
            };

            if let Some(current_output_and_rect) = self.output_and_rect_for_window(window) {
                let outputs_and_rects = self.outputs_and_rects();
                let adjacent_outputs = [
                    adjacent_monitor_in_direction(&outputs_and_rects, &current_output_and_rect, Direction::Up)
                        .map(|_| WindowMenuDirection::Up),
                    adjacent_monitor_in_direction(&outputs_and_rects, &current_output_and_rect, Direction::Down)
                        .map(|_| WindowMenuDirection::Down),
                    adjacent_monitor_in_direction(&outputs_and_rects, &current_output_and_rect, Direction::Left)
                        .map(|_| WindowMenuDirection::Left),
                    adjacent_monitor_in_direction(&outputs_and_rects, &current_output_and_rect, Direction::Right)
                        .map(|_| WindowMenuDirection::Right),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();

                let state = PendingWindowMenuState {
                    focus: window_menu_anchor_focus_target,
                    location,
                    seat: seat.clone(),
                    serial,
                };
                if let Err(err) = self.core.compositor_ui_state.create_window_menu::<Self>(WindowMenuState {
                    window_id: window.window_id(),
                    maximize_state: Some(window.maximized()),
                    can_minimize: true,
                    can_move: true,
                    can_resize: !window.maximized(),
                    stacking_state: if window.normal_stacking() {
                        StackingState::Normal
                    } else if window.always_on_bottom() {
                        StackingState::AlwaysBelow
                    } else {
                        StackingState::AlwaysOnTop
                    },
                    shaded_state: Some(window.shaded()),
                    fullscreen_state: Some(window.fullscreened()),
                    sticky: window.sticky(),
                    workspace_names,
                    current_workspace: self.core.workspace_manager.active_workspace_index(),
                    adjacent_outputs,
                    can_close: true,
                }) {
                    tracing::warn!("Failed to create window menu: {err}");
                } else {
                    self.core.pending_window_menu_state = Some(state);
                }
            }
        }
    }

    fn handle_window_menu_action(&mut self, window: WindowElement, action: ActionType) {
        match action {
            ActionType::ToggleMaximize => {
                if !window.maximized() {
                    self.set_window_maximized(&window, None);
                } else {
                    self.set_window_unmaximized(&window, None);
                }
            }
            ActionType::Minimize => self.set_window_minimized(&window),
            ActionType::MinimizeOtherWindows => {
                let other_windows = self
                    .core
                    .workspace_manager
                    .active_workspace()
                    .visible_windows()
                    .filter(|elem| **elem != window)
                    .cloned()
                    .collect::<Vec<_>>();
                for other_window in other_windows {
                    self.set_window_minimized(&other_window);
                }
            }
            ActionType::Move => {
                let serial = SERIAL_COUNTER.next_serial();
                // Set focus back to the window, because it may still be on the menu anchor
                // window.
                self.focus_window(&window, serial, None);
                // Use a keyboard trigger because we don't have a pointer button pressed
                self.start_window_move(window, self.core.seat.clone(), serial, GrabTrigger::Keyboard);
            }
            ActionType::Resize => {
                let serial = SERIAL_COUNTER.next_serial();
                // Set focus back to the window, because it may still be on the menu anchor
                // window.
                self.focus_window(&window, serial, None);
                self.start_window_resize(
                    window,
                    self.core.seat.clone(),
                    serial,
                    ResizeEdge::BOTTOM_RIGHT,
                    // Use a keyboard trigger because we don't have a pointer button pressed
                    GrabTrigger::Keyboard,
                );
            }
            ActionType::StackOnTop => self.set_window_always_on_top(&window),
            ActionType::StackNormal => self.set_window_normal_stacking(&window),
            ActionType::StackBelow => self.set_window_always_on_bottom(&window),
            ActionType::ToggleShade => {
                self.set_window_shaded(&window, !window.shaded());
            }
            ActionType::ToggleFullscreen => {
                let pointer_loc = self.core.pointer.current_location();
                let pointer_output = self
                    .core
                    .workspace_manager
                    .outputs()
                    .find(|output| {
                        output
                            .geometry()
                            .filter(|output_rect| output_rect.contains(pointer_loc.to_i32_round()))
                            .is_some()
                    })
                    .cloned();
                self.set_window_fullscreen(&window, pointer_output);
            }
            ActionType::ToggleSticky => {
                self.set_window_sticky(&window, !window.sticky());
            }
            ActionType::Close => {
                self.close_window(&window);
            }
        }
    }

    fn handle_window_menu_move_to_workspace(&mut self, window: WindowElement, index: u32) {
        self.move_window_to_workspace_index(&window, index);
    }

    fn handle_window_menu_move_to_output_in_direction(&mut self, window: WindowElement, direction: WindowMenuDirection) {
        self.move_window_to_output_in_direction(&window, direction.into());
    }
}

impl From<WindowMenuDirection> for Direction {
    fn from(value: WindowMenuDirection) -> Self {
        match value {
            WindowMenuDirection::Up => Self::Up,
            WindowMenuDirection::Down => Self::Down,
            WindowMenuDirection::Left => Self::Left,
            WindowMenuDirection::Right => Self::Right,
        }
    }
}
