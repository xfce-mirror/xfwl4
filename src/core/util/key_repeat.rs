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
    sync::{Arc, Mutex},
    time::Duration,
};

use calloop::{
    LoopHandle, RegistrationToken,
    timer::{TimeoutAction, Timer},
};
use smithay::{backend::input::KeyState, utils::SERIAL_COUNTER};
use xkbcommon::xkb::Keycode;

use crate::{
    backend::Backend,
    core::{config::KeyboardConfig, state::Xfwl4State},
};

#[derive(Debug, Default)]
pub struct KeyRepeat {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    keycode: Option<Keycode>,
    token: Option<RegistrationToken>,
}

impl KeyRepeat {
    pub fn key_press<BackendData: Backend + 'static>(
        &mut self,
        loop_handle: &LoopHandle<'_, Xfwl4State<BackendData>>,
        keyboard_config: &KeyboardConfig,
        keycode: Keycode,
        is_modifier_key: bool,
    ) {
        if keyboard_config.is_key_repeat_enabled() && !is_modifier_key {
            let inner = self.inner.lock().unwrap();
            if inner.token.is_none() || inner.keycode.is_none_or(|cur_keycode| cur_keycode != keycode) {
                drop(inner);
                self.stop(loop_handle);

                let inner = Arc::clone(&self.inner);
                let delay = keyboard_config.key_repeat_delay();
                let token = loop_handle
                    .insert_source(Timer::from_duration(delay), move |_, _, state| {
                        if inner.lock().unwrap().keycode == Some(keycode)
                            && let Some(keyboard) = state.core.seat.get_keyboard()
                            && keyboard.pressed_keys().contains(&keycode)
                        {
                            let serial = SERIAL_COUNTER.next_serial();
                            let time = state.core.now_input();
                            keyboard.input_forward(state, keycode, KeyState::Pressed, serial, time, false);

                            let rate = state.core.keyboard_config.key_repeat_rate();
                            let interval = (Duration::from_secs(1) / rate.max(1) as u32).max(Duration::from_millis(1));
                            TimeoutAction::ToDuration(interval)
                        } else {
                            let mut inner = inner.lock().unwrap();
                            inner.keycode = None;
                            inner.token = None;
                            TimeoutAction::Drop
                        }
                    })
                    .ok();

                let mut inner = self.inner.lock().unwrap();
                inner.keycode = Some(keycode);
                inner.token = token;
            }
        } else if !is_modifier_key {
            self.stop(loop_handle);
        }
    }

    pub fn key_release<BackendData: Backend + 'static>(&mut self, loop_handle: &LoopHandle<'_, Xfwl4State<BackendData>>, keycode: Keycode) {
        let repeating = self.inner.lock().unwrap().keycode.is_some_and(|cur_keycode| cur_keycode == keycode);
        if repeating {
            self.stop(loop_handle);
        }
    }

    pub fn stop<BackendData: Backend + 'static>(&mut self, loop_handle: &LoopHandle<'_, Xfwl4State<BackendData>>) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(token) = inner.token.take() {
            loop_handle.remove(token);
        }
        inner.keycode = None;
    }
}

impl Drop for KeyRepeat {
    fn drop(&mut self) {
        // The timer can't be unregistered without a loop handle, so mark the repeat as finished;
        // it removes itself the next time it fires.
        let mut inner = self.inner.lock().unwrap();
        inner.keycode = None;
        inner.token = None;
    }
}
