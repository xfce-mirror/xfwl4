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

use std::{collections::HashMap, fmt, str::FromStr};

use glib::{Sender, clone};
use libxfce4kbd_private::{ShortcutManualExt, ShortcutsProvider, ShortcutsProviderExt};

use crate::{
    core::config::ShortcutKey,
    ui::{ToUiMessage, UnparsedShortcut},
};

#[derive(Debug)]
pub struct KeyboardShorctutsConfig<ActionType: Send + Sync + 'static> {
    provider: ShortcutsProvider,
    shortcuts: HashMap<ShortcutKey, ActionType>,
}

impl<ActionType> KeyboardShorctutsConfig<ActionType>
where
    ActionType: FromStr + fmt::Display + Clone + PartialEq + Send + Sync + 'static,
    <ActionType as FromStr>::Err: fmt::Display,
{
    pub fn new(provider_name: &str) -> Self {
        Self {
            provider: ShortcutsProvider::new(provider_name),
            shortcuts: HashMap::new(),
        }
    }

    pub fn init<F1, F2>(&self, to_ui_tx: Sender<ToUiMessage>, added_event_builder: F1, removed_event_builder: F2)
    where
        F1: Fn(String, ActionType) -> UnparsedShortcut + 'static,
        F2: Fn(String) -> UnparsedShortcut + 'static,
    {
        let shortcuts = self.provider.shortcuts().into_iter().flat_map(|shortcut| {
            shortcut
                .command()
                .parse::<ActionType>()
                .map(|action| (shortcut.shortcut().to_owned(), action))
        });

        for (accelerator, action) in shortcuts {
            let _ = to_ui_tx.send(ToUiMessage::ParseShortcut(added_event_builder(accelerator, action)));
        }

        self.provider
            .connect_shortcut_added(clone!(@strong to_ui_tx => move |provider, name| {
                if let Some(shortcut) = provider.shortcut(name) {
                    match shortcut.command().parse::<ActionType>() {
                        Err(err) => tracing::warn!("Shortcut action '{}' is invalid: {err}", shortcut.command()),
                        Ok(action) => {
                            let _ = to_ui_tx.send(ToUiMessage::ParseShortcut(added_event_builder(shortcut.shortcut().to_owned(), action)));
                        }
                    }
                }
            }));
        self.provider
            .connect_shortcut_removed(clone!(@strong to_ui_tx => move |_provider, name| {
                let _ = to_ui_tx.send(ToUiMessage::ParseShortcut(removed_event_builder(name.to_owned())));
            }));
    }

    pub fn add(&mut self, key: ShortcutKey, action: ActionType) {
        self.shortcuts.insert(key, action);
    }

    pub fn find<'a>(&'a self, key: &ShortcutKey) -> Option<&'a ActionType> {
        self.shortcuts.get(key)
    }

    pub fn find_by_action<'a>(&'a self, action: &ActionType) -> Option<&'a ShortcutKey> {
        self.shortcuts
            .iter()
            .find_map(|(key, an_action)| (action == an_action).then_some(key))
    }

    pub fn remove(&mut self, key: &ShortcutKey) {
        self.shortcuts.remove(key);
    }
}
