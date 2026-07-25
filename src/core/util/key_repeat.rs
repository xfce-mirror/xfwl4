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

use std::time::Duration;

use calloop::{
    LoopHandle, RegistrationToken,
    timer::{TimeoutAction, Timer},
};
use smithay::{backend::input::KeyState, utils::SERIAL_COUNTER};
use xkbcommon::xkb::Keycode;

use crate::{backend::Backend, core::state::Xfwl4State};

#[derive(Debug, Default)]
pub struct KeyRepeat {
    keycode: Option<Keycode>,
    token: Option<RegistrationToken>,
}

impl KeyRepeat {
    pub fn start<BackendData: Backend + 'static, F>(
        &mut self,
        handle: LoopHandle<'_, Xfwl4State<BackendData>>,
        location: F,
        keycode: Keycode,
        delay: Duration,
        rate: i32,
    ) where
        F: Fn(&mut Xfwl4State<BackendData>) -> &mut KeyRepeat + 'static,
    {
        if self.token.is_none() || self.keycode.is_none_or(|cur_keycode| cur_keycode != keycode) {
            self.stop(handle.clone());

            let interval = (Duration::from_secs(1) / rate.max(1) as u32).max(Duration::from_millis(1));
            let token = handle
                .insert_source(Timer::from_duration(delay), move |_, _, state| {
                    if let Some(keyboard) = state.core.seat.get_keyboard()
                        && keyboard.pressed_keys().contains(&keycode)
                    {
                        let serial = SERIAL_COUNTER.next_serial();
                        let time = state.core.clock.now();
                        keyboard.input_forward(state, keycode, KeyState::Pressed, serial, time.as_millis(), false);
                        TimeoutAction::ToDuration(interval)
                    } else {
                        let repeat = location(state);
                        repeat.keycode = None;
                        repeat.token = None;
                        TimeoutAction::Drop
                    }
                })
                .ok();

            self.keycode = Some(keycode);
            self.token = token;
        }
    }

    pub fn stop_if_repeating_keycode<D>(&mut self, handle: LoopHandle<'_, D>, keycode: Keycode) {
        if self.keycode.is_some_and(|cur_keycode| cur_keycode == keycode) {
            self.stop(handle);
        }
    }

    pub fn stop<D>(&mut self, handle: LoopHandle<'_, D>) {
        if let Some(token) = self.token.take() {
            handle.remove(token);
        }
        self.keycode = None;
    }
}

impl Drop for KeyRepeat {
    fn drop(&mut self) {
        if self.token.is_some() {
            tracing::warn!("BUG: leaked timeout for key repeat");
        }
    }
}
