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

use std::{ffi::OsString, process::Command};

use gtk::gdk::ModifierType;
use smithay::{
    backend::input::{ButtonState, KeyState, ProximityState, TabletToolTipState, TouchSlot},
    desktop::{WindowSurfaceType, layer_map_for_output},
    input::{
        keyboard::{FilterResult, Keycode, Keysym, keysyms as xkb},
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
            GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
            GrabStartData as PointerGrabStartData, MotionEvent, RelativeMotionEvent,
        },
        touch::{DownEvent, UpEvent},
    },
    reexports::wayland_server::protocol::wl_pointer,
    utils::{Logical, Point, SERIAL_COUNTER, Serial, Size},
    wayland::{
        input_method::InputMethodSeat,
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat,
        pointer_constraints::{PointerConstraint, with_pointer_constraint},
        seat::WaylandFocus,
        shell::wlr_layer::{KeyboardInteractivity, Layer as WlrLayer},
        tablet_manager::TabletSeatTrait,
    },
};
use tracing::{debug, error, info};

use crate::{
    backend::{
        Backend, DeviceCapabilities, KeyboardInputEvent, PointerInputEvent, TabletInputEvent, TabletToolAxisData, TabletToolButtonData,
        TabletToolProximityData, TabletToolTipData, TouchInputEvent, TranslatedInput,
    },
    core::{
        config::{ShortcutKey, WmShortcutAction},
        focus::{KeyboardFocusTarget, PointerFocusTarget},
        shell::{GrabTrigger, ResizeEdge},
        state::{Xfwl4Core, Xfwl4State},
        ui_thread::ActionLocation,
        util::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, XkbStateGdkExt},
    },
    ui::{ToUiMessage, tabwin::TabwinConfig},
};

