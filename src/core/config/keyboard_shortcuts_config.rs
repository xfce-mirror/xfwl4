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

use std::{cell::RefCell, collections::HashMap, fmt, rc::Rc, str::FromStr};

use anyhow::anyhow;
use glib::clone;
use xfce4_kbd_private::{ShortcutManualExt, ShortcutsProvider, ShortcutsProviderExt};

use crate::core::config::{ShortcutKey, keyboard_shortcuts::parse_accelerator};

#[derive(Debug, Clone)]
pub struct KeyboardShorctutsConfig<ActionType> {
    provider: Rc<ShortcutsProvider>,
    shortcuts: Rc<RefCell<HashMap<ShortcutKey, ActionType>>>,
}

impl<ActionType> KeyboardShorctutsConfig<ActionType>
where
    ActionType: FromStr + fmt::Display + Clone + PartialEq + 'static,
    <ActionType as FromStr>::Err: fmt::Display,
{
    pub fn new(provider_name: &str) -> Self {
        let config = Self {
            provider: Rc::new(ShortcutsProvider::new(provider_name)),
            shortcuts: Default::default(),
        };

        let parse_accelerator_and_action = |accelerator: &str, action_name: &str| {
            if let Some(key) = parse_accelerator(accelerator) {
                if let Ok(action) = action_name.parse::<ActionType>() {
                    Ok((key, action))
                } else {
                    Err(anyhow!("Invalid shortcut action '{action_name}'"))
                }
            } else {
                Err(anyhow!("Invalid shortcut accelerator '{accelerator}' for action '{action_name}'"))
            }
        };

        for shortcut in config.provider.shortcuts() {
            match parse_accelerator_and_action(shortcut.shortcut(), shortcut.command()) {
                Ok((key, action)) => {
                    config.shortcuts.borrow_mut().insert(key, action);
                }
                Err(err) => tracing::info!("{err}"),
            }
        }

        config
            .provider
            .connect_shortcut_added(clone!(@strong config => move |provider, name| {
                if let Some(shortcut) = provider.shortcut(name) {
                    match parse_accelerator_and_action(shortcut.shortcut(), shortcut.command()) {
                        Ok((key, action)) => {
                            config.shortcuts.borrow_mut().insert(key, action);
                        }
                        Err(err) => tracing::info!("{err}"),
                    }
                }
            }));
        config
            .provider
            .connect_shortcut_removed(clone!(@strong config => move |_provider, name| {
                if let Some(key) = parse_accelerator(name) {
                    config.shortcuts.borrow_mut().remove(&key);
                }
            }));

        config
    }

    pub fn find(&self, key: &ShortcutKey) -> Option<ActionType> {
        self.shortcuts.borrow().get(key).cloned()
    }

    pub fn find_by_action(&self, action: &ActionType) -> Option<ShortcutKey> {
        self.shortcuts
            .borrow()
            .iter()
            .find_map(|(key, an_action)| (action == an_action).then(|| key.clone()))
    }
}
