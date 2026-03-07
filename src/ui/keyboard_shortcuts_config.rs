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

use std::{collections::HashMap, marker::PhantomData, str::FromStr};

use glib::clone;
use gtk::gdk;
use libxfce4kbd_private::{Shortcut, ShortcutManualExt, ShortcutsProvider, ShortcutsProviderExt};
use smithay::reexports::calloop::channel::Sender;
use xkbcommon::xkb::Keysym;

use crate::ui::FromUiMessage;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ShortcutKey {
    pub keysym: Keysym,
    pub modifiers: gdk::ModifierType,
}

impl ShortcutKey {
    pub fn new(keysym: Keysym, modifiers: gdk::ModifierType) -> Self {
        Self { keysym, modifiers }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // ShortcutsProvider is never used but needs to be kept alive
pub struct KeyboardShorctutsConfig<ActionType: Send + Sync + 'static>(ShortcutsProvider, PhantomData<ActionType>);

impl<ActionType> KeyboardShorctutsConfig<ActionType>
where
    ActionType: FromStr + Clone + Send + Sync + 'static,
{
    pub fn new<F1, F2>(provider_name: &str, from_ui_tx: Sender<FromUiMessage>, added_event_builder: F1, removed_event_builder: F2) -> Self
    where
        F1: Fn(ShortcutKey, ActionType) -> FromUiMessage + 'static,
        F2: Fn(ShortcutKey) -> FromUiMessage + 'static,
    {
        let provider = ShortcutsProvider::new(provider_name);
        let shortcuts = provider
            .shortcuts()
            .into_iter()
            .flat_map(parse_shortcut::<ActionType>)
            .collect::<HashMap<ShortcutKey, ActionType>>();

        for (key, action) in shortcuts {
            let _ = from_ui_tx.send(added_event_builder(key, action));
        }

        let config = Self(provider.clone(), PhantomData::<ActionType>);

        provider.connect_shortcut_added(clone!(@strong from_ui_tx => move |provider, name| {
            if let Some((key, action)) = provider.shortcut(name).and_then(parse_shortcut::<ActionType>) {
                let _ = from_ui_tx.send(added_event_builder(key, action));
            } else {
                tracing::warn!("Accellerator '{}' for shortcut is missing or invalid", name);
            }
        }));
        provider.connect_shortcut_removed(clone!(@strong from_ui_tx => move |_provider, name| {
            let (key, modifiers) = gtk::accelerator_parse(name);
            if key != 0 || !modifiers.is_empty() {
                let key = ShortcutKey::new(Keysym::new(key), modifiers);
                removed_event_builder(key);
            } else {
                tracing::warn!("Accellerator '{}' for shortcut is invalid", name);
            }
        }));

        config
    }
}

fn parse_shortcut<ActionType>(shortcut: Shortcut) -> Option<(ShortcutKey, ActionType)>
where
    ActionType: FromStr + Clone + Send + Sync + 'static,
{
    if let Ok(action) = shortcut.command().parse::<ActionType>()
        && let (key, modifiers) = gtk::accelerator_parse(shortcut.shortcut())
        && (key != 0 || !modifiers.is_empty())
    {
        let keysym = Keysym::new(key);
        let keysym = if keysym == Keysym::Tab && modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
            // When <Shift> is held, the keysym we get from libinput is ISO_Left_Tab, not Tab.
            Keysym::ISO_Left_Tab
        } else {
            keysym
        };

        let key = ShortcutKey::new(keysym, modifiers);
        Some((key, action))
    } else {
        tracing::warn!(
            "Accellerator '{}' for shortcut '{}' is missing or invalid",
            shortcut.shortcut(),
            shortcut.command()
        );
        None
    }
}