impl<BackendData: Backend> Xfwl4State<BackendData> {
    pub(in crate::core) fn process_common_key_action(&mut self, action: KeyAction, serial: Serial) {
        let focused_window = || {
            self.core
                .seat
                .get_keyboard()
                .and_then(|keyboard| keyboard.current_focus())
                .and_then(|focus| match focus {
                    KeyboardFocusTarget::Window(w) => self.core.workspace_manager.active_workspace().find_element(|elem| elem.0 == w),
                    _ => None,
                })
        };

        match action {
            KeyAction::None => (),

            KeyAction::Quit => {
                info!("Quitting.");
                self.shutdown();
            }

            KeyAction::VtSwitch(num) => {
                self.backend.switch_vt(num);
            }

            KeyAction::Run(argv0, args) => {
                info!("Starting program: {}", argv0.display());
                if let Err(err) = Command::new(&argv0).args(args).spawn() {
                    error!("Failed to start program {}: {err}", argv0.display());
                }
            }

            KeyAction::WmAction(WmShortcutAction::PopupMenu) => {
                if let Some(window) = focused_window() {
                    let seat = self.core.seat.clone();
                    let pointer_location = self.core.pointer.current_location().to_i32_round();
                    self.pop_up_window_menu(&window, &seat, serial, ActionLocation::Absolute(pointer_location));
                }
            }

            KeyAction::WmAction(WmShortcutAction::CloseWindow) => {
                if let Some(window) = focused_window() {
                    window.close();
                }
            }

            KeyAction::WmAction(WmShortcutAction::MaximizeHoriz) => (), // TODO
            KeyAction::WmAction(WmShortcutAction::MaximizeVert) => (),  // TODO
            KeyAction::WmAction(WmShortcutAction::MaximizeWindow) => {
                if let Some(window) = focused_window() {
                    let is_maximized = window.maximized();
                    self.set_window_maximized(&window, !is_maximized);
                }
            }

            KeyAction::WmAction(WmShortcutAction::FillWindow) => {
                if let Some(window) = focused_window() {
                    self.set_window_filled(&window);
                }
            }

            KeyAction::WmAction(WmShortcutAction::ShadeWindow) => {
                if let Some(window) = focused_window() {
                    self.set_window_shaded(&window, !window.shaded());
                }
            }

            KeyAction::WmAction(WmShortcutAction::ToggleFullscreen) => {
                if let Some(window) = focused_window() {
                    let is_fullscreen = window.fullscreened();
                    if is_fullscreen {
                        self.set_window_unfullscreen(&window);
                    } else {
                        self.set_window_fullscreen(&window, None);
                    }
                }
            }

            KeyAction::WmAction(WmShortcutAction::HideWindow) => {
                if let Some(window) = focused_window() {
                    self.set_window_minimized(&window);
                }
            }

            KeyAction::WmAction(WmShortcutAction::Move) => {
                if let Some(window) = focused_window() {
                    let seat = self.core.seat.clone();
                    self.start_window_move(window, seat, serial, GrabTrigger::Keyboard);
                }
            }

            KeyAction::WmAction(WmShortcutAction::Resize) => {
                if let Some(window) = focused_window() {
                    let seat = self.core.seat.clone();
                    self.start_window_resize(window, seat, serial, ResizeEdge::BOTTOM_RIGHT, GrabTrigger::Keyboard);
                }
            }

            KeyAction::WmAction(WmShortcutAction::ToggleAbove) => {
                if let Some(window) = focused_window() {
                    self.set_window_always_on_top(&window, !window.always_on_top());
                }
            }

            KeyAction::WmAction(WmShortcutAction::LowerWindow) => {
                if let Some(window) = focused_window() {
                    self.lower_window(&window, serial);
                }
            }

            KeyAction::WmAction(WmShortcutAction::RaiseWindow) => {
                if let Some(window) = focused_window() {
                    self.raise_window(&window, serial);
                }
            }

            KeyAction::WmAction(WmShortcutAction::RaiseLowerWindow) => {
                if let Some(window) = focused_window()
                    && let Some(workspace) = self.core.workspace_manager.workspace_for_window_mut(&window)
                {
                    let is_top = workspace.elements().last().is_some_and(|last| last == &window);
                    if is_top {
                        self.lower_window(&window, serial);
                    } else {
                        self.raise_window(&window, serial);
                    }
                }
            }

            KeyAction::WmAction(WmShortcutAction::UpWorkspace) => self.core.workspace_manager.activate_up(),
            KeyAction::WmAction(WmShortcutAction::DownWorkspace) => self.core.workspace_manager.activate_down(),
            KeyAction::WmAction(WmShortcutAction::LeftWorkspace) => self.core.workspace_manager.activate_left(),
            KeyAction::WmAction(WmShortcutAction::RightWorkspace) => self.core.workspace_manager.activate_right(),
            KeyAction::WmAction(WmShortcutAction::NextWorkspace) => self.core.workspace_manager.activate_next(),
            KeyAction::WmAction(WmShortcutAction::PrevWorkspace) => self.core.workspace_manager.activate_previous(),
            KeyAction::WmAction(WmShortcutAction::Workspace1) => self.core.workspace_manager.set_active_workspace(0),
            KeyAction::WmAction(WmShortcutAction::Workspace2) => self.core.workspace_manager.set_active_workspace(1),
            KeyAction::WmAction(WmShortcutAction::Workspace3) => self.core.workspace_manager.set_active_workspace(2),
            KeyAction::WmAction(WmShortcutAction::Workspace4) => self.core.workspace_manager.set_active_workspace(3),
            KeyAction::WmAction(WmShortcutAction::Workspace5) => self.core.workspace_manager.set_active_workspace(4),
            KeyAction::WmAction(WmShortcutAction::Workspace6) => self.core.workspace_manager.set_active_workspace(5),
            KeyAction::WmAction(WmShortcutAction::Workspace7) => self.core.workspace_manager.set_active_workspace(6),
            KeyAction::WmAction(WmShortcutAction::Workspace8) => self.core.workspace_manager.set_active_workspace(7),
            KeyAction::WmAction(WmShortcutAction::Workspace9) => self.core.workspace_manager.set_active_workspace(8),
            KeyAction::WmAction(WmShortcutAction::Workspace10) => self.core.workspace_manager.set_active_workspace(9),
            KeyAction::WmAction(WmShortcutAction::Workspace11) => self.core.workspace_manager.set_active_workspace(10),
            KeyAction::WmAction(WmShortcutAction::Workspace12) => self.core.workspace_manager.set_active_workspace(11),
            KeyAction::WmAction(WmShortcutAction::AddWorkspace) => self.core.workspace_manager.add_workspace(),
            KeyAction::WmAction(WmShortcutAction::AddAdjacentWorkspace) => {
                let cur_num = self.core.workspace_manager.active_workspace_index();
                self.core.workspace_manager.insert_workspace(cur_num + 1);
            }
            KeyAction::WmAction(WmShortcutAction::DelWorkspace) => {
                let n_workspaces = self.core.workspace_manager.workspaces().len() as u32;
                self.core.workspace_manager.remove_workspace(n_workspaces - 1);
            }
            KeyAction::WmAction(WmShortcutAction::DelActiveWorkspace) => {
                let cur_num = self.core.workspace_manager.active_workspace_index();
                self.core.workspace_manager.remove_workspace(cur_num);
            }
            KeyAction::WmAction(WmShortcutAction::MoveUpWorkspace) => {
                if let Some(window) = focused_window()
                    && let Some(new_index) = self.core.workspace_manager.move_window_up(&window)
                {
                    self.core.workspace_manager.set_active_workspace(new_index);
                }
            }
            KeyAction::WmAction(WmShortcutAction::MoveDownWorkspace) => {
                if let Some(window) = focused_window()
                    && let Some(new_index) = self.core.workspace_manager.move_window_down(&window)
                {
                    self.core.workspace_manager.set_active_workspace(new_index);
                }
            }
            KeyAction::WmAction(WmShortcutAction::MoveLeftWorkspace) => {
                if let Some(window) = focused_window()
                    && let Some(new_index) = self.core.workspace_manager.move_window_left(&window)
                {
                    self.core.workspace_manager.set_active_workspace(new_index);
                }
            }
            KeyAction::WmAction(WmShortcutAction::MoveRightWorkspace) => {
                if let Some(window) = focused_window()
                    && let Some(new_index) = self.core.workspace_manager.move_window_right(&window)
                {
                    self.core.workspace_manager.set_active_workspace(new_index);
                }
            }
            KeyAction::WmAction(WmShortcutAction::MovePrevWorkspace) => {
                if let Some(window) = focused_window()
                    && let Some(new_index) = self.core.workspace_manager.move_window_previous(&window)
                {
                    self.core.workspace_manager.set_active_workspace(new_index);
                }
            }
            KeyAction::WmAction(WmShortcutAction::MoveNextWorkspace) => {
                if let Some(window) = focused_window()
                    && let Some(new_index) = self.core.workspace_manager.move_window_next(&window)
                {
                    self.core.workspace_manager.set_active_workspace(new_index);
                }
            }
            KeyAction::WmAction(WmShortcutAction::MoveWorkspace1) => {
                if let Some(window) = focused_window()
                    && self.core.workspace_manager.move_window_to(&window, 0)
                {
                    self.core.workspace_manager.set_active_workspace(0);
                }
            }
            KeyAction::WmAction(WmShortcutAction::MoveWorkspace2) => {
                if let Some(window) = focused_window()
                    && self.core.workspace_manager.move_window_to(&window, 1)
                {
                    self.core.workspace_manager.set_active_workspace(1);
                }
            }
            KeyAction::WmAction(WmShortcutAction::MoveWorkspace3) => {
                if let Some(window) = focused_window()
                    && self.core.workspace_manager.move_window_to(&window, 2)
                {
                    self.core.workspace_manager.set_active_workspace(2);
                }
            }
            KeyAction::WmAction(WmShortcutAction::MoveWorkspace4) => {
                if let Some(window) = focused_window()
                    && self.core.workspace_manager.move_window_to(&window, 3)
                {
                    self.core.workspace_manager.set_active_workspace(3);
                }
            }
            KeyAction::WmAction(WmShortcutAction::MoveWorkspace5) => {
                if let Some(window) = focused_window()
                    && self.core.workspace_manager.move_window_to(&window, 4)
                {
                    self.core.workspace_manager.set_active_workspace(4);
                }
            }
            KeyAction::WmAction(WmShortcutAction::MoveWorkspace6) => {
                if let Some(window) = focused_window()
                    && self.core.workspace_manager.move_window_to(&window, 5)
                {
                    self.core.workspace_manager.set_active_workspace(5);
                }
            }
            KeyAction::WmAction(WmShortcutAction::MoveWorkspace7) => {
                if let Some(window) = focused_window()
                    && self.core.workspace_manager.move_window_to(&window, 6)
                {
                    self.core.workspace_manager.set_active_workspace(6);
                }
            }
            KeyAction::WmAction(WmShortcutAction::MoveWorkspace8) => {
                if let Some(window) = focused_window()
                    && self.core.workspace_manager.move_window_to(&window, 7)
                {
                    self.core.workspace_manager.set_active_workspace(7);
                }
            }
            KeyAction::WmAction(WmShortcutAction::MoveWorkspace9) => {
                if let Some(window) = focused_window()
                    && self.core.workspace_manager.move_window_to(&window, 8)
                {
                    self.core.workspace_manager.set_active_workspace(8);
                }
            }
            KeyAction::WmAction(WmShortcutAction::MoveWorkspace10) => {
                if let Some(window) = focused_window()
                    && self.core.workspace_manager.move_window_to(&window, 9)
                {
                    self.core.workspace_manager.set_active_workspace(9);
                }
            }

            KeyAction::WmAction(WmShortcutAction::MoveWorkspace11) => {
                if let Some(window) = focused_window()
                    && self.core.workspace_manager.move_window_to(&window, 10)
                {
                    self.core.workspace_manager.set_active_workspace(10);
                }
            }
            KeyAction::WmAction(WmShortcutAction::MoveWorkspace12) => {
                if let Some(window) = focused_window()
                    && self.core.workspace_manager.move_window_to(&window, 11)
                {
                    self.core.workspace_manager.set_active_workspace(11);
                }
            }

            KeyAction::WmAction(action @ WmShortcutAction::CycleWindows)
            | KeyAction::WmAction(action @ WmShortcutAction::CycleReverseWindows) => {
                if let Some(output) = self.output_under_pointer() {
                    let clients = self.collect_tabwin_clients(&output);

                    let initial_selection = if action == WmShortcutAction::CycleWindows {
                        clients.get(1).or_else(|| clients.first())
                    } else {
                        clients.last()
                    }
                    .map(|client| client.id.clone());

                    if let Some(initial_selection) = initial_selection {
                        self.core.cycling_windows = true;
                        let _ = self.core.to_ui_channel_tx.send(ToUiMessage::ShowTabwin(TabwinConfig {
                            mode: self.core.config.cycle_tabwin_mode(),
                            window_opacity: (self.core.config.popup_opacity() as f64 / 100.).clamp(0., 1.),
                            cycle_preview: self.core.config.cycle_preview(),
                            clients,
                            initial_selection,
                            next_shortcut: self
                                .core
                                .shortcut_for_wm_action(WmShortcutAction::CycleWindows)
                                .unwrap_or_else(|| ShortcutKey::new(Keysym::Tab, ModifierType::MOD1_MASK)),
                            prev_shortcut: self
                                .core
                                .shortcut_for_wm_action(WmShortcutAction::CycleReverseWindows)
                                .unwrap_or_else(|| {
                                    ShortcutKey::new(Keysym::ISO_Left_Tab, ModifierType::MOD1_MASK | ModifierType::SHIFT_MASK)
                                }),
                            up_shortcut: self
                                .core
                                .shortcut_for_wm_action(WmShortcutAction::Up)
                                .unwrap_or_else(|| ShortcutKey::new(Keysym::Up, ModifierType::empty())),
                            down_shortcut: self
                                .core
                                .shortcut_for_wm_action(WmShortcutAction::Down)
                                .unwrap_or_else(|| ShortcutKey::new(Keysym::Down, ModifierType::empty())),
                            left_shortcut: self
                                .core
                                .shortcut_for_wm_action(WmShortcutAction::Left)
                                .unwrap_or_else(|| ShortcutKey::new(Keysym::Left, ModifierType::empty())),
                            right_shortcut: self
                                .core
                                .shortcut_for_wm_action(WmShortcutAction::Right)
                                .unwrap_or_else(|| ShortcutKey::new(Keysym::Right, ModifierType::empty())),
                            cancel_shortcut: self
                                .core
                                .shortcut_for_wm_action(WmShortcutAction::Cancel)
                                .unwrap_or_else(|| ShortcutKey::new(Keysym::Escape, ModifierType::empty())),
                        }));
                    }
                }
            }

            KeyAction::WmAction(WmShortcutAction::FillHoriz) => (),          // TODO
            KeyAction::WmAction(WmShortcutAction::FillVert) => (),           // TODO
            KeyAction::WmAction(WmShortcutAction::ShowDesktop) => (),        // TODO
            KeyAction::WmAction(WmShortcutAction::StickWindow) => (),        // TODO
            KeyAction::WmAction(WmShortcutAction::SwitchApplication) => (),  // TODO
            KeyAction::WmAction(WmShortcutAction::SwitchWindow) => (),       // TODO
            KeyAction::WmAction(WmShortcutAction::TileDown) => (),           // TODO
            KeyAction::WmAction(WmShortcutAction::TileLeft) => (),           // TODO
            KeyAction::WmAction(WmShortcutAction::TileRight) => (),          // TODO
            KeyAction::WmAction(WmShortcutAction::TileUp) => (),             // TODO
            KeyAction::WmAction(WmShortcutAction::TileDownLeft) => (),       // TODO
            KeyAction::WmAction(WmShortcutAction::TileDownRight) => (),      // TODO
            KeyAction::WmAction(WmShortcutAction::TileUpLeft) => (),         // TODO
            KeyAction::WmAction(WmShortcutAction::TileUpRight) => (),        // TODO
            KeyAction::WmAction(WmShortcutAction::MoveToMonitorDown) => (),  // TODO
            KeyAction::WmAction(WmShortcutAction::MoveToMonitorLeft) => (),  // TODO
            KeyAction::WmAction(WmShortcutAction::MoveToMonitorRight) => (), // TODO
            KeyAction::WmAction(WmShortcutAction::MoveToMonitorUp) => (),    // TODO

            KeyAction::WmAction(
                WmShortcutAction::Cancel | WmShortcutAction::Up | WmShortcutAction::Down | WmShortcutAction::Left | WmShortcutAction::Right,
            ) => {
                // I'm pretty sure we should never get here, as up/down/left/right/cancel are
                // explicitly ignored by the keyboard shortcut handler.  These are only used in
                // special circumstances like tabwin navigation and keyboard-interactive
                // move/resize.
                tracing::debug!("Got {action:?}, which is unexpected here");
            }
        }
    }

