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

use std::collections::HashSet;

use gtk::gdk::ModifierType;
use smithay::{
    backend::input::{KeyState, TouchSlot},
    desktop::WindowSurface,
    input::{
        Seat, SeatHandler,
        keyboard::{GrabStartData as KeyboardGrabStartData, KeyboardGrab, KeyboardInnerHandle, ModifiersState},
        pointer::{
            AxisFrame, ButtonEvent, Focus, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
            GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
            GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
        },
        touch::{DownEvent, GrabStartData as TouchGrabStartData, TouchGrab},
    },
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Serial},
};
use xkbcommon::xkb::{Keycode, Keysym};

use crate::{
    backend::Backend,
    core::{
        config::WmShortcutAction,
        cycle::{CyclingPhase, TabwinGrab},
        focus::PointerFocusTarget,
        shell::WindowElement,
        state::Xfwl4State,
        util::{KeyRepeat, ScrollAccumulator, XkbStateGdkExt},
    },
    protocols::xfwl4_compositor_ui::proto::xfwl4_ui_tabwin_v1::{CloseReason, NavigateAction},
};

struct TabwinPointerGrab<BackendData: Backend + 'static> {
    start_data: PointerGrabStartData<Xfwl4State<BackendData>>,
    target: PointerFocusTarget,
    pointer_over_target: bool,
    scroller: ScrollAccumulator,
}

struct TabwinTouchGrab<BackendData: Backend + 'static> {
    start_data: TouchGrabStartData<Xfwl4State<BackendData>>,
    target: PointerFocusTarget,
    touches_down_on_target: HashSet<TouchSlot>,
    touches_on_target: HashSet<TouchSlot>,
}

struct TabwinKeyboardGrab<'l, BackendData: Backend + 'static> {
    start_data: KeyboardGrabStartData<Xfwl4State<BackendData>>,
    buffered_keystrokes: Vec<Keystroke>,
    key_repeat: KeyRepeat<'l, BackendData>,
}

struct Keystroke {
    keycode: Keycode,
    state: KeyState,
    time: u32,
    serial: Serial,
    keysym: Keysym,
    raw_keysym: Option<Keysym>,
    modifier_mask: ModifierType,
    mods_changed: bool,
}

