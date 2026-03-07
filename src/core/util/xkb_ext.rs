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

use std::sync::LazyLock;

use gtk::gdk;
use xkbcommon::xkb;

pub trait XkbStateGdkExt {
    fn gdk_modifier_mask(&self) -> gdk::ModifierType;
}

impl XkbStateGdkExt for xkb::State {
    fn gdk_modifier_mask(&self) -> gdk::ModifierType {
        // This is from gtk/gdk/wayland/gdkkeys-wayland.c; don't change this.
        static MAPPING: LazyLock<Vec<(&str, gdk::ModifierType)>> = LazyLock::new(|| {
            vec![
                (xkb::MOD_NAME_SHIFT, gdk::ModifierType::SHIFT_MASK),
                (xkb::MOD_NAME_CAPS, gdk::ModifierType::LOCK_MASK),
                (xkb::MOD_NAME_CTRL, gdk::ModifierType::CONTROL_MASK),
                (xkb::MOD_NAME_ALT, gdk::ModifierType::MOD1_MASK),
                (xkb::MOD_NAME_NUM, gdk::ModifierType::MOD2_MASK),
                ("Mod3", gdk::ModifierType::MOD3_MASK),
                (xkb::MOD_NAME_LOGO, gdk::ModifierType::MOD4_MASK | gdk::ModifierType::SUPER_MASK),
                ("Mod5", gdk::ModifierType::MOD5_MASK),
                ("Super", gdk::ModifierType::SUPER_MASK),
                ("Hyper", gdk::ModifierType::HYPER_MASK),
            ]
        });

        let mask = MAPPING.iter().fold(gdk::ModifierType::empty(), |accum, (name, mask)| {
            if self.mod_name_is_active(name, xkb::STATE_MODS_EFFECTIVE) {
                accum | *mask
            } else {
                accum
            }
        });

        // GDK also doesn't add META if MOD1 is set, because GDK treats MOD1 as a synonym for Alt,
        // and does not expect it to be mapped to something else.
        if self.mod_name_is_active("Meta", xkb::STATE_MODS_EFFECTIVE) && !mask.contains(gdk::ModifierType::MOD1_MASK) {
            mask | gdk::ModifierType::META_MASK
        } else {
            mask
        }
    }
}