    pub(in crate::core) fn on_keyboard_key(&mut self, keycode: u32, state: KeyState, time: u32) -> (KeyAction, Serial) {
        let keycode = Keycode::new(keycode);
        debug!(?keycode, ?state, "key");
        let serial = SERIAL_COUNTER.next_serial();
        let mut suppressed_keys = self.core.suppressed_keys.clone();
        let keyboard = self.core.seat.get_keyboard().unwrap();

        for layer in self.core.shell_protocol_delegates.layer_surfaces().rev().collect::<Vec<_>>() {
            let exclusive = layer.with_cached_state(|data| {
                data.keyboard_interactivity == KeyboardInteractivity::Exclusive
                    && (data.layer == WlrLayer::Top || data.layer == WlrLayer::Overlay)
            });
            if exclusive {
                let surface = self.core.workspace_manager.active_workspace().outputs().find_map(|o| {
                    let map = layer_map_for_output(o);
                    map.layers().find(|l| l.layer_surface() == &layer).cloned()
                });
                if let Some(surface) = surface {
                    keyboard.set_focus(self, Some(surface.into()), serial);
                    keyboard.input::<(), _>(self, keycode, state, serial, time, |_, _, _| FilterResult::Forward);
                    return (KeyAction::None, serial);
                };
            }
        }

        let inhibited = self
            .core
            .workspace_manager
            .active_workspace()
            .element_under(self.core.pointer.current_location())
            .and_then(|(window, _)| {
                let surface = window.wl_surface()?;
                self.core.seat.keyboard_shortcuts_inhibitor_for_surface(&surface)
            })
            .map(|inhibitor| inhibitor.is_active())
            .unwrap_or(false);

        let modifier_mask = keyboard.with_xkb_state(self, |ctx| {
            let xkb = ctx.xkb().lock().unwrap();
            // SAFETY: I won't hold this reference longer than the Xkb instance above.
            let state = unsafe { xkb.state() };

            state.gdk_modifier_mask()
        });

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
                        let action = data.process_keyboard_shortcut(modifier_mask, keysym);

                        if action.is_some() {
                            suppressed_keys.push(keysym);
                        }

                        action.map(FilterResult::Intercept).unwrap_or(FilterResult::Forward)
                    } else {
                        FilterResult::Forward
                    }
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

