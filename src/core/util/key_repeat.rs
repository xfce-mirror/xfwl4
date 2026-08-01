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

use std::{cell::RefCell, rc::Rc, time::Duration};

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

#[derive(Debug)]
pub struct KeyRepeat<'l, BackendData: Backend + 'static> {
    handle: LoopHandle<'l, Xfwl4State<BackendData>>,
    inner: Rc<RefCell<Inner>>,
}

#[derive(Debug)]
struct Inner {
    keycode: Option<Keycode>,
    token: Option<RegistrationToken>,
}

impl<'l, BackendData: Backend + 'static> KeyRepeat<'l, BackendData> {
    pub fn new(handle: LoopHandle<'l, Xfwl4State<BackendData>>) -> Self {
        Self {
            handle,
            inner: Rc::new(RefCell::new(Inner {
                keycode: None,
                token: None,
            })),
        }
    }

    pub fn key_press(&mut self, keyboard_config: &KeyboardConfig, keycode: Keycode, is_modifier_key: bool) {
        if keyboard_config.is_key_repeat_enabled() && !is_modifier_key {
            let inner = self.inner.borrow();
            if inner.token.is_none() || inner.keycode.is_none_or(|cur_keycode| cur_keycode != keycode) {
                drop(inner);
                self.stop();

                let inner = Rc::clone(&self.inner);
                let delay = keyboard_config.key_repeat_delay();
                let token = self
                    .handle
                    .insert_source(Timer::from_duration(delay), move |_, _, state| {
                        if let Some(keyboard) = state.core.seat.get_keyboard()
                            && keyboard.pressed_keys().contains(&keycode)
                        {
                            let serial = SERIAL_COUNTER.next_serial();
                            let time = state.core.now();
                            keyboard.input_forward(state, keycode, KeyState::Pressed, serial, time.as_millis(), false);

                            let rate = state.core.keyboard_config.key_repeat_rate();
                            let interval = (Duration::from_secs(1) / rate.max(1) as u32).max(Duration::from_millis(1));
                            TimeoutAction::ToDuration(interval)
                        } else {
                            let mut inner = inner.borrow_mut();
                            inner.keycode = None;
                            inner.token = None;
                            TimeoutAction::Drop
                        }
                    })
                    .ok();

                let mut inner = self.inner.borrow_mut();
                inner.keycode = Some(keycode);
                inner.token = token;
            }
        } else if !is_modifier_key {
            self.stop();
        }
    }

    pub fn key_release(&mut self, keycode: Keycode) {
        if self.inner.borrow().keycode.is_some_and(|cur_keycode| cur_keycode == keycode) {
            self.stop();
        }
    }

    pub fn stop(&mut self) {
        let mut inner = self.inner.borrow_mut();
        if let Some(token) = inner.token.take() {
            self.handle.remove(token);
        }
        inner.keycode = None;
    }
}

impl<'l, BackendData: Backend + 'static> Drop for KeyRepeat<'l, BackendData> {
    fn drop(&mut self) {
        self.stop();
    }
}
