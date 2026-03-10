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

use gtk::gdk;
use xkbcommon::xkb::Keysym;

use crate::core::config::{CommandShortcut, KeyboardShortcutName, ShortcutKey};

#[derive(Debug)]
pub enum UnparsedShortcut {
    Wm { accelerator: String, action: KeyboardShortcutName },
    Command { accelerator: String, command: CommandShortcut },
    WmRemoval(String),
    CommandRemoval(String),
}

#[derive(Debug)]
pub enum ParsedShortcut {
    Wm { key: ShortcutKey, action: KeyboardShortcutName },
    Command { key: ShortcutKey, command: CommandShortcut },
    WmRemoval(ShortcutKey),
    CommandRemoval(ShortcutKey),
}

pub fn parse_shortcut(accelerator: &str) -> Option<ShortcutKey> {
    let (key, modifiers) = gtk::accelerator_parse(accelerator);
    if key != 0 || !modifiers.is_empty() {
        let keysym = Keysym::new(key);
        let keysym = if keysym == Keysym::Tab && modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
            // When <Shift> is held, the keysym we get from libinput is ISO_Left_Tab, not Tab.
            Keysym::ISO_Left_Tab
        } else {
            keysym
        };

        Some(ShortcutKey::new(keysym, modifiers))
    } else {
        None
    }
}