        self.core.suppressed_keys = suppressed_keys;
        (action, serial)
    }

    pub(in crate::core) fn on_pointer_motion_relative(
        &mut self,
        delta: Point<f64, Logical>,
        delta_unaccel: Point<f64, Logical>,
        utime: u64,
    ) {
        let mut pointer_location = self.core.pointer.current_location();
        let serial = SERIAL_COUNTER.next_serial();

        let pointer = self.core.pointer.clone();
        let under = self.surface_under(pointer_location);

        let mut pointer_locked = false;
        let mut pointer_confined = false;
        let mut confine_region = None;
        if let Some((surface, surface_loc)) = under.as_ref().and_then(|(target, l)| Some((target.wl_surface()?, l))) {
            with_pointer_constraint(&surface, &pointer, |constraint| match constraint {
                Some(constraint) if constraint.is_active() => {
                    // Constraint does not apply if not within region
                    if !constraint
                        .region()
                        .is_none_or(|x| x.contains((pointer_location - *surface_loc).to_i32_round()))
                    {
                        return;
                    }
                    match &*constraint {
                        PointerConstraint::Locked(_locked) => {
                            pointer_locked = true;
                        }
                        PointerConstraint::Confined(confine) => {
                            pointer_confined = true;
                            confine_region = confine.region().cloned();
                        }
                    }
                }
                _ => {}
            });
        }

        pointer.relative_motion(
            self,
            under.clone(),
            &RelativeMotionEvent {
                delta,
                delta_unaccel,
                utime,
            },
        );

        // If pointer is locked, only emit relative motion
        if pointer_locked {
            pointer.frame(self);
            return;
        }

        pointer_location += delta;

        pointer_location = self.clamp_coords(pointer_location);

        let new_under = self.surface_under(pointer_location);

        // If confined, don't move pointer if it would go outside surface or region
        if pointer_confined && let Some((surface, surface_loc)) = &under {
            if new_under.as_ref().and_then(|(under, _)| under.wl_surface()) != surface.wl_surface() {
                pointer.frame(self);
                return;
            }
            if let Some(region) = confine_region
                && !region.contains((pointer_location - *surface_loc).to_i32_round())
            {
                pointer.frame(self);
                return;
            }
        }

        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: pointer_location,
                serial,
                time: (utime / 1000) as u32,
            },
        );
        pointer.frame(self);

        // If pointer is now in a constraint region, activate it
        // TODO Anywhere else pointer is moved needs to do this
        if let Some((under, surface_location)) = new_under.and_then(|(target, loc)| Some((target.wl_surface()?.into_owned(), loc))) {
            with_pointer_constraint(&under, &pointer, |constraint| match constraint {
                Some(constraint) if !constraint.is_active() => {
                    let point = (pointer_location - surface_location).to_i32_round();
                    if constraint.region().is_none_or(|region| region.contains(point)) {
                        constraint.activate();
                    }
                }
                _ => {}
            });
        }
    }

    pub(in crate::core) fn on_pointer_motion_absolute(&mut self, position: Point<f64, Logical>, time: u32) {
        let serial = SERIAL_COUNTER.next_serial();
        let workspace = self.core.workspace_manager.active_workspace();
        let max_x = workspace
            .outputs()
            .fold(0, |acc, o| acc + workspace.output_geometry(o).unwrap().size.w);
        let max_y = workspace
            .outputs()
            .max_by_key(|o| workspace.output_geometry(o).unwrap().size.h)
            .map(|o| workspace.output_geometry(o).unwrap().size.h);
        if let Some(max_y) = max_y {
            let mut pos: Point<f64, Logical> = (position.x * max_x as f64, position.y * max_y as f64).into();
            pos = self.clamp_coords(pos);
            let pointer = self.core.pointer.clone();
            let under = self.surface_under(pos);
            pointer.motion(
                self,
                under,
                &MotionEvent {
                    location: pos,
                    serial,
                    time,
                },
            );
            pointer.frame(self);
        }
    }

    pub(in crate::core) fn on_pointer_button(&mut self, button: u32, state: ButtonState, time: u32) {
        let serial = SERIAL_COUNTER.next_serial();

        let location = self.core.pointer.current_location();
        let (target, window) = self
            .surface_under(location)
            .and_then(|(target, _)| self.window_for_pointer_focus_target(&target).map(|window| (target, window)))
            .unzip();

        if state == ButtonState::Pressed {
            if self.core.config.raise_on_click()
                && (button == BTN_LEFT || self.core.config.raise_with_any_button())
                && let Some(window) = &window
            {
                self.core.workspace_manager.active_workspace_mut().raise_window(window, false);
            }

            if self.core.config.click_to_focus() {
                self.update_keyboard_focus(self.core.pointer.current_location(), serial);
            }
        }

        let swallow_event = if state == ButtonState::Pressed
            && self.easy_key_pressed()
            && let Some(target) = target
            && let Some(window) = window
        {
            if button == BTN_LEFT {
                let start_data = PointerGrabStartData {
                    focus: Some((target, location)),
                    button,
                    location,
                };
                self.start_maybe_window_move(window, self.core.seat.clone(), serial, GrabTrigger::Pointer, Some(start_data));
                true
            } else if button == BTN_RIGHT {
                let start_data = PointerGrabStartData {
                    focus: Some((target, location)),
                    button,
                    location,
                };

                let edges = self
                    .core
                    .workspace_manager
                    .active_workspace()
                    .element_geometry(&window)
                    .map(|geom| {
                        let location = location.to_i32_round::<i32>() - geom.loc;
                        let corner_size = Size::<_, Logical>::from(((geom.size.w / 3).max(50), (geom.size.h / 3).max(50)));
                        let x_dist = geom.size.w / 2 - (geom.size.w / 2 - location.x).abs();
                        let y_dist = geom.size.h / 2 - (geom.size.h / 2 - location.y).abs();

                        if x_dist < corner_size.w && y_dist < corner_size.h {
                            if location.x < geom.size.w / 2 {
                                if location.y < geom.size.h / 2 {
                                    ResizeEdge::TOP_LEFT
                                } else {
                                    ResizeEdge::BOTTOM_LEFT
                                }
                            } else if location.y < geom.size.h / 2 {
                                ResizeEdge::TOP_RIGHT
                            } else {
                                ResizeEdge::BOTTOM_RIGHT
                            }
                        } else if x_dist / corner_size.w < y_dist / corner_size.h {
                            if location.x < geom.size.w / 2 {
                                ResizeEdge::LEFT
                            } else {
                                ResizeEdge::RIGHT
                            }
                        } else if location.y < geom.size.h / 2 {
                            ResizeEdge::TOP
                        } else {
                            ResizeEdge::BOTTOM
                        }
                    })
                    .unwrap_or(ResizeEdge::TOP);

                self.start_maybe_window_resize(
                    window,
                    self.core.seat.clone(),
                    serial,
                    edges,
                    GrabTrigger::Pointer,
                    Some(start_data),
                );

                true
            } else if button == BTN_MIDDLE {
                self.lower_window(&window, serial);
                true
            } else {
                false
            }
        } else {
            false
        };

        if !swallow_event {
            let pointer = self.core.pointer.clone();
            pointer.button(
                self,
                &ButtonEvent {
                    button,
                    state: wl_pointer::ButtonState::from(state).try_into().unwrap(),
                    serial,
                    time,
                },
            );
            pointer.frame(self);
        }
    }

    fn easy_key_pressed(&mut self) -> bool {
        if let Some(keyboard) = self.core.seat.get_keyboard() {
            let easy_key = self.core.config.easy_click();
            let modifier_mask = keyboard.with_xkb_state(self, |ctx| {
                let xkb = ctx.xkb().lock().unwrap();
                // SAFETY: 'state' won't live longer than 'xkb'.
                let state = unsafe { xkb.state() };
                state.gdk_modifier_mask()
            }) & !ModifierType::LOCK_MASK;

            modifier_mask == easy_key.modifier_mask()
        } else {
            false
        }
    }

    pub(in crate::core) fn on_pointer_axis(&mut self, frame: AxisFrame) {
        let vertical_amount = frame.axis.1;
        if self.easy_key_pressed() {
            self.core.workspace_manager.reset_scroll_amount();

            if let Some(output) = self
                .core
                .workspace_manager
                .active_workspace()
                .output_under(self.core.pointer.current_location())
                .next()
                && let Some(zoom_state) = self.core.outputs_config.zoom_state_for_output_mut(output)
            {
                zoom_state.scrolled_for_zoom(vertical_amount);
            }
        } else {
            if let Some(output) = self
                .core
                .workspace_manager
                .active_workspace()
                .output_under(self.core.pointer.current_location())
                .next()
                && let Some(zoom_state) = self.core.outputs_config.zoom_state_for_output_mut(output)
            {
                zoom_state.reset_scroll_amount();
            }

            if vertical_amount != 0.0
                && self.core.config.scroll_workspaces()
                && self
                    .core
                    .workspace_manager
                    .active_workspace()
                    .element_under(self.core.pointer.current_location())
                    .is_none()
            {
                self.core.workspace_manager.scrolled_for_switch(vertical_amount);
            } else {
                self.core.workspace_manager.reset_scroll_amount();
            }

            let pointer = self.core.pointer.clone();
            pointer.axis(self, frame);
            pointer.frame(self);
        }
    }

    pub(in crate::core) fn on_gesture_swipe_begin(&mut self, time: u32, fingers: u32) {
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.core.pointer.clone();
        pointer.gesture_swipe_begin(self, &GestureSwipeBeginEvent { serial, time, fingers });
    }

    pub(in crate::core) fn on_gesture_swipe_update(&mut self, time: u32, delta: Point<f64, Logical>) {
        let pointer = self.core.pointer.clone();
        pointer.gesture_swipe_update(self, &GestureSwipeUpdateEvent { time, delta });
    }

    pub(in crate::core) fn on_gesture_swipe_end(&mut self, time: u32, cancelled: bool) {
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.core.pointer.clone();
        pointer.gesture_swipe_end(self, &GestureSwipeEndEvent { serial, time, cancelled });
    }

    pub(in crate::core) fn on_gesture_pinch_begin(&mut self, time: u32, fingers: u32) {
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.core.pointer.clone();
        pointer.gesture_pinch_begin(self, &GesturePinchBeginEvent { serial, time, fingers });
    }

    pub(in crate::core) fn on_gesture_pinch_update(&mut self, time: u32, delta: Point<f64, Logical>, scale: f64, rotation: f64) {
        let pointer = self.core.pointer.clone();
        pointer.gesture_pinch_update(
            self,
            &GesturePinchUpdateEvent {
                time,
                delta,
                scale,
                rotation,
            },
        );
    }

    pub(in crate::core) fn on_gesture_pinch_end(&mut self, time: u32, cancelled: bool) {
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.core.pointer.clone();
        pointer.gesture_pinch_end(self, &GesturePinchEndEvent { serial, time, cancelled });
    }

    pub(in crate::core) fn on_gesture_hold_begin(&mut self, time: u32, fingers: u32) {
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.core.pointer.clone();
        pointer.gesture_hold_begin(self, &GestureHoldBeginEvent { serial, time, fingers });
    }

    pub(in crate::core) fn on_gesture_hold_end(&mut self, time: u32, cancelled: bool) {
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.core.pointer.clone();
        pointer.gesture_hold_end(self, &GestureHoldEndEvent { serial, time, cancelled });
    }

    pub(in crate::core) fn on_touch_down(&mut self, slot: TouchSlot, position: Point<f64, Logical>, time: u32) {
        let Some(handle) = self.core.seat.get_touch() else {
            return;
        };
        let Some(touch_location) = self.touch_location_from_normalized(position) else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        self.update_keyboard_focus(touch_location, serial);
        let under = self.surface_under(touch_location);
        handle.down(
            self,
            under,
            &DownEvent {
                slot,
                location: touch_location,
                serial,
                time,
            },
        );
    }

    pub(in crate::core) fn on_touch_up(&mut self, slot: TouchSlot, time: u32) {
        let Some(handle) = self.core.seat.get_touch() else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        handle.up(self, &UpEvent { slot, serial, time })
    }

    pub(in crate::core) fn on_touch_motion(&mut self, slot: TouchSlot, position: Point<f64, Logical>, time: u32) {
        let Some(handle) = self.core.seat.get_touch() else {
            return;
        };
        let Some(touch_location) = self.touch_location_from_normalized(position) else {
            return;
        };
        let under = self.surface_under(touch_location);
        handle.motion(
            self,
            under,
            &smithay::input::touch::MotionEvent {
                slot,
                location: touch_location,
                time,
            },
        );
    }

    pub(in crate::core) fn on_touch_frame(&mut self) {
        let Some(handle) = self.core.seat.get_touch() else {
            return;
        };
        handle.frame(self);
    }

    pub(in crate::core) fn on_touch_cancel(&mut self) {
        let Some(handle) = self.core.seat.get_touch() else {
            return;
        };
        handle.cancel(self);
    }

    pub(in crate::core) fn on_tablet_tool_proximity(&mut self, data: TabletToolProximityData) {
        let TabletToolProximityData {
            descriptor,
            tablet,
            state,
            position,
            time,
        } = data;
        let dh = self.core.display_handle.clone();
        let tablet_seat = self.core.seat.tablet_seat();

        if let Some(pointer_location) = self.touch_location_from_normalized(position) {
            tablet_seat.add_tool::<Self>(self, &dh, &descriptor);

            let pointer = self.core.pointer.clone();
            let under = self.surface_under(pointer_location);
            let tablet_seat = self.core.seat.tablet_seat();
            let tablet_handle = tablet_seat.get_tablet(&tablet);
            let tool_handle = tablet_seat.get_tool(&descriptor);

            pointer.motion(
                self,
                under.clone(),
                &MotionEvent {
                    location: pointer_location,
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                },
            );
            pointer.frame(self);

            if let (Some(under), Some(tablet_handle), Some(tool_handle)) = (
                under.and_then(|(f, loc)| f.wl_surface().map(|s| (s.into_owned(), loc))),
                tablet_handle,
                tool_handle,
            ) {
                match state {
                    ProximityState::In => {
                        tool_handle.proximity_in(pointer_location, under, &tablet_handle, SERIAL_COUNTER.next_serial(), time)
                    }
                    ProximityState::Out => tool_handle.proximity_out(time),
                }
            }
        }
    }

    pub(in crate::core) fn on_tablet_tool_axis(&mut self, data: TabletToolAxisData) {
        let TabletToolAxisData {
            descriptor,
            tablet,
            position,
            pressure,
            distance,
            tilt,
            slider,
            rotation,
            wheel,
            time,
        } = data;
        let tablet_seat = self.core.seat.tablet_seat();

        if let Some(pointer_location) = self.touch_location_from_normalized(position) {
            let pointer = self.core.pointer.clone();
            let under = self.surface_under(pointer_location);
            let tablet_handle = tablet_seat.get_tablet(&tablet);
            let tool_handle = tablet_seat.get_tool(&descriptor);

            pointer.motion(
                self,
                under.clone(),
                &MotionEvent {
                    location: pointer_location,
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                },
            );

            if let (Some(tablet_handle), Some(tool_handle)) = (tablet_handle, tool_handle) {
                if let Some(pressure) = pressure {
                    tool_handle.pressure(pressure);
                }
                if let Some(distance) = distance {
                    tool_handle.distance(distance);
                }
                if let Some(tilt) = tilt {
                    tool_handle.tilt(tilt);
                }
                if let Some(slider) = slider {
                    tool_handle.slider_position(slider);
                }
                if let Some(rotation) = rotation {
                    tool_handle.rotation(rotation);
                }
                if let Some((delta, delta_discrete)) = wheel {
                    tool_handle.wheel(delta, delta_discrete);
                }

                tool_handle.motion(
                    pointer_location,
                    under.and_then(|(f, loc)| f.wl_surface().map(|s| (s.into_owned(), loc))),
                    &tablet_handle,
                    SERIAL_COUNTER.next_serial(),
                    time,
                );
            }

            pointer.frame(self);
        }
    }

    pub(in crate::core) fn on_tablet_tool_tip(&mut self, data: TabletToolTipData) {
        let TabletToolTipData {
            descriptor,
            position: _,
            tip_state,
            time,
        } = data;
        let tool_handle = self.core.seat.tablet_seat().get_tool(&descriptor);

        if let Some(tool_handle) = tool_handle {
            match tip_state {
                TabletToolTipState::Down => {
                    let serial = SERIAL_COUNTER.next_serial();
                    tool_handle.tip_down(serial, time);
                    // change the keyboard focus
                    self.update_keyboard_focus(self.core.pointer.current_location(), serial);
                }
                TabletToolTipState::Up => {
                    tool_handle.tip_up(time);
                }
            }
        }
    }

    pub(in crate::core) fn on_tablet_tool_button(&mut self, data: TabletToolButtonData) {
        let TabletToolButtonData {
            descriptor,
            button,
            state,
            time,
        } = data;
        let tool_handle = self.core.seat.tablet_seat().get_tool(&descriptor);

        if let Some(tool_handle) = tool_handle {
            tool_handle.button(button, state, SERIAL_COUNTER.next_serial(), time);
        }
    }

    pub(in crate::core) fn on_device_added(&mut self, caps: DeviceCapabilities) {
        if caps.has_keyboard
            && let Some(led_state) = self.core.seat.get_keyboard().map(|keyboard| keyboard.led_state())
        {
            self.backend.update_led_state(led_state);
        }
        if caps.has_touch && self.core.seat.get_touch().is_none() {
            self.core.seat.add_touch();
        }
        if let Some(tablet_descriptor) = caps.tablet_descriptor {
            self.core
                .seat
                .tablet_seat()
                .add_tablet::<Self>(&self.core.display_handle.clone(), &tablet_descriptor);
        }
    }

    pub(in crate::core) fn on_device_removed(&mut self, caps: DeviceCapabilities) {
        if let Some(tablet_descriptor) = caps.tablet_descriptor {
            let tablet_seat = self.core.seat.tablet_seat();
            tablet_seat.remove_tablet(&tablet_descriptor);
            // If there are no tablets in seat we can remove all tools
            if tablet_seat.count_tablets() == 0 {
                tablet_seat.clear_tools();
            }
        }
    }

    pub(crate) fn dispatch_translated_input(&mut self, input: TranslatedInput) -> KeyAction {
        match &input {
            TranslatedInput::DeviceAdded(_) | TranslatedInput::DeviceRemoved(_) => {}
            _ => self.core.notify_activity(&self.core.seat.clone()),
        }

        match input {
            TranslatedInput::Keyboard(KeyboardInputEvent::Key { keycode, state, time }) => {
                let (action, serial) = self.on_keyboard_key(keycode, state, time);
                self.process_common_key_action(action, serial);
                KeyAction::None
            }
            TranslatedInput::Pointer(PointerInputEvent::MotionRelative {
                delta,
                delta_unaccel,
                utime,
            }) => {
                self.on_pointer_motion_relative(delta, delta_unaccel, utime);
                KeyAction::None
            }
            TranslatedInput::Pointer(PointerInputEvent::MotionAbsolute { position, time }) => {
                self.on_pointer_motion_absolute(position, time);
                KeyAction::None
            }
            TranslatedInput::Pointer(PointerInputEvent::Button { button, state, time }) => {
                self.on_pointer_button(button, state, time);
                KeyAction::None
            }
            TranslatedInput::Pointer(PointerInputEvent::Axis { frame }) => {
                self.on_pointer_axis(frame);
                KeyAction::None
            }
            TranslatedInput::Pointer(PointerInputEvent::GestureSwipeBegin { time, fingers }) => {
                self.on_gesture_swipe_begin(time, fingers);
                KeyAction::None
            }
            TranslatedInput::Pointer(PointerInputEvent::GestureSwipeUpdate { time, delta }) => {
                self.on_gesture_swipe_update(time, delta);
                KeyAction::None
            }
            TranslatedInput::Pointer(PointerInputEvent::GestureSwipeEnd { time, cancelled }) => {
                self.on_gesture_swipe_end(time, cancelled);
                KeyAction::None
            }
            TranslatedInput::Pointer(PointerInputEvent::GesturePinchBegin { time, fingers }) => {
                self.on_gesture_pinch_begin(time, fingers);
                KeyAction::None
            }
            TranslatedInput::Pointer(PointerInputEvent::GesturePinchUpdate {
                time,
                delta,
                scale,
                rotation,
            }) => {
                self.on_gesture_pinch_update(time, delta, scale, rotation);
                KeyAction::None
            }
            TranslatedInput::Pointer(PointerInputEvent::GesturePinchEnd { time, cancelled }) => {
                self.on_gesture_pinch_end(time, cancelled);
                KeyAction::None
            }
            TranslatedInput::Pointer(PointerInputEvent::GestureHoldBegin { time, fingers }) => {
                self.on_gesture_hold_begin(time, fingers);
                KeyAction::None
            }
            TranslatedInput::Pointer(PointerInputEvent::GestureHoldEnd { time, cancelled }) => {
                self.on_gesture_hold_end(time, cancelled);
                KeyAction::None
            }
            TranslatedInput::Touch(TouchInputEvent::Down { slot, position, time }) => {
                self.on_touch_down(slot, position, time);
                KeyAction::None
            }
            TranslatedInput::Touch(TouchInputEvent::Up { slot, time }) => {
                self.on_touch_up(slot, time);
                KeyAction::None
            }
            TranslatedInput::Touch(TouchInputEvent::Motion { slot, position, time }) => {
                self.on_touch_motion(slot, position, time);
                KeyAction::None
            }
            TranslatedInput::Touch(TouchInputEvent::Frame) => {
                self.on_touch_frame();
                KeyAction::None
            }
            TranslatedInput::Touch(TouchInputEvent::Cancel) => {
                self.on_touch_cancel();
                KeyAction::None
            }
            TranslatedInput::Tablet(TabletInputEvent::ToolProximity(data)) => {
                self.on_tablet_tool_proximity(data);
                KeyAction::None
            }
            TranslatedInput::Tablet(TabletInputEvent::ToolAxis(data)) => {
                self.on_tablet_tool_axis(data);
                KeyAction::None
            }
            TranslatedInput::Tablet(TabletInputEvent::ToolTip(data)) => {
                self.on_tablet_tool_tip(data);
                KeyAction::None
            }
            TranslatedInput::Tablet(TabletInputEvent::ToolButton(data)) => {
                self.on_tablet_tool_button(data);
                KeyAction::None
            }
            TranslatedInput::DeviceAdded(caps) => {
                self.on_device_added(caps);
                KeyAction::None
            }
            TranslatedInput::DeviceRemoved(caps) => {
                self.on_device_removed(caps);
                KeyAction::None
            }
        }
    }

    pub(in crate::core) fn update_keyboard_focus(&mut self, location: Point<f64, Logical>, serial: Serial) {
        let keyboard = self.core.seat.get_keyboard().unwrap();
        let touch = self.core.seat.get_touch();
        let input_method = self.core.seat.input_method();
        // change the keyboard focus unless the pointer or keyboard is grabbed
        // We test for any matching surface type here but always use the root
        // (in case of a window the toplevel) surface for the focus.
        // So for example if a user clicks on a subsurface or popup the toplevel
        // will receive the keyboard focus. Directly assigning the focus to the
        // matching surface leads to issues with clients dismissing popups and
        // subsurface menus (for example firefox-wayland).
        // see here for a discussion about that issue:
        // https://gitlab.freedesktop.org/wayland/wayland/-/issues/294
        if !self.core.pointer.is_grabbed()
            && (!keyboard.is_grabbed() || input_method.keyboard_grabbed())
            && !touch.map(|touch| touch.is_grabbed()).unwrap_or(false)
        {
            let workspace = self.core.workspace_manager.active_workspace_mut();
            let output = workspace.output_under(location).next().cloned();
            if let Some(output) = output.as_ref() {
                let output_geo = workspace.output_geometry(output).unwrap();
                if let Some(window) = workspace.fullscreen_window_for_output(output)
                    && let Some((_, _)) = window.surface_under(location - output_geo.loc.to_f64(), WindowSurfaceType::ALL)
                {
                    #[cfg(feature = "xwayland")]
                    if self.core.config.raise_on_focus()
                        && let Some(surface) = window.0.x11_surface()
                    {
                        self.core.xwm.as_mut().unwrap().raise_window(surface).unwrap();
                    }
                    keyboard.set_focus(self, Some(window.into()), serial);
                    return;
                }

                let layers = layer_map_for_output(output);

                if let Some(layer) = [WlrLayer::Overlay, WlrLayer::Top, WlrLayer::Bottom, WlrLayer::Background]
                    .into_iter()
                    .find_map(|wlr_layer| {
                        let layer = layers.layer_under(wlr_layer, location - output_geo.loc.to_f64())?;
                        let layer_loc = layers.layer_geometry(layer).unwrap().loc;
                        layer
                            .surface_under(location - output_geo.loc.to_f64() - layer_loc.to_f64(), WindowSurfaceType::POPUP)
                            .map(|_| layer)
                    })
                {
                    keyboard.set_focus(self, Some(layer.clone().into()), serial);
                    return;
                }

                if let Some(layer) = layers
                    .layer_under(WlrLayer::Overlay, location - output_geo.loc.to_f64())
                    .or_else(|| layers.layer_under(WlrLayer::Top, location - output_geo.loc.to_f64()))
                    && layer.can_receive_keyboard_focus()
                    && let Some((_, _)) = layer.surface_under(
                        location - output_geo.loc.to_f64() - layers.layer_geometry(layer).unwrap().loc.to_f64(),
                        WindowSurfaceType::TOPLEVEL,
                    )
                {
                    keyboard.set_focus(self, Some(layer.clone().into()), serial);
                    return;
                }
            }

            if let Some((window, _)) = workspace.element_under(location).map(|(w, p)| (w.clone(), p)) {
                if self.core.config.raise_on_focus() {
                    workspace.raise_element(&window, true);
                } else {
                    workspace.activate_window(&window);
                }
                #[cfg(feature = "xwayland")]
                if self.core.config.raise_on_focus()
                    && let Some(surface) = window.0.x11_surface()
                {
                    self.core.xwm.as_mut().unwrap().raise_window(surface).unwrap();
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
                        WindowSurfaceType::TOPLEVEL,
                    )
                {
                    keyboard.set_focus(self, Some(layer.clone().into()), serial);
                }
            };
        }
    }

    pub(in crate::core) fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(PointerFocusTarget, Point<f64, Logical>)> {
        let workspace = self.core.workspace_manager.active_workspace();
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
        } else if let Some(focus) = [WlrLayer::Overlay, WlrLayer::Top, WlrLayer::Bottom, WlrLayer::Background]
            .into_iter()
            .find_map(|wlr_layer| {
                let layer = layers.layer_under(wlr_layer, pos - output_geo.loc.to_f64())?;
                let layer_loc = layers.layer_geometry(layer).unwrap().loc;
                layer
                    .surface_under(pos - output_geo.loc.to_f64() - layer_loc.to_f64(), WindowSurfaceType::POPUP)
                    .map(|(surface, loc)| (PointerFocusTarget::from(surface), loc + layer_loc + output_geo.loc))
            })
        {
            under = Some(focus)
        } else if let Some(focus) = layers
            .layer_under(WlrLayer::Overlay, pos - output_geo.loc.to_f64())
            .or_else(|| layers.layer_under(WlrLayer::Top, pos - output_geo.loc.to_f64()))
            .and_then(|layer| {
                let layer_loc = layers.layer_geometry(layer).unwrap().loc;
                layer
                    .surface_under(pos - output_geo.loc.to_f64() - layer_loc.to_f64(), WindowSurfaceType::TOPLEVEL)
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
                    .surface_under(pos - output_geo.loc.to_f64() - layer_loc.to_f64(), WindowSurfaceType::TOPLEVEL)
                    .map(|(surface, loc)| (PointerFocusTarget::from(surface), loc + layer_loc + output_geo.loc))
            })
        {
            under = Some(focus)
        };
        under.map(|(s, l)| (s, l.to_f64()))
    }

    pub(in crate::core) fn output_under_pointer(&self) -> Option<smithay::output::Output> {
        let pos = self.core.pointer.current_location().to_i32_round();
        let workspace = self.core.workspace_manager.active_workspace();
        workspace
            .outputs()
            .find(|o| workspace.output_geometry(o).unwrap().contains(pos))
            .cloned()
    }

    pub(crate) fn release_all_keys(&mut self) {
        let keyboard = self.core.seat.get_keyboard().unwrap();
        for keycode in keyboard.pressed_keys() {
            keyboard.input(self, keycode, KeyState::Released, SERIAL_COUNTER.next_serial(), 0, |_, _, _| {
                FilterResult::Forward::<bool>
            });
        }
    }

    fn touch_location_from_normalized(&self, position: Point<f64, Logical>) -> Option<Point<f64, Logical>> {
        let workspace = self.core.workspace_manager.active_workspace();
        let output = workspace
            .outputs()
            .find(|output| output.name().starts_with("eDP"))
            .or_else(|| workspace.outputs().next())?;
        let output_geometry = workspace.output_geometry(output)?;
        let transform = output.current_transform();
        let size = transform.invert().transform_size(output_geometry.size);
        let scaled = Point::<f64, Logical>::from((position.x * size.w as f64, position.y * size.h as f64));
        Some(transform.transform_point_in(scaled, &size.to_f64()) + output_geometry.loc.to_f64())
    }

    fn clamp_coords(&self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        let workspace = self.core.workspace_manager.active_workspace();
        if workspace.outputs().next().is_none() {
            return pos;
        }

        let (pos_x, pos_y) = pos.into();
        let max_x = workspace
            .outputs()
            .fold(0, |acc, o| acc + workspace.output_geometry(o).unwrap().size.w);
        let clamped_x = pos_x.clamp(0.0, max_x as f64);
        let max_y = workspace
            .outputs()
            .find(|o| {
                let geo = workspace.output_geometry(o).unwrap();
                geo.contains((clamped_x as i32, 0))
            })
            .map(|o| workspace.output_geometry(o).unwrap().size.h);

        if let Some(max_y) = max_y {
            let clamped_y = pos_y.clamp(0.0, max_y as f64);
            (clamped_x, clamped_y).into()
        } else {
            (clamped_x, pos_y).into()
        }
    }

    fn process_keyboard_shortcut(&self, modifier_mask: ModifierType, keysym: Keysym) -> Option<KeyAction> {
        // We ignore some modifiers when matching shortcuts.
        let modifier_mask = modifier_mask & !(ModifierType::LOCK_MASK | ModifierType::MOD4_MASK);

        if modifier_mask == (ModifierType::CONTROL_MASK | ModifierType::MOD1_MASK) && keysym == Keysym::BackSpace {
            Some(KeyAction::Quit)
        } else if (xkb::KEY_XF86Switch_VT_1..=xkb::KEY_XF86Switch_VT_12).contains(&keysym.raw()) {
            Some(KeyAction::VtSwitch((keysym.raw() - xkb::KEY_XF86Switch_VT_1 + 1) as i32))
        } else if !self.core.cycling_windows {
            let key = ShortcutKey::new(keysym, modifier_mask);

            #[allow(clippy::manual_map)]
            if let Some(action) = self.core.wm_shortcuts.find(&key) {
                tracing::debug!("got WM action: {action:?}");
                match action {
                    WmShortcutAction::Up
                    | WmShortcutAction::Down
                    | WmShortcutAction::Left
                    | WmShortcutAction::Right
                    | WmShortcutAction::Cancel => {
                        // These actions are only handled if the compositor is in a
                        // particular state, like the tabwin is up, or a
                        // keyboard-interactive resize or move is active.  Otherwise,
                        // these keys need to be passed to the focused client.
                        None
                    }
                    _ => Some(KeyAction::WmAction(action)),
                }
            } else if let Some(command) = self.core.command_shortcuts.find(&key) {
                Some(KeyAction::Run(command.argv0.clone(), command.args.clone()))
            } else {
                None
            }
        } else {
            // We don't handle any other keybindings when cycling windows; the tabwin itself
            // handles all events.
            None
        }
    }
}

impl<BackendData: Backend + 'static> Xfwl4Core<BackendData> {
    fn shortcut_for_wm_action(&self, action: WmShortcutAction) -> Option<ShortcutKey> {
        self.wm_shortcuts.find_by_action(&action)
    }
}

/// Possible results of a keyboard action
#[derive(Debug)]
pub enum KeyAction {
    Quit,
    VtSwitch(i32),
    Run(OsString, Vec<OsString>),
    WmAction(WmShortcutAction),
    None,
}
