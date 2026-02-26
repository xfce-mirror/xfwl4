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
//
// Portions of this file are based on "anvil", an example compositor
// based on the smithay crate, and are licensed under the MIT license
// with the following terms:
//
// Copyright (C) Victor Berger <victor.berger@m4x.org>
// Copyright (C) Drakulix (Victoria Brekenfeld)
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use std::{convert::TryInto, process::Command};

use smithay::{
    backend::input::{
        self, Axis, AxisSource, Event, InputBackend, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    desktop::{WindowSurfaceType, layer_map_for_output},
    input::{
        keyboard::{FilterResult, KeyboardHandle, Keycode, Keysym, ModifiersState, keysyms as xkb, xkb::ModMask},
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    output::Scale,
    reexports::{wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1, wayland_server::protocol::wl_pointer},
    utils::{Logical, Point, SERIAL_COUNTER, Serial, Transform},
    wayland::{
        input_method::InputMethodSeat,
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat,
        shell::wlr_layer::{KeyboardInteractivity, Layer as WlrLayer},
        virtual_keyboard::VirtualKeyboardHandler,
    },
};
use tracing::{debug, error, info};

#[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
use smithay::backend::input::AbsolutePositionEvent;
#[cfg(any(feature = "winit", feature = "x11"))]
use smithay::output::Output;

use crate::{
    Xfwl4State,
    backend::Backend,
    focus::PointerFocusTarget,
    ui::{ToUiMessage, tabwin::TabwinConfig},
};

impl<BackendData: Backend> Xfwl4State<BackendData> {
    pub(crate) fn process_common_key_action(&mut self, action: KeyAction) {
        match action {
            KeyAction::None => (),

            KeyAction::Quit => {
                info!("Quitting.");
                self.shutdown();
            }

            KeyAction::Run(cmd, args) => {
                info!(cmd, "Starting program");

                if let Err(e) = Command::new(&cmd)
                    .args(args)
                    .envs(self.socket_name.clone().map(|v| ("WAYLAND_DISPLAY", v)).into_iter().chain(
                        #[cfg(feature = "xwayland")]
                        self.xdisplay.map(|v| ("DISPLAY", format!(":{v}"))),
                        #[cfg(not(feature = "xwayland"))]
                        None,
                    ))
                    .spawn()
                {
                    error!(cmd, err = %e, "Failed to start program");
                }
            }

            KeyAction::ToggleDecorations => {
                for workspace in self.workspace_manager.workspaces() {
                    for element in workspace.elements() {
                        #[allow(irrefutable_let_patterns)]
                        if let Some(toplevel) = element.0.toplevel() {
                            let mode_changed = toplevel.with_pending_state(|state| {
                                if let Some(current_mode) = state.decoration_mode {
                                    let new_mode = if current_mode == zxdg_toplevel_decoration_v1::Mode::ClientSide {
                                        zxdg_toplevel_decoration_v1::Mode::ServerSide
                                    } else {
                                        zxdg_toplevel_decoration_v1::Mode::ClientSide
                                    };
                                    state.decoration_mode = Some(new_mode);
                                    true
                                } else {
                                    false
                                }
                            });

                            if mode_changed && toplevel.is_initial_configure_sent() {
                                toplevel.send_pending_configure();
                            }
                        }
                    }
                }
            }

            KeyAction::WorkspaceUp => self.workspace_manager.activate_up(),
            KeyAction::WorkspaceDown => self.workspace_manager.activate_down(),
            KeyAction::WorkspaceLeft => self.workspace_manager.activate_left(),
            KeyAction::WorkspaceRight => self.workspace_manager.activate_right(),

            KeyAction::StartCycleWindowsForward | KeyAction::StartCycleWindowsReverse => {
                if let Some(output) = self.output_under_pointer() {
                    let clients = self.collect_tabwin_clients(&output);

                    let initial_selection = if let KeyAction::StartCycleWindowsForward = action {
                        clients.get(1).or_else(|| clients.first())
                    } else {
                        clients.last()
                    }
                    .map(|client| client.id.clone());

                    if let Some(initial_selection) = initial_selection {
                        self.cycling_windows = true;
                        let _ = self.to_ui_channel_tx.send(ToUiMessage::ShowTabwin(TabwinConfig {
                            mode: self.config.cycle_tabwin_mode(),
                            cycle_preview: self.config.cycle_preview(),
                            clients,
                            initial_selection,
                        }));
                    }
                }
            }

            KeyAction::CycleWindowsNext => {
                if self.cycling_windows {
                    let _ = self.to_ui_channel_tx.send(ToUiMessage::TabwinNext);
                }
            }

            KeyAction::CycleWindowsPrevious => {
                if self.cycling_windows {
                    let _ = self.to_ui_channel_tx.send(ToUiMessage::TabwinPrevious);
                }
            }

            KeyAction::FinishCycleWindows => {
                if self.cycling_windows {
                    let _ = self.to_ui_channel_tx.send(ToUiMessage::FinshTabwin);
                    self.cycling_windows = false;
                }
            }

            KeyAction::CancelCycleWindows => {
                if self.cycling_windows {
                    let _ = self.to_ui_channel_tx.send(ToUiMessage::CancelTabwin);
                    self.cycling_windows = false;
                }
            }

            _ => unreachable!("Common key action handler encountered backend specific action {:?}", action),
        }
    }

    pub(crate) fn keyboard_key_to_action<B: InputBackend>(&mut self, evt: B::KeyboardKeyEvent) -> KeyAction {
        let keycode = evt.key_code();
        let state = evt.state();
        debug!(?keycode, ?state, "key");
        let serial = SERIAL_COUNTER.next_serial();
        let time = Event::time_msec(&evt);
        let mut suppressed_keys = self.suppressed_keys.clone();
        let keyboard = self.seat.get_keyboard().unwrap();
        let workspace = self.workspace_manager.active_workspace();
        let cycling_windows = self.cycling_windows;

        for layer in self.layer_shell_state.layer_surfaces().rev() {
            let exclusive = layer.with_cached_state(|data| {
                data.keyboard_interactivity == KeyboardInteractivity::Exclusive
                    && (data.layer == WlrLayer::Top || data.layer == WlrLayer::Overlay)
            });
            if exclusive {
                let surface = workspace.outputs().find_map(|o| {
                    let map = layer_map_for_output(o);

                    map.layers().find(|l| l.layer_surface() == &layer).cloned()
                });
                if let Some(surface) = surface {
                    keyboard.set_focus(self, Some(surface.into()), serial);
                    keyboard.input::<(), _>(self, keycode, state, serial, time, |_, _, _| FilterResult::Forward);
                    return KeyAction::None;
                };
            }
        }

        let inhibited = workspace
            .element_under(self.pointer.current_location())
            .and_then(|(window, _)| {
                let surface = window.wl_surface()?;
                self.seat.keyboard_shortcuts_inhibitor_for_surface(&surface)
            })
            .map(|inhibitor| inhibitor.is_active())
            .unwrap_or(false);

        let action = keyboard
            .input(self, keycode, state, serial, time, |data, modifiers, handle| {
                let keysym = handle.modified_sym();

                debug!(
                    ?state,
                    mods = ?modifiers,
                    keysym = ::xkbcommon::xkb::keysym_get_name(keysym),
                    "keysym"
                );

                // If the key is pressed and triggered a action
                // we will not forward the key to the client.
                // Additionally add the key to the suppressed keys
                // so that we can decide on a release if the key
                // should be forwarded to the client or not.
                if let KeyState::Pressed = state {
                    if !inhibited {
                        let action = data.process_keyboard_shortcut(*modifiers, keysym);

                        if action.is_some() {
                            suppressed_keys.push(keysym);
                        }

                        action.map(FilterResult::Intercept).unwrap_or(FilterResult::Forward)
                    } else {
                        FilterResult::Forward
                    }
                } else if let KeyState::Released = state
                    && cycling_windows
                    && !modifiers.alt
                    && !modifiers.shift
                {
                    FilterResult::Intercept(KeyAction::FinishCycleWindows)
                } else {
                    let suppressed = suppressed_keys.contains(&keysym);
                    if suppressed {
                        suppressed_keys.retain(|k| *k != keysym);
                        FilterResult::Intercept(KeyAction::None)
                    } else {
                        FilterResult::Forward
                    }
                }
            })
            .unwrap_or(KeyAction::None);

        self.suppressed_keys = suppressed_keys;
        action
    }

    pub(crate) fn on_pointer_button<B: InputBackend>(&mut self, evt: B::PointerButtonEvent) {
        let serial = SERIAL_COUNTER.next_serial();
        let button = evt.button_code();

        let state = wl_pointer::ButtonState::from(evt.state());

        if wl_pointer::ButtonState::Pressed == state {
            self.update_keyboard_focus(self.pointer.current_location(), serial);
        };
        let pointer = self.pointer.clone();
        pointer.button(
            self,
            &ButtonEvent {
                button,
                state: state.try_into().unwrap(),
                serial,
                time: evt.time_msec(),
            },
        );
        pointer.frame(self);
    }

    pub(crate) fn update_keyboard_focus(&mut self, location: Point<f64, Logical>, serial: Serial) {
        let keyboard = self.seat.get_keyboard().unwrap();
        let touch = self.seat.get_touch();
        let input_method = self.seat.input_method();
        // change the keyboard focus unless the pointer or keyboard is grabbed
        // We test for any matching surface type here but always use the root
        // (in case of a window the toplevel) surface for the focus.
        // So for example if a user clicks on a subsurface or popup the toplevel
        // will receive the keyboard focus. Directly assigning the focus to the
        // matching surface leads to issues with clients dismissing popups and
        // subsurface menus (for example firefox-wayland).
        // see here for a discussion about that issue:
        // https://gitlab.freedesktop.org/wayland/wayland/-/issues/294
        if !self.pointer.is_grabbed()
            && (!keyboard.is_grabbed() || input_method.keyboard_grabbed())
            && !touch.map(|touch| touch.is_grabbed()).unwrap_or(false)
        {
            let workspace = self.workspace_manager.active_workspace_mut();
            let output = workspace.output_under(location).next().cloned();
            if let Some(output) = output.as_ref() {
                let output_geo = workspace.output_geometry(output).unwrap();
                if let Some(window) = workspace.fullscreen_window_for_output(output)
                    && let Some((_, _)) = window.surface_under(location - output_geo.loc.to_f64(), WindowSurfaceType::ALL)
                {
                    #[cfg(feature = "xwayland")]
                    if let Some(surface) = window.0.x11_surface() {
                        self.xwm.as_mut().unwrap().raise_window(surface).unwrap();
                    }
                    keyboard.set_focus(self, Some(window.into()), serial);
                    return;
                }

                let layers = layer_map_for_output(output);
                if let Some(layer) = layers
                    .layer_under(WlrLayer::Overlay, location - output_geo.loc.to_f64())
                    .or_else(|| layers.layer_under(WlrLayer::Top, location - output_geo.loc.to_f64()))
                    && layer.can_receive_keyboard_focus()
                    && let Some((_, _)) = layer.surface_under(
                        location - output_geo.loc.to_f64() - layers.layer_geometry(layer).unwrap().loc.to_f64(),
                        WindowSurfaceType::ALL,
                    )
                {
                    keyboard.set_focus(self, Some(layer.clone().into()), serial);
                    return;
                }
            }

            if let Some((window, _)) = workspace.element_under(location).map(|(w, p)| (w.clone(), p)) {
                workspace.raise_element(&window, true);
                #[cfg(feature = "xwayland")]
                if let Some(surface) = window.0.x11_surface() {
                    self.xwm.as_mut().unwrap().raise_window(surface).unwrap();
                }
                keyboard.set_focus(self, Some(window.into()), serial);
                return;
            }

            if let Some(output) = output.as_ref() {
                let output_geo = workspace.output_geometry(output).unwrap();
                let layers = layer_map_for_output(output);
                if let Some(layer) = layers
                    .layer_under(WlrLayer::Bottom, location - output_geo.loc.to_f64())
                    .or_else(|| layers.layer_under(WlrLayer::Background, location - output_geo.loc.to_f64()))
                    && layer.can_receive_keyboard_focus()
                    && let Some((_, _)) = layer.surface_under(
                        location - output_geo.loc.to_f64() - layers.layer_geometry(layer).unwrap().loc.to_f64(),
                        WindowSurfaceType::ALL,
                    )
                {
                    keyboard.set_focus(self, Some(layer.clone().into()), serial);
                }
            };
        }
    }

    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(PointerFocusTarget, Point<f64, Logical>)> {
        let workspace = self.workspace_manager.active_workspace();
        let output = workspace.outputs().find(|o| {
            let geometry = workspace.output_geometry(o).unwrap();
            geometry.contains(pos.to_i32_round())
        })?;
        let output_geo = workspace.output_geometry(output).unwrap();
        let layers = layer_map_for_output(output);

        let mut under = None;
        if let Some((surface, loc)) = workspace
            .fullscreen_window_for_output(output)
            .and_then(|w| w.surface_under(pos - output_geo.loc.to_f64(), WindowSurfaceType::ALL))
        {
            under = Some((surface, loc + output_geo.loc));
        } else if let Some(focus) = layers
            .layer_under(WlrLayer::Overlay, pos - output_geo.loc.to_f64())
            .or_else(|| layers.layer_under(WlrLayer::Top, pos - output_geo.loc.to_f64()))
            .and_then(|layer| {
                let layer_loc = layers.layer_geometry(layer).unwrap().loc;
                layer
                    .surface_under(pos - output_geo.loc.to_f64() - layer_loc.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, loc)| (PointerFocusTarget::from(surface), loc + layer_loc + output_geo.loc))
            })
        {
            under = Some(focus)
        } else if let Some(focus) = workspace.element_under(pos).and_then(|(window, loc)| {
            window
                .surface_under(pos - loc.to_f64(), WindowSurfaceType::ALL)
                .map(|(surface, surf_loc)| (surface, surf_loc + loc))
        }) {
            under = Some(focus);
        } else if let Some(focus) = layers
            .layer_under(WlrLayer::Bottom, pos - output_geo.loc.to_f64())
            .or_else(|| layers.layer_under(WlrLayer::Background, pos - output_geo.loc.to_f64()))
            .and_then(|layer| {
                let layer_loc = layers.layer_geometry(layer).unwrap().loc;
                layer
                    .surface_under(pos - output_geo.loc.to_f64() - layer_loc.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, loc)| (PointerFocusTarget::from(surface), loc + layer_loc + output_geo.loc))
            })
        {
            under = Some(focus)
        };
        under.map(|(s, l)| (s, l.to_f64()))
    }

    pub(crate) fn on_pointer_axis<B: InputBackend>(&mut self, evt: B::PointerAxisEvent) {
        let horizontal_amount = evt
            .amount(input::Axis::Horizontal)
            .unwrap_or_else(|| evt.amount_v120(input::Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.);
        let vertical_amount = evt
            .amount(input::Axis::Vertical)
            .unwrap_or_else(|| evt.amount_v120(input::Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.);
        let horizontal_amount_discrete = evt.amount_v120(input::Axis::Horizontal);
        let vertical_amount_discrete = evt.amount_v120(input::Axis::Vertical);

        {
            let mut frame = AxisFrame::new(evt.time_msec()).source(evt.source());
            if horizontal_amount != 0.0 {
                frame = frame.relative_direction(Axis::Horizontal, evt.relative_direction(Axis::Horizontal));
                frame = frame.value(Axis::Horizontal, horizontal_amount);
                if let Some(discrete) = horizontal_amount_discrete {
                    frame = frame.v120(Axis::Horizontal, discrete as i32);
                }
            }
            if vertical_amount != 0.0 {
                frame = frame.relative_direction(Axis::Vertical, evt.relative_direction(Axis::Vertical));
                frame = frame.value(Axis::Vertical, vertical_amount);
                if let Some(discrete) = vertical_amount_discrete {
                    frame = frame.v120(Axis::Vertical, discrete as i32);
                }

                if self.config.scroll_workspaces()
                    && self
                        .workspace_manager
                        .active_workspace()
                        .element_under(self.pointer.current_location())
                        .is_none()
                {
                    let is_next = vertical_amount > 0.;
                    let steps = (vertical_amount.round() / 15.).abs() as u32;
                    for _ in 0..steps {
                        if is_next {
                            self.workspace_manager.activate_next();
                        } else {
                            self.workspace_manager.activate_previous();
                        }
                    }
                }
            }
            if evt.source() == AxisSource::Finger {
                if evt.amount(Axis::Horizontal) == Some(0.0) {
                    frame = frame.stop(Axis::Horizontal);
                }
                if evt.amount(Axis::Vertical) == Some(0.0) {
                    frame = frame.stop(Axis::Vertical);
                }
            }
            let pointer = self.pointer.clone();
            pointer.axis(self, frame);
            pointer.frame(self);
        }
    }

    pub fn output_under_pointer(&self) -> Option<Output> {
        let pos = self.pointer.current_location().to_i32_round();
        let workspace = self.workspace_manager.active_workspace();
        workspace
            .outputs()
            .find(|o| workspace.output_geometry(o).unwrap().contains(pos))
            .cloned()
    }

    fn process_keyboard_shortcut(&self, modifiers: ModifiersState, keysym: Keysym) -> Option<KeyAction> {
        #[inline]
        fn is_tab(keysym: Keysym) -> bool {
            keysym == Keysym::Tab || keysym == Keysym::ISO_Left_Tab || keysym == Keysym::KP_Tab
        }

        if modifiers.ctrl && modifiers.alt && keysym == Keysym::BackSpace || modifiers.logo && keysym == Keysym::q {
            // ctrl+alt+backspace = quit
            // logo + q = quit
            Some(KeyAction::Quit)
        } else if (xkb::KEY_XF86Switch_VT_1..=xkb::KEY_XF86Switch_VT_12).contains(&keysym.raw()) {
            // VTSwitch
            Some(KeyAction::VtSwitch((keysym.raw() - xkb::KEY_XF86Switch_VT_1 + 1) as i32))
        } else if modifiers.logo && keysym == Keysym::Return {
            // run terminal
            Some(KeyAction::Run("xfce4-terminal".into(), vec!["--disable-server".into()]))
        } else if modifiers.logo && (xkb::KEY_1..=xkb::KEY_9).contains(&keysym.raw()) {
            Some(KeyAction::Screen((keysym.raw() - xkb::KEY_1) as usize))
        } else if modifiers.logo && modifiers.shift && keysym == Keysym::M {
            Some(KeyAction::ScaleDown)
        } else if modifiers.logo && modifiers.shift && keysym == Keysym::P {
            Some(KeyAction::ScaleUp)
        } else if modifiers.logo && modifiers.shift && keysym == Keysym::R {
            Some(KeyAction::RotateOutput)
        } else if modifiers.logo && modifiers.shift && keysym == Keysym::T {
            Some(KeyAction::ToggleTint)
        } else if modifiers.logo && modifiers.shift && keysym == Keysym::D {
            Some(KeyAction::ToggleDecorations)
        } else if modifiers.alt && modifiers.ctrl && keysym == Keysym::Up {
            Some(KeyAction::WorkspaceUp)
        } else if modifiers.alt && modifiers.ctrl && keysym == Keysym::Down {
            Some(KeyAction::WorkspaceDown)
        } else if modifiers.alt && modifiers.ctrl && keysym == Keysym::Left {
            Some(KeyAction::WorkspaceLeft)
        } else if modifiers.alt && modifiers.ctrl && keysym == Keysym::Right {
            Some(KeyAction::WorkspaceRight)
        } else if modifiers.alt && modifiers.shift && is_tab(keysym) {
            if !self.cycling_windows {
                Some(KeyAction::StartCycleWindowsReverse)
            } else {
                Some(KeyAction::CycleWindowsPrevious)
            }
        } else if modifiers.alt && is_tab(keysym) {
            if !self.cycling_windows {
                Some(KeyAction::StartCycleWindowsForward)
            } else {
                Some(KeyAction::CycleWindowsNext)
            }
        } else if self.cycling_windows && keysym == Keysym::Escape {
            Some(KeyAction::CancelCycleWindows)
        } else {
            None
        }
    }
}

#[cfg(any(feature = "winit", feature = "x11"))]
impl<BackendData: Backend> Xfwl4State<BackendData> {
    pub fn process_input_event_windowed<B: InputBackend>(&mut self, event: InputEvent<B>, output_name: &str) {
        if !matches!(
            event,
            InputEvent::DeviceAdded { .. } | InputEvent::DeviceRemoved { .. } | InputEvent::Special(_)
        ) {
            self.ext_idle_notifier_state.notify_activity(&self.seat);
        }

        match event {
            InputEvent::Keyboard { event } => match self.keyboard_key_to_action::<B>(event) {
                KeyAction::ScaleUp => {
                    let output = self
                        .workspace_manager
                        .active_workspace()
                        .outputs()
                        .find(|o| o.name() == output_name)
                        .unwrap()
                        .clone();

                    let current_scale = output.current_scale().fractional_scale();
                    let new_scale = current_scale + 0.25;
                    output.change_current_state(None, None, Some(Scale::Fractional(new_scale)), None);

                    self.output_changed(&output);
                }

                KeyAction::ScaleDown => {
                    let output = self
                        .workspace_manager
                        .active_workspace()
                        .outputs()
                        .find(|o| o.name() == output_name)
                        .unwrap()
                        .clone();

                    let current_scale = output.current_scale().fractional_scale();
                    let new_scale = f64::max(1.0, current_scale - 0.25);
                    output.change_current_state(None, None, Some(Scale::Fractional(new_scale)), None);

                    self.output_changed(&output);
                }

                KeyAction::RotateOutput => {
                    let output = self
                        .workspace_manager
                        .active_workspace()
                        .outputs()
                        .find(|o| o.name() == output_name)
                        .unwrap()
                        .clone();

                    let current_transform = output.current_transform();
                    let new_transform = match current_transform {
                        Transform::Normal => Transform::_90,
                        Transform::_90 => Transform::_180,
                        Transform::_180 => Transform::_270,
                        Transform::_270 => Transform::Flipped,
                        Transform::Flipped => Transform::Flipped90,
                        Transform::Flipped90 => Transform::Flipped180,
                        Transform::Flipped180 => Transform::Flipped270,
                        Transform::Flipped270 => Transform::Normal,
                    };
                    tracing::info!(?current_transform, ?new_transform, output = ?output.name(), "changing output transform");
                    output.change_current_state(None, Some(new_transform), None, None);

                    self.output_changed(&output);
                }

                action => match action {
                    KeyAction::None | KeyAction::Quit | KeyAction::Run(_, _) | KeyAction::ToggleDecorations => {
                        self.process_common_key_action(action)
                    }

                    _ => tracing::warn!(?action, output_name, "Key action unsupported on on output backend.",),
                },
            },

            InputEvent::PointerMotionAbsolute { event } => {
                let output = self
                    .workspace_manager
                    .active_workspace()
                    .outputs()
                    .find(|o| o.name() == output_name)
                    .unwrap()
                    .clone();
                self.on_pointer_move_absolute_windowed::<B>(event, &output)
            }
            InputEvent::PointerButton { event } => self.on_pointer_button::<B>(event),
            InputEvent::PointerAxis { event } => self.on_pointer_axis::<B>(event),
            _ => (), // other events are not handled in xfwl4 (yet)
        }
    }

    fn on_pointer_move_absolute_windowed<B: InputBackend>(&mut self, evt: B::PointerMotionAbsoluteEvent, output: &Output) {
        let workspace = self.workspace_manager.active_workspace();
        let output_geo = workspace.output_geometry(output).unwrap();

        let pos = evt.position_transformed(output_geo.size) + output_geo.loc.to_f64();
        let serial = SERIAL_COUNTER.next_serial();

        let pointer = self.pointer.clone();
        let under = self.surface_under(pos);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: pos,
                serial,
                time: evt.time_msec(),
            },
        );
        pointer.frame(self);
    }

    pub fn release_all_keys(&mut self) {
        let keyboard = self.seat.get_keyboard().unwrap();
        for keycode in keyboard.pressed_keys() {
            keyboard.input(self, keycode, KeyState::Released, SERIAL_COUNTER.next_serial(), 0, |_, _, _| {
                FilterResult::Forward::<bool>
            });
        }
    }
}