impl<BackendData: Backend + 'static> PointerGrab<Xfwl4State<BackendData>> for TabwinPointerGrab<BackendData> {
    fn motion(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(<Xfwl4State<BackendData> as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        self.pointer_over_target = focus.as_ref().is_some_and(|(target, _)| *target == self.target);
        if !self.pointer_over_target {
            self.scroller.reset();
        }
        let tabwin_focus = focus.filter(|(target, _)| *target == self.target);
        handle.motion(data, tabwin_focus, event);
    }

    fn relative_motion(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(<Xfwl4State<BackendData> as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        self.pointer_over_target = focus.as_ref().is_some_and(|(target, _)| *target == self.target);
        let tabwin_focus = focus.filter(|(target, _)| *target == self.target);
        handle.relative_motion(data, tabwin_focus, event);
    }

    fn button(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &ButtonEvent,
    ) {
        if self.pointer_over_target {
            handle.button(data, event);
        } else {
            // Unset the pointer grab ourselves so we don't deadlock.
            data.core.cycling_state.set_grab_active(TabwinGrab::Pointer, false);
            data.finish_cycling(CloseReason::Cancelled);
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        _handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        details: AxisFrame,
    ) {
        if self.pointer_over_target {
            let steps = self.scroller.accumulate(details.axis.1);
            for _ in 0..(steps.abs()) {
                if steps > 0 {
                    data.core.compositor_ui_state.tabwin_navigate(NavigateAction::Next);
                } else {
                    data.core.compositor_ui_state.tabwin_navigate(NavigateAction::Prev);
                }
            }
        }
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn frame(&mut self, data: &mut Xfwl4State<BackendData>, handle: &mut PointerInnerHandle<'_, Xfwl4State<BackendData>>) {
        handle.frame(data);
    }

    fn unset(&mut self, data: &mut Xfwl4State<BackendData>) {
        data.core.cycling_state.set_grab_active(TabwinGrab::Pointer, false);
        data.clear_tabwin_grabs();
    }

    fn start_data(&self) -> &PointerGrabStartData<Xfwl4State<BackendData>> {
        &self.start_data
    }
}

impl<BackendData: Backend + 'static> TouchGrab<Xfwl4State<BackendData>> for TabwinTouchGrab<BackendData> {
    fn motion(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(<Xfwl4State<BackendData> as SeatHandler>::TouchFocus, Point<f64, Logical>)>,
        event: &smithay::input::touch::MotionEvent,
    ) {
        if let Some((target, location)) = focus
            && target == self.target
        {
            if !self.touches_down_on_target.contains(&event.slot) {
                self.touches_down_on_target.insert(event.slot);

                let down = DownEvent {
                    slot: event.slot,
                    location,
                    serial: SERIAL_COUNTER.next_serial(),
                    time: event.time,
                };
                handle.down(data, Some((target.clone(), location)), &down);
            }

            self.touches_on_target.insert(event.slot);
            handle.motion(data, Some((target, location)), event);
        } else {
            self.touches_on_target.remove(&event.slot);
        }
    }

    fn down(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        focus: Option<(<Xfwl4State<BackendData> as SeatHandler>::TouchFocus, Point<f64, Logical>)>,
        event: &smithay::input::touch::DownEvent,
    ) {
        if let Some((target, location)) = focus
            && target == self.target
        {
            self.touches_on_target.insert(event.slot);
            handle.down(data, Some((target, location)), event);
        }
    }

    fn up(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &smithay::input::touch::UpEvent,
    ) {
        if self.touches_down_on_target.remove(&event.slot) {
            handle.up(data, event);
        }
    }

    fn cancel(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
    ) {
        self.touches_down_on_target.clear();
        self.touches_on_target.clear();
        handle.cancel(data);
    }

    fn orientation(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &smithay::input::touch::OrientationEvent,
    ) {
        if self.touches_down_on_target.contains(&event.slot) {
            handle.orientation(data, event);
        }
    }

    fn shape(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
        event: &smithay::input::touch::ShapeEvent,
    ) {
        if self.touches_down_on_target.contains(&event.slot) {
            handle.shape(data, event);
        }
    }

    fn frame(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut smithay::input::touch::TouchInnerHandle<'_, Xfwl4State<BackendData>>,
    ) {
        handle.frame(data);
    }

    fn unset(&mut self, data: &mut Xfwl4State<BackendData>) {
        data.core.cycling_state.set_grab_active(TabwinGrab::Touch, false);
        data.clear_tabwin_grabs();
    }

    fn start_data(&self) -> &TouchGrabStartData<Xfwl4State<BackendData>> {
        &self.start_data
    }
}

impl<BackendData: Backend + 'static> KeyboardGrab<Xfwl4State<BackendData>> for TabwinKeyboardGrab<'static, BackendData> {
    fn set_focus(
        &mut self,
        _data: &mut Xfwl4State<BackendData>,
        _handle: &mut KeyboardInnerHandle<'_, Xfwl4State<BackendData>>,
        _focus: Option<<Xfwl4State<BackendData> as SeatHandler>::KeyboardFocus>,
        _serial: Serial,
    ) {
    }

    fn input(
        &mut self,
        data: &mut Xfwl4State<BackendData>,
        handle: &mut KeyboardInnerHandle<'_, Xfwl4State<BackendData>>,
        keycode: Keycode,
        state: KeyState,
        modifiers: Option<ModifiersState>,
        serial: Serial,
        time: u32,
    ) {
        let keysym_handle = handle.keysym_handle(keycode);
        // SAFETY: 'Xkb' instance outlives 'XkbState' instance.
        let modifier_mask = unsafe { keysym_handle.xkb().lock().unwrap().state().gdk_modifier_mask() };

        if data.core.cycling_state.cycling_phase() == CyclingPhase::Finishing {
            self.buffered_keystrokes.push(Keystroke {
                state,
                keycode,
                time,
                serial,
                keysym: keysym_handle.modified_sym(),
                raw_keysym: keysym_handle.raw_latin_sym_or_raw_current_sym(),
                modifier_mask,
                mods_changed: modifiers.is_some(),
            });
        } else {
            let next_key = data.core.shortcuts_state.wm_shortcut_key_for(WmShortcutAction::CycleWindows);
            let prev_key = data.core.shortcuts_state.wm_shortcut_key_for(WmShortcutAction::CycleReverseWindows);
            let next_prev_modifiers =
                next_key.map_or(ModifierType::empty(), |key| key.modifiers) | prev_key.map_or(ModifierType::empty(), |key| key.modifiers);

            match state {
                KeyState::Pressed => {
                    let keysym = keysym_handle.modified_sym();
                    let raw_keysym = keysym_handle.raw_latin_sym_or_raw_current_sym();

                    let resolve_action = |modifier_mask| {
                        data.resolve_configured_wm_shortcut_action(modifier_mask, keysym)
                            .or_else(|| raw_keysym.and_then(|keysym| data.resolve_configured_wm_shortcut_action(modifier_mask, keysym)))
                    };

                    let action = resolve_action(modifier_mask).filter(|action| {
                        matches!(
                            action,
                            WmShortcutAction::CycleWindows
                                | WmShortcutAction::CycleReverseWindows
                                | WmShortcutAction::Up
                                | WmShortcutAction::Down
                                | WmShortcutAction::Left
                                | WmShortcutAction::Right
                                | WmShortcutAction::Cancel
                        )
                    });
                    let action_without_next_prev = resolve_action(modifier_mask & !next_prev_modifiers);

                    let navigate_action = match action.or(action_without_next_prev) {
                        Some(WmShortcutAction::CycleWindows) => Some(NavigateAction::Next),
                        Some(WmShortcutAction::CycleReverseWindows) => Some(NavigateAction::Prev),
                        Some(WmShortcutAction::Up) => Some(NavigateAction::Up),
                        Some(WmShortcutAction::Down) => Some(NavigateAction::Down),
                        Some(WmShortcutAction::Left) => Some(NavigateAction::Left),
                        Some(WmShortcutAction::Right) => Some(NavigateAction::Right),
                        Some(WmShortcutAction::Cancel) => {
                            // Unset the keyboard grab ourselves so we don't deadlock.
                            data.core.cycling_state.set_grab_active(TabwinGrab::Keyboard, false);
                            data.finish_cycling(CloseReason::Cancelled);
                            handle.unset_grab(self, data, serial, true);
                            None
                        }
                        _ => None,
                    };

                    if let Some(navigate_action) = navigate_action {
                        data.core.compositor_ui_state.tabwin_navigate(navigate_action);
                    }

                    self.key_repeat
                        .key_press(&data.core.keyboard_config, keycode, keysym.is_modifier_key());
                }

                KeyState::Released => {
                    self.key_repeat.key_release(keycode);

                    let dismiss = (modifier_mask & !next_prev_modifiers) == modifier_mask;
                    if dismiss {
                        data.finish_cycling(CloseReason::Committed);
                    }
                }
            }
        }
    }

    fn unset(&mut self, data: &mut Xfwl4State<BackendData>) {
        data.core.cycling_state.set_grab_active(TabwinGrab::Keyboard, false);
        data.clear_tabwin_grabs();

        if !self.buffered_keystrokes.is_empty() {
            let buffered_keystrokes = std::mem::take(&mut self.buffered_keystrokes);
            data.core.loop_handle.insert_idle(|data| {
                let inhibited = data.shortcuts_inhibited_under_pointer();
                let has_exclusive_surface = if let Some(surface) = data.layer_surface_with_exclusive_focus() {
                    data.focus_target(surface, SERIAL_COUNTER.next_serial(), None);
                    true
                } else {
                    false
                };

                for Keystroke {
                    keycode,
                    state,
                    time,
                    serial,
                    keysym,
                    raw_keysym,
                    modifier_mask,
                    mods_changed,
                } in buffered_keystrokes
                {
                    if !has_exclusive_surface
                        && let Some(action) = data.resolve_key_action(state, keycode, keysym, raw_keysym, modifier_mask, inhibited)
                    {
                        data.process_common_key_action(action, serial);
                    } else if let Some(keyboard) = data.core.seat.get_keyboard() {
                        keyboard.input_forward(data, keycode, state, serial, time, mods_changed);
                    }
                }
            });
        }
    }

    fn start_data(&self) -> &KeyboardGrabStartData<Xfwl4State<BackendData>> {
        &self.start_data
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub(in crate::core) fn start_tabwin_keyboard_grab(&mut self, seat: Seat<Self>) {
        if !self.core.cycling_state.grab_active(TabwinGrab::Keyboard)
            && let Some(keyboard) = seat.get_keyboard()
        {
            let mut grab = TabwinKeyboardGrab {
                start_data: keyboard.grab_start_data().unwrap_or_else(|| KeyboardGrabStartData { focus: None }),
                buffered_keystrokes: Vec::new(),
                key_repeat: KeyRepeat::new(self.core.loop_handle.clone()),
            };

            if let Some((keysym, keycode)) = self.core.cycling_state.take_pending_cycle_key() {
                self.core.shortcuts_state.unsuppress_key(keysym);
                grab.key_repeat.key_press(&self.core.keyboard_config, keycode, false);
            }

            keyboard.set_grab(self, grab, SERIAL_COUNTER.next_serial());
            self.core.cycling_state.set_grab_active(TabwinGrab::Keyboard, true);
        }
    }

    pub(in crate::core) fn start_tabwin_pointer_touch_grabs(
        &mut self,
        tabwin: WindowElement,
        seat: Seat<Self>,
        tabwin_geo: Rectangle<i32, Logical>,
    ) {
        if let WindowSurface::Wayland(surface) = tabwin.0.underlying_surface() {
            if !self.core.cycling_state.grab_active(TabwinGrab::Pointer)
                && let Some(pointer) = seat.get_pointer()
            {
                let target = PointerFocusTarget::WlSurface(surface.wl_surface().clone());
                let grab = TabwinPointerGrab {
                    start_data: pointer.grab_start_data().unwrap_or_else(|| PointerGrabStartData {
                        focus: None,
                        button: 0,
                        location: pointer.current_location(),
                    }),
                    target: target.clone(),
                    pointer_over_target: true,
                    scroller: ScrollAccumulator::default(),
                };
                let serial = SERIAL_COUNTER.next_serial();
                pointer.set_grab(self, grab, serial, Focus::Clear);

                let location = pointer.current_location();
                let focus = tabwin_geo.to_f64().contains(location).then(|| (target, tabwin_geo.loc.to_f64()));
                pointer.motion(
                    self,
                    focus,
                    &MotionEvent {
                        location,
                        serial,
                        time: self.core.now().as_millis(),
                    },
                );
                pointer.frame(self);

                self.core.cycling_state.set_grab_active(TabwinGrab::Pointer, true);
            }

            if !self.core.cycling_state.grab_active(TabwinGrab::Touch)
                && let Some(touch) = seat.get_touch()
            {
                let target = PointerFocusTarget::WlSurface(surface.wl_surface().clone());
                let grab = TabwinTouchGrab {
                    start_data: touch.grab_start_data().unwrap_or_else(|| TouchGrabStartData {
                        focus: None,
                        slot: TouchSlot::from(None::<u32>),
                        location: (0., 0.).into(),
                    }),
                    target,
                    touches_down_on_target: HashSet::default(),
                    touches_on_target: HashSet::default(),
                };
                touch.set_grab(self, grab, SERIAL_COUNTER.next_serial());
                self.core.cycling_state.set_grab_active(TabwinGrab::Touch, true);
            }
        }
    }

    fn finish_cycling(&mut self, reason: CloseReason) {
        self.core.compositor_ui_state.tabwin_close(reason);
        self.clear_window_cycling_state();

        match reason {
            CloseReason::Committed => self.core.cycling_state.enter_finishing_phase(),
            CloseReason::Cancelled => self.clear_tabwin_grabs(),
        }
    }

    // Do not call from inside a grab without removing that particular grab using the "inner"
    // handle, and then setting its bool to false.  Otherwise we deadlock.
    pub(in crate::core) fn clear_tabwin_grabs(&mut self) {
        if self.core.cycling_state.take_grab_active(TabwinGrab::Pointer) {
            let serial = SERIAL_COUNTER.next_serial();
            let time = self.core.now().as_millis();
            let pointer = self.core.pointer.clone();
            pointer.unset_grab(self, serial, time);
        }

        if self.core.cycling_state.take_grab_active(TabwinGrab::Touch)
            && let Some(touch) = self.core.seat.clone().get_touch()
        {
            touch.unset_grab(self);
        }

        if self.core.cycling_state.take_grab_active(TabwinGrab::Keyboard)
            && let Some(keyboard) = self.core.seat.get_keyboard()
        {
            keyboard.unset_grab(self);
        }
    }
}
