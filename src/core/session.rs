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

use anyhow::anyhow;
use calloop::{
    LoopHandle,
    futures::{Scheduler, executor},
};
use gio::{BusType, DBusCallFlags, DBusProxyFlags, traits::DBusProxyExt};
use glib::ToVariant;

use crate::{backend::Backend, core::state::Xfwl4State};

const SESSION_MANAGER_NAME: &str = "org.xfce.SessionManager";
const SESSION_MANAGER_PATH: &str = "/org/xfce/SessionManager";
const SESSION_MANAGER_INTERFACE: &str = "org.xfce.Session.Manager";

enum FutureResultAction {
    LogoutComplete,
}

pub struct Session {
    scheduler: Scheduler<FutureResultAction>,
    logout_in_progress: bool,
}

impl Session {
    pub fn new<BackendData: Backend>(handle: LoopHandle<'_, Xfwl4State<BackendData>>) -> anyhow::Result<Self> {
        let (executor, scheduler) = executor()?;
        handle
            .insert_source(executor, |action, _, state| match action {
                FutureResultAction::LogoutComplete => {
                    state.core.session.logout_in_progress = false;
                }
            })
            .map_err(|err| anyhow!("Failed to register future executor source: {err}"))?;

        Ok(Self {
            scheduler,
            logout_in_progress: false,
        })
    }

    pub fn request_logout(&mut self) -> anyhow::Result<()> {
        async fn do_logout() -> anyhow::Result<()> {
            let bus = gio::bus_get_future(BusType::Session).await?;

            let proxy = gio::DBusProxy::new_future(
                &bus,
                DBusProxyFlags::DO_NOT_CONNECT_SIGNALS | DBusProxyFlags::DO_NOT_LOAD_PROPERTIES,
                None,
                Some(SESSION_MANAGER_NAME),
                SESSION_MANAGER_PATH,
                SESSION_MANAGER_INTERFACE,
            )
            .await?;

            // Logout(show_dialog = true, allow_save = true)
            proxy
                .call_future("Logout", Some(&(true, true).to_variant()), DBusCallFlags::NONE, 1000)
                .await?;

            Ok(())
        }

        if !self.logout_in_progress {
            self.scheduler.schedule(async {
                if let Err(err) = do_logout().await {
                    tracing::warn!("Failed to trigger logout: {err}");
                }
                FutureResultAction::LogoutComplete
            })?;
            self.logout_in_progress = true;
        }

        Ok(())
    }
}
