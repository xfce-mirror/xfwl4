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

use smithay::{input::keyboard::XkbConfig, reexports::calloop::LoopHandle};
use xfconf::ChannelExtManual;

use crate::{
    backend::Backend,
    core::{state::Xfwl4State, util::CalloopXfconfSource},
};

const KEYBOARDS_CHANNEL_NAME: &str = "keyboards";
const KEYBOARD_LAYOUT_CHANNEL_NAME: &str = "keyboard-layout";

const PROP_KEY_REPEAT_ENABLE: &str = "/Default/KeyRepeat";
const PROP_KEY_REPEAT_DELAY: &str = "/Default/KeyRepeat/Delay";
const PROP_KEY_REPEAT_RATE: &str = "/Default/KeyRepeat/Rate";

const PROP_RESTORE_NUMLOCK_ENABLE: &str = "/Default/RestoreNumlock";
const PROP_NUMLOCK_STATE: &str = "/Default/Numlock";

const PROP_XKB_LAYOUT: &str = "/Default/XkbLayout";
const PROP_XKB_MODEL: &str = "/Default/XkbModel";
const PROP_XKB_VARIANT: &str = "/Default/XkbVariant";
const PROP_XKB_OPTIONS_ROOT: &str = "/Default/XkbOptions";

const DEFAULT_KEY_REPEAT_ENABLE: bool = true;
pub const DEFAULT_KEY_REPEAT_DELAY: i32 = 200;
pub const DEFAULT_KEY_REPEAT_RATE: i32 = 25;

#[derive(Debug)]
pub struct KeyboardConfig {
    keyboards_channel: xfconf::Channel,
    keyboard_layout_channel: xfconf::Channel,
}

impl KeyboardConfig {
    pub fn new<BackendData: Backend + 'static>(loop_handle: LoopHandle<'static, Xfwl4State<BackendData>>) -> Self {
        let keyboards_channel = xfconf::Channel::new(KEYBOARDS_CHANNEL_NAME);
        let keyboard_layout_channel = xfconf::Channel::new(KEYBOARD_LAYOUT_CHANNEL_NAME);

        let keyboards_source = CalloopXfconfSource::new(
            keyboards_channel.clone(),
            [PROP_KEY_REPEAT_ENABLE, PROP_KEY_REPEAT_DELAY, PROP_KEY_REPEAT_RATE],
        );
        loop_handle
            .insert_source(keyboards_source, |_, _, state| {
                state.apply_key_repeat_settings();
            })
            .expect("failed to insert keyboards xfconf source into event loop");

        let keyboard_layout_source = CalloopXfconfSource::new(keyboard_layout_channel.clone(), []);
        loop_handle
            .insert_source(keyboard_layout_source, |_, _, state| {
                state.apply_keyboard_layout(true);
            })
            .expect("failed to insert keyboard layout xfconf source into event loop");

        let config = Self {
            keyboards_channel,
            keyboard_layout_channel,
        };

        let restore_numlock = config.should_restore_numlock();
        loop_handle.insert_idle(move |state| {
            state.apply_key_repeat_settings();
            state.apply_keyboard_layout(restore_numlock);
        });

        config
    }

    fn is_key_repeat_enabled(&self) -> Option<bool> {
        self.keyboards_channel.get_property(PROP_KEY_REPEAT_ENABLE)
    }

    fn key_repeat_delay(&self) -> Option<i32> {
        self.keyboards_channel.get_property(PROP_KEY_REPEAT_DELAY)
    }

    fn key_repeat_rate(&self) -> Option<i32> {
        self.keyboards_channel.get_property(PROP_KEY_REPEAT_RATE)
    }

    fn should_restore_numlock(&self) -> bool {
        self.keyboards_channel.get_property(PROP_RESTORE_NUMLOCK_ENABLE).unwrap_or(false)
    }

    fn stored_numlock_state(&self) -> Option<bool> {
        self.keyboards_channel.get_property(PROP_NUMLOCK_STATE)
    }

    pub fn store_numlock_state(&self, numlock_on: bool) {
        self.keyboards_channel.set_property(PROP_NUMLOCK_STATE, numlock_on);
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    fn apply_key_repeat_settings(&self) {
        if let Some(keyboard) = self.core.seat.get_keyboard() {
            let (repeat_rate, repeat_delay) = if self
                .core
                .keyboard_config
                .is_key_repeat_enabled()
                .unwrap_or(DEFAULT_KEY_REPEAT_ENABLE)
            {
                (
                    self.core.keyboard_config.key_repeat_rate().unwrap_or(DEFAULT_KEY_REPEAT_RATE),
                    self.core.keyboard_config.key_repeat_delay().unwrap_or(DEFAULT_KEY_REPEAT_DELAY),
                )
            } else {
                (0, self.core.keyboard_config.key_repeat_delay().unwrap_or(DEFAULT_KEY_REPEAT_DELAY))
            };

            tracing::debug!("Setting keyboard repeat (delay, rate): ({repeat_delay}ms, {repeat_rate})");
            keyboard.change_repeat_info(repeat_rate, repeat_delay);
        }
    }

    fn apply_keyboard_layout(&mut self, restore_numlock: bool) {
        if let Some(keyboard) = self.core.seat.get_keyboard() {
            let model = self
                .core
                .keyboard_config
                .keyboard_layout_channel
                .get_property::<String>(PROP_XKB_MODEL);
            let layout = self
                .core
                .keyboard_config
                .keyboard_layout_channel
                .get_property::<String>(PROP_XKB_LAYOUT);
            let variant = self
                .core
                .keyboard_config
                .keyboard_layout_channel
                .get_property::<String>(PROP_XKB_VARIANT);
            let options = self
                .core
                .keyboard_config
                .keyboard_layout_channel
                .get_properties(Some(PROP_XKB_OPTIONS_ROOT))
                .into_values()
                .filter_map(|option_v| {
                    option_v
                        .transform::<String>()
                        .ok()
                        .and_then(|option_v| option_v.get::<String>().ok())
                        .filter(|v| !v.is_empty())
                })
                .collect::<Vec<String>>()
                .join(",");
            let numlock_on = self.core.keyboard_config.stored_numlock_state();

            let xkb_config = XkbConfig {
                rules: "",
                model: &model.unwrap_or("".to_owned()),
                layout: &layout.unwrap_or("".to_owned()),
                variant: &variant.unwrap_or("".to_owned()),
                options: (!options.is_empty()).then_some(options),
            };
            tracing::debug!(
                "Updating XKB config, model={}, layout={}, variant={}, options={}",
                xkb_config.model,
                xkb_config.layout,
                xkb_config.variant,
                xkb_config.options.as_ref().unwrap_or(&"".to_owned())
            );
            if let Err(err) = keyboard.set_xkb_config(self, xkb_config) {
                tracing::error!("Failed to set keyboard XKB config: {err}");
            }

            if restore_numlock && let Some(numlock_on) = numlock_on {
                let mut mods = keyboard.modifier_state();
                if mods.num_lock != numlock_on {
                    mods.num_lock = numlock_on;
                    keyboard.set_modifier_state(mods);
                    self.backend.update_led_state(keyboard.led_state());
                }

                // set_xkb_config() above will clear numlock, which will trigger us to
                // store false for the numlock state, so re-store it as whatever we're
                // setting here.
                self.core.keyboard_config.store_numlock_state(numlock_on);
            }
        }
    }
}
