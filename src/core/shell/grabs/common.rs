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

use gtk::gdk::ModifierType;
use smithay::{backend::input::KeyState, input::keyboard::KeyboardInnerHandle};
use xkbcommon::xkb::{Keycode, Keysym};

use crate::{
    backend::Backend,
    core::{
        config::{IGNORED_MODIFIERS, ShortcutKey, WmShortcutAction},
        state::Xfwl4State,
        util::XkbStateGdkExt,
    },
};

pub(super) enum MoveResizeAction {
    Up,
    Down,
    Left,
    Right,
    Finish,
    Cancel,
}

pub(super) fn keyboard_move_resize_get_action<BackendData: Backend + 'static>(
    data: &mut Xfwl4State<BackendData>,
    handle: &KeyboardInnerHandle<'_, Xfwl4State<BackendData>>,
    keycode: Keycode,
    state: KeyState,
) -> Option<MoveResizeAction> {
    if state == KeyState::Pressed {
        let key = {
            let keysym_handle = handle.keysym_handle(keycode);
            let keysym = keysym_handle.modified_sym();
            let xkb = keysym_handle.xkb().lock().unwrap();
            // SAFETY: 'state' will not live longer than 'xkb'.
            let state = unsafe { xkb.state() };
            let modifier_mask = state.gdk_modifier_mask();
            ShortcutKey::new(keysym, modifier_mask & !(IGNORED_MODIFIERS | ModifierType::MOD4_MASK))
        };

        if key.keysym == Keysym::Return || key.keysym == Keysym::KP_Enter || key.keysym == Keysym::ISO_Enter {
            Some(MoveResizeAction::Finish)
        } else if let Some(action) = data.core.wm_shortcuts.find(&key) {
            match action {
                WmShortcutAction::Left => Some(MoveResizeAction::Left),
                WmShortcutAction::Right => Some(MoveResizeAction::Right),
                WmShortcutAction::Up => Some(MoveResizeAction::Up),
                WmShortcutAction::Down => Some(MoveResizeAction::Down),
                WmShortcutAction::Cancel => Some(MoveResizeAction::Cancel),
                _ => None,
            }
        } else {
            None
        }
    } else {
        None
    }
}
