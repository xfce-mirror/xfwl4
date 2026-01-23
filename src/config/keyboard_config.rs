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

use smithay::{
    input::{SeatHandler, keyboard::KeyboardHandle},
    reexports::calloop::channel::{self, Channel, Sender},
};
use xfconf::ChannelExtManual;

const KEYBOARDS_CHANNEL_NAME: &str = "keyboards";
const KEYBOARD_LAYOUT_CHANNEL_NAME: &str = "keyboard-layout";

const PROP_KEY_REPEAT_ROOT: &str = "/Default/KeyRepeat";
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
pub struct KeyboardConfig<State: SeatHandler + 'static> {
    keyboards_channel: xfconf::Channel,
    keyboard_layout_channel: xfconf::Channel,
    keyboard_handle: KeyboardHandle<State>,
    xkb_config_tx: Sender<XkbConfigOwned>,
}

impl<State: SeatHandler + 'static> Clone for KeyboardConfig<State> {
    fn clone(&self) -> Self {
        Self {
            keyboards_channel: self.keyboards_channel.clone(),
            keyboard_layout_channel: self.keyboard_layout_channel.clone(),
            keyboard_handle: self.keyboard_handle.clone(),
            xkb_config_tx: self.xkb_config_tx.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct XkbConfigOwned {
    pub model: Option<String>,
    pub layout: Option<String>,
    pub variant: Option<String>,
    pub options: Option<String>,
}

impl<State: SeatHandler + 'static> KeyboardConfig<State> {
    pub fn new(keyboard_handle: KeyboardHandle<State>) -> (Self, Channel<XkbConfigOwned>) {
        let keyboards_channel = xfconf::Channel::new(KEYBOARDS_CHANNEL_NAME);
        let keyboard_layout_channel = xfconf::Channel::new(KEYBOARD_LAYOUT_CHANNEL_NAME);

        let (xkb_config_tx, xkb_config_rx) = channel::channel();

        let config = Self {
            keyboards_channel,
            keyboard_layout_channel,
            keyboard_handle,
            xkb_config_tx,
        };

        config.keyboards_channel.connect_property_changed(Some(PROP_KEY_REPEAT_ROOT), {
            let config = config.clone();
            move |_, _, _| {
                config.handle_key_repeat_changed();
            }
        });
        config.handle_key_repeat_changed();

        config.keyboard_layout_channel.connect_property_changed(None, {
            let config = config.clone();
            move |_, _, _| {
                config.handle_keyboard_layout_changed();
            }
        });
        config.handle_keyboard_layout_changed();

        if config.should_restore_numlock()
            && let Some(numlock_on) = config.stored_numlock_state()
        {
            let mut mods = config.keyboard_handle.modifier_state();
            if mods.num_lock != numlock_on {
                mods.num_lock = numlock_on;
                config.keyboard_handle.set_modifier_state(mods);
            }
        }

        (config, xkb_config_rx)
    }

    fn handle_key_repeat_changed(&self) {
        let (repeat_rate, repeat_delay) = if self.is_key_repeat_enabled().unwrap_or(DEFAULT_KEY_REPEAT_ENABLE) {
            (
                self.key_repeat_rate().unwrap_or(DEFAULT_KEY_REPEAT_RATE),
                self.key_repeat_delay().unwrap_or(DEFAULT_KEY_REPEAT_DELAY),
            )
        } else {
            (0, self.key_repeat_delay().unwrap_or(DEFAULT_KEY_REPEAT_DELAY))
        };
        tracing::debug!("Setting keyboard repeat (delay, rate): ({repeat_delay}ms, {repeat_rate})");
        self.keyboard_handle.change_repeat_info(repeat_rate, repeat_delay);
    }

    fn handle_keyboard_layout_changed(&self) {
        let model = self.keyboard_layout_channel.get_property::<String>(PROP_XKB_MODEL);
        let layout = self.keyboard_layout_channel.get_property::<String>(PROP_XKB_LAYOUT);
        let variant = self.keyboard_layout_channel.get_property::<String>(PROP_XKB_VARIANT);
        let options = self
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

        let xkb_config = XkbConfigOwned {
            model,
            layout,
            variant,
            options: (!options.is_empty()).then_some(options),
        };

        if let Err(err) = self.xkb_config_tx.send(xkb_config) {
            tracing::error!("Failed to send new XkbConfig to state: {err}");
        }
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
