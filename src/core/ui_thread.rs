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

use std::{cell::Cell, rc::Rc};

use gtk::cairo;
use smithay::{
    backend::input::ButtonState,
    input::{
        Seat,
        pointer::{ButtonEvent, MotionEvent},
    },
    reexports::{calloop::channel, wayland_server::Resource},
    utils::{Logical, Point, SERIAL_COUNTER, Serial},
    wayland::seat::WaylandFocus,
};

use crate::{
    backend::Backend,
    core::{focus::PointerFocusTarget, shell::WindowElement, state::Xfwl4State, util::OutputExt},
    ui::{
        FromUiMessage, ToUiMessage,
        tabwin::TabwinAction,
        window_menu::{FullscreenState, MaximizeState, ShadeState, StackingState, WindowMenuAction, WindowMenuState},
    },
};

const BTN_RIGHT: u32 = 0x111;

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub fn handle_ui_thread_message(&mut self, message: FromUiMessage) -> anyhow::Result<()> {
        match message {
            FromUiMessage::DefaultMainContextClaimed => Ok(()),
            FromUiMessage::IconThemeChanged(icon_theme_name) => {
                self.core.icon_theme.set_icon_theme_name(&icon_theme_name);
                self.update_window_decorations_icon_theme();
                Ok(())
            }
            FromUiMessage::IconSizes(sizes) => {
                for size in sizes {
                    tracing::debug!("adding icon size {size}");
                    self.core.xdg_toplevel_icon_manager.add_icon_size(size);
                }
                Ok(())
            }
            FromUiMessage::TabwinAction(TabwinAction::HoverWindow(_)) => Ok(()),
            FromUiMessage::TabwinAction(TabwinAction::WindowSelected(selected)) => {
                let predicate = |elem: &WindowElement| elem.0.wl_surface().is_some_and(|surf| surf.id() == selected);

                if let Some(window) = self.core.workspace_manager.active_workspace().find_element(predicate) {
                    if window.minimized() {
                        self.set_window_unminimized(&window, true);
                    } else {
                        let workspace = self.core.workspace_manager.active_workspace_mut();
                        workspace.raise_window(&window, true);
                    }
                } else {
                    let mut idx_and_window = None::<(u32, WindowElement)>;
                    for (idx, workspace) in self.core.workspace_manager.workspaces().iter().enumerate() {
                        if let Some(window) = workspace.find_element(predicate) {
                            idx_and_window = Some((idx as u32, window));
                            break;
                        }
                    }

                    if let Some((idx, window)) = idx_and_window {
                        self.core.workspace_manager.set_active_workspace(idx);
                        if window.minimized() {
                            self.set_window_unminimized(&window, true);
                        } else if let Some(workspace) = self.core.workspace_manager.workspaces_mut().get_mut(idx as usize) {
                            workspace.raise_window(&window, true);
                        }
                    }
                }

                Ok(())
            }
            FromUiMessage::WindowMenuAction(window_id, action) => {
                tracing::debug!("got window menu action {action:?}");
                if let Some(window) = self
                    .core
                    .workspace_manager
                    .active_workspace()
                    .find_element(|elem| elem.wl_surface().is_some_and(|surf| surf.id() == window_id))
                {
                    self.handle_window_menu_action(window, action);
                }
                Ok(())
            }
            FromUiMessage::WindowMenuDismissed => {
                if let Some(window_menu_anchor) = self.core.window_menu_anchor.as_ref() {
                    self.core.workspace_manager.active_workspace_mut().unmap_elem(window_menu_anchor);

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
                Ok(())
            }
            FromUiMessage::ThemeColorsChanged(theme_colors) => {
                if self.core.config.update_color_names(theme_colors)
                    && let Err(err) = self.load_decoration_theme()
                {
                    tracing::warn!("Failed to load theme: {err}");
                }
                Ok(())
            }
            FromUiMessage::FontSettingsChanged(font_settings) => {
                let mut options = gtk::cairo::FontOptions::new().expect("creating cairo FontOptions should not fail");
                options.set_hint_metrics(cairo::HintMetrics::On);
                options.set_hint_style(font_settings.hint_style);
                options.set_subpixel_order(font_settings.subpixel_order);
                options.set_antialias(font_settings.antialias);

                self.core.font_options = options;
                self.update_window_decorations_font_options();

                Ok(())
            }
            FromUiMessage::PointerBehaviorSettingsChanged(settings) => {
                self.core.pointer_behavior_settings = settings;
                Ok(())
            }
        }
    }

    pub fn pop_up_window_menu(&mut self, window: &WindowElement, seat: &Seat<Self>, serial: Serial, location: Point<i32, Logical>) {
        if let Some(window_location) = self.core.workspace_manager.active_workspace().element_location(window)
            && let Some(window_id) = window.0.wl_surface().map(|surf| surf.id())
            && let Some(pointer) = seat.get_pointer()
            && let Some(window_menu_anchor) = self.core.window_menu_anchor.as_ref()
            && let Some(window_menu_anchor_focus_target) = window_menu_anchor
                .wl_surface()
                .map(|surf| PointerFocusTarget::WlSurface(surf.into_owned()))
        {
            let mut location = window_location + location;
            if let Some(window_decorations) = window.decoration_state().window_decorations() {
                location += window_decorations.decorations_offset();
            } else {
                location -= window.0.geometry().loc;
            }

            let (tx, rx) = channel::channel::<()>();
            let focus = Cell::new(Some(window_menu_anchor_focus_target));
            let token = Rc::new(Cell::new(None));

            let tok = self
                .core
                .handle
                .insert_source(rx, {
                    let token = Rc::clone(&token);
                    move |event, _, state| {
                        if let channel::Event::Msg(()) = event {
                            if let Some(focus) = focus.take()
                                && let Some(window_menu_anchor) = state.core.window_menu_anchor.as_ref()
                            {
                                // Map the anchor window so rendering and hit-testing will work
                                // without hacks.
                                state.core.workspace_manager.active_workspace_mut().map_element(
                                    window_menu_anchor.clone(),
                                    location,
                                    false,
                                );

                                // Release any active grab (e.g. ClickGrab from the button press
                                // that triggered show_window_menu).  ClickGrab ignores the focus
                                // parameter in motion events, so we must release it before
                                // synthesizing events to the anchor window.
                                pointer.unset_grab(state, serial, state.core.clock.now().as_millis());

                                // Next send motion to the anchor window to give it pointer focus.
                                let pointer_loc = pointer.current_location();
                                let motion_event = MotionEvent {
                                    location: pointer_loc,
                                    serial: SERIAL_COUNTER.next_serial(),
                                    time: state.core.clock.now().as_millis(),
                                };
                                pointer.motion(state, Some((focus.clone(), pointer_loc)), &motion_event);
                                pointer.frame(state);

                                // Then synthesize a right-click so GTK will pop up the menu.
                                let button_event = ButtonEvent {
                                    state: ButtonState::Pressed,
                                    serial: SERIAL_COUNTER.next_serial(),
                                    time: state.core.clock.now().as_millis(),
                                    button: BTN_RIGHT,
                                };
                                pointer.button(state, &button_event);
                                pointer.frame(state);
                            }

                            if let Some(token) = token.take() {
                                state.core.handle.remove(token);
                            }
                        }
                    }
                })
                .expect("failed to register one-shot channel with event loop");
            token.set(Some(tok));

            let current_workspace = self.core.workspace_manager.active_workspace_index();
            let workspace_names = self
                .core
                .workspace_manager
                .workspaces()
                .iter()
                .map(|workspace| workspace.name().to_owned())
                .collect();

            let outputs = self.backend.outputs();
            let current_monitor = self
                .core
                .workspace_manager
                .active_workspace()
                .outputs_for_element(window)
                .into_iter()
                .next()
                .and_then(|output| {
                    outputs.iter().find_map(|(global_id, an_output)| {
                        if output == *an_output {
                            output.geometry().map(|geom| (global_id.clone(), geom))
                        } else {
                            None
                        }
                    })
                });
            let monitors = outputs
                .into_iter()
                .flat_map(|(global_id, output)| output.geometry().map(|geom| (global_id, geom)))
                .collect();

            let _ = self.core.to_ui_channel_tx.send(ToUiMessage::PrepareWindowMenu(
                tx,
                WindowMenuState {
                    window_id,
                    maximize_state: MaximizeState::Normal,
                    can_minimize: true,
                    can_move: true,
                    can_resize: true,
                    stacking_state: StackingState::Normal,
                    shade_state: ShadeState::Normal,
                    fullscreen_state: FullscreenState::Normal,
                    sticky: false,
                    can_move_workspaces: true,
                    current_workspace,
                    workspace_names,
                    current_monitor,
                    monitors,
                    can_close: true,
                },
            ));
        }
    }

    fn handle_window_menu_action(&mut self, window: WindowElement, action: WindowMenuAction) {
        match action {
            WindowMenuAction::ToggleMaximize => {
                self.set_window_maximized(&window, !window.maximized());
            }
            WindowMenuAction::Minimize => self.set_window_minimized(&window),
            WindowMenuAction::MinimizeOtherWindows => {
                let other_windows = self
                    .core
                    .workspace_manager
                    .active_workspace()
                    .elements()
                    .filter(|elem| **elem != window)
                    .cloned()
                    .collect::<Vec<_>>();
                for other_window in other_windows {
                    self.set_window_minimized(&other_window);
                }
            }
            WindowMenuAction::Move => {
                // TODO
            }
            WindowMenuAction::Resize => {
                // TODO
            }
            WindowMenuAction::StackOnTop => {
                // TODO
            }
            WindowMenuAction::StackNormal => {
                // TODO
            }
            WindowMenuAction::StackBelow => {
                // TODO
            }
            WindowMenuAction::ToggleShade => {
                self.set_window_shaded(&window, !window.shaded());
            }
            WindowMenuAction::Fullscreen => {
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
            WindowMenuAction::ToggleSticky => {
                // TODO
            }
            WindowMenuAction::MoveToWorkspace(idx) => {
                let cur_workspace = self.core.workspace_manager.active_workspace_mut();
                let loc = cur_workspace.element_location(&window).unwrap_or_default();
                cur_workspace.unmap_elem(&window);

                if let Some(new_workspace) = self.core.workspace_manager.workspaces_mut().get_mut(idx as usize) {
                    new_workspace.map_element(window, loc, false);
                } else {
                    // This shouldn't happen, but...
                    self.core.workspace_manager.active_workspace_mut().map_element(window, loc, true);
                }
            }
            WindowMenuAction::MoveToOutput(output_rect) => {
                let cur_workspace = self.core.workspace_manager.active_workspace_mut();
                let loc = cur_workspace.element_location(&window).unwrap_or_default();
                let new_location = if let Some(cur_output_rect) = cur_workspace.outputs_for_element(&window).iter().find_map(|output| {
                    output
                        .geometry()
                        .and_then(|cur_output_rect| cur_output_rect.contains(loc).then_some(cur_output_rect))
                }) {
                    let offset_in_cur_output = loc - cur_output_rect.loc;
                    output_rect.loc + offset_in_cur_output
                } else {
                    output_rect.loc
                };
                cur_workspace.map_element(window, new_location, false);
            }
            WindowMenuAction::Close => {
                window.close();
            }
        }
    }
}
