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

use std::{
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};

use anyhow::anyhow;
use calloop::futures::{Scheduler, executor};
use smithay::reexports::calloop::LoopHandle;
use zbus::{Connection, connection::Builder as ConnectionBuilder};

#[derive(Debug, Clone)]
pub struct ZBusAdapter {
    conn: Connection,
    scheduler: Scheduler<()>,
    running: Rc<AtomicBool>,
}

impl ZBusAdapter {
    pub fn init<S>(handle: LoopHandle<'_, S>) -> anyhow::Result<ZBusAdapter> {
        let conn = async_io::block_on(ConnectionBuilder::session()?.internal_executor(false).build())?;

        let (executor, scheduler) = executor()?;
        handle
            .insert_source(executor, |_, _, _| {})
            .map_err(|err| anyhow!("Failed to register calloop executor in event loop: {err}"))?;

        let running = Rc::new(AtomicBool::new(true));
        scheduler.schedule({
            let running = Rc::clone(&running);
            let conn = conn.clone();
            async move {
                while running.load(Ordering::SeqCst) {
                    conn.executor().tick().await;
                }
            }
        })?;

        Ok(ZBusAdapter { conn, scheduler, running })
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn schedule<F>(&self, fut: F)
    where
        F: Future<Output = ()> + 'static,
    {
        if self.scheduler.schedule(fut).is_err() {
            tracing::warn!("Tried to schedule calloop future, but the executor has been destroyed");
        }
    }

    pub fn shutdown(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}
