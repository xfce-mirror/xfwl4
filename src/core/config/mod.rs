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

mod keyboard_config;
mod keyboard_shortcuts;
mod output_config;
mod pointer_config;
mod xfwl4_config;
mod xfwl4_config_types;

pub use keyboard_config::{DEFAULT_KEY_REPEAT_DELAY, DEFAULT_KEY_REPEAT_RATE, KeyboardConfig, XkbConfigOwned};
pub use keyboard_shortcuts::{KeyboardShortcutAction, KeyboardShortcutName};
pub use output_config::{OutputConfig, OutputConfigChange, OutputsConfig, scale_from_fractional};
pub use pointer_config::PointerConfig;
pub use xfwl4_config::Xfwl4Config;
pub use xfwl4_config_types::*;

pub const XFWM4_CHANNEL_NAME: &str = "xfwm4";