/// Possible results of a keyboard action
#[allow(dead_code)] // some of these are only read if udev is enabled
#[derive(Debug)]
pub enum KeyAction {
    /// Quit the compositor
    Quit,
    /// Trigger a vt-switch
    VtSwitch(i32),
    /// run a command
    Run(String, Vec<String>),
    /// Switch the current screen
    Screen(usize),
    ScaleUp,
    ScaleDown,
    RotateOutput,
    ToggleTint,
    ToggleDecorations,
    WorkspaceUp,
    WorkspaceDown,
    WorkspaceLeft,
    WorkspaceRight,
    StartCycleWindowsForward,
    StartCycleWindowsReverse,
    CycleWindowsNext,
    CycleWindowsPrevious,
    FinishCycleWindows,
    CancelCycleWindows,
    /// Do nothing more
    None,
}

impl<BackendData: Backend> VirtualKeyboardHandler for Xfwl4State<BackendData> {
    fn on_keyboard_event(&mut self, keycode: Keycode, state: KeyState, time: u32, keyboard: KeyboardHandle<Self>) {
        let serial = SERIAL_COUNTER.next_serial();
        keyboard.input(self, keycode, state, serial, time, |_, _, _| FilterResult::Forward::<bool>);
    }
    fn on_keyboard_modifiers(
        &mut self,
        _depressed_mods: ModMask,
        _latched_mods: ModMask,
        _locked_mods: ModMask,
        _keyboard: KeyboardHandle<Self>,
    ) {
    }
}
