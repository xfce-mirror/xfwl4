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
    cell::RefCell,
    os::fd::{AsFd, AsRawFd},
    ptr,
    rc::Rc,
};

use glib::ffi::{
    G_IO_ERR, G_IO_HUP, G_IO_IN, G_IO_OUT, GFALSE, GIOCondition, GSource, GSourceFuncs, GTRUE, g_source_add_unix_fd, g_source_attach,
    g_source_modify_unix_fd, g_source_new, g_source_query_unix_fd, g_source_unref,
};
use wayland_client::{Connection, EventQueue, backend::WaylandError};

/// Custom GSource that integrates a wayland-client EventQueue into a GLib
/// main loop.
///
/// Implements the full GSource lifecycle (prepare/check/dispatch):
/// - prepare: flushes outgoing requests, acquires read guard
/// - check: reads events from fd after poll, determines if dispatch needed
/// - dispatch: dispatches pending events to handlers
///
/// # Safety requirements
///
/// - **Single-threaded only.** The `Rc<RefCell<...>>` state is not
///   `Send`/`Sync`. The source must be attached to a main context that
///   runs on the same thread where it was created.
/// - **No re-entrant dispatch.** If a Wayland event handler triggers a
///   nested main loop (e.g. `gtk::main()`), the `RefCell` borrows will
///   panic.
/// - **No external borrows during dispatch.** The `EventQueue` and state
///   `Rc`s must not be borrowed elsewhere when the source dispatches.
///   Since glib dispatches sources sequentially on one thread, this is
///   satisfied as long as borrows are not held across yield points.
/// - **Call `destroy()` to clean up.** If the source is not destroyed
///   before the main context is dropped, the Rust objects inside will
///   leak.
#[derive(Debug)]
pub struct WaylandClientSource {
    source: *mut GSource,
}

#[repr(C)]
struct WaylandSource<D: 'static> {
    source: GSource,
    fd_tag: glib::ffi::gpointer,
    connection: Connection,
    event_queue: Rc<RefCell<EventQueue<D>>>,
    state: Rc<RefCell<D>>,
    read_guard: Option<wayland_client::backend::ReadEventsGuard>,
}

fn wayland_source<D: 'static>(source: *mut GSource) -> &'static mut WaylandSource<D> {
    unsafe { &mut *(source as *mut WaylandSource<D>) }
}

impl WaylandClientSource {
    pub fn attach<D: 'static>(
        connection: Connection,
        event_queue: Rc<RefCell<EventQueue<D>>>,
        state: Rc<RefCell<D>>,
        context: Option<&glib::MainContext>,
    ) -> Self {
        let funcs = Box::into_raw(Box::new(GSourceFuncs {
            prepare: Some(wayland_source_prepare::<D>),
            check: Some(wayland_source_check::<D>),
            dispatch: Some(wayland_source_dispatch::<D>),
            finalize: Some(wayland_source_finalize::<D>),
            closure_callback: None,
            closure_marshal: None,
        }));

        // SAFETY: We pass valid 'funcs' and the size passed is the actual size of our struct.
        let source = unsafe { g_source_new(funcs, std::mem::size_of::<WaylandSource<D>>() as u32) };
        let ws = wayland_source::<D>(source);

        let fd = connection.as_fd().as_raw_fd();
        // SAFETY: connection guarantees 'fd' is valid.
        ws.fd_tag = unsafe { g_source_add_unix_fd(source, fd, G_IO_IN | G_IO_ERR | G_IO_HUP) };
        // SAFETY: WaylandSource/ws is initialized by g_source_new() to be the appropriate size,
        // and #[repr(C)] gives it a C-compatible layout.
        unsafe {
            ptr::write(&mut ws.connection, connection);
            ptr::write(&mut ws.event_queue, event_queue);
            ptr::write(&mut ws.state, state);
            ptr::write(&mut ws.read_guard, None);
        }

        let context_ptr = context
            .map(|c| {
                use glib::translate::ToGlibPtr;
                c.to_glib_none().0
            })
            .unwrap_or(ptr::null_mut());
        // SAFETY: The source is fully valid (created by g_source_new()), with its other fields
        // set, and 'context_ptr' is either a valid GMainContext, or NULL.
        unsafe { g_source_attach(source, context_ptr) };

        Self { source }
    }

    pub fn destroy(self) {
        // SAFETY: glib will only call destroy() once, so 'self.source' is valid and alive.
        unsafe {
            glib::ffi::g_source_destroy(self.source);
            g_source_unref(self.source);
        }
    }
}

unsafe extern "C" fn wayland_source_prepare<D: 'static>(source: *mut GSource, timeout: *mut i32) -> i32 {
    let ws = wayland_source::<D>(source);

    // SAFETY: glib always passes a valid pointer here.
    unsafe { *timeout = -1 };

    if let Err(err) = ws.connection.backend().flush() {
        match err {
            WaylandError::Io(ref io_err) if io_err.kind() == std::io::ErrorKind::WouldBlock => {
                // SAFETY: We do not touch the FD, so it is always valid, and 'source' came from
                // glib and is valid.
                unsafe {
                    g_source_modify_unix_fd(source, ws.fd_tag, G_IO_IN | G_IO_OUT | G_IO_ERR | G_IO_HUP);
                }
            }
            _ => {
                tracing::error!("Failed to flush Wayland connection: {err}");
                return GTRUE;
            }
        }
    }

    let queue = ws.event_queue.borrow();
    match queue.prepare_read() {
        Some(guard) => {
            ws.read_guard = Some(guard);
            GFALSE
        }
        None => GTRUE,
    }
}

unsafe extern "C" fn wayland_source_check<D: 'static>(source: *mut GSource) -> i32 {
    let ws = wayland_source::<D>(source);

    // SAFETY: 'source' continues to be valid, as it comes from glib.
    let revents = unsafe { g_source_query_unix_fd(source, ws.fd_tag) } as GIOCondition;

    if revents & (G_IO_ERR | G_IO_HUP) != 0 {
        tracing::error!("Wayland connection error or hangup");
        return GTRUE;
    }

    if revents & G_IO_OUT != 0 {
        let _ = ws.connection.backend().flush();
        // SAFETY: 'source' continues to be valid, as it comes from glib; we do not touch the FD.
        unsafe { g_source_modify_unix_fd(source, ws.fd_tag, G_IO_IN | G_IO_ERR | G_IO_HUP) };
    }

    if revents & G_IO_IN != 0 {
        if let Some(guard) = ws.read_guard.take() {
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(err)) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => {
                    tracing::error!("Failed to read Wayland events: {err}");
                    return GTRUE;
                }
            }
        }
        return GTRUE;
    }

    if ws.read_guard.is_none() {
        return GTRUE;
    }

    GFALSE
}

unsafe extern "C" fn wayland_source_dispatch<D: 'static>(
    source: *mut GSource,
    _callback: glib::ffi::GSourceFunc,
    _user_data: glib::ffi::gpointer,
) -> i32 {
    let ws = wayland_source::<D>(source);

    ws.read_guard = None;

    let mut queue = ws.event_queue.borrow_mut();
    let mut state = ws.state.borrow_mut();

    match queue.dispatch_pending(&mut *state) {
        Ok(_) => GTRUE,
        Err(err) => {
            tracing::error!("Failed to dispatch Wayland events: {err}");
            GFALSE
        }
    }
}

unsafe extern "C" fn wayland_source_finalize<D: 'static>(source: *mut GSource) {
    let ws = wayland_source::<D>(source);

    ws.read_guard = None;

    // SAFETY: Since the memory of WaylandSource was allocated by glib, we cannot allow rust's
    // memory deallocation to run for the entire struct.  We manually run Drop for the parts of the
    // struct that are actual rust objects.
    unsafe {
        ptr::drop_in_place(&mut ws.event_queue);
        ptr::drop_in_place(&mut ws.state);
        ptr::drop_in_place(&mut ws.connection);

        let _ = Box::from_raw(ws.source.source_funcs as *mut GSourceFuncs);
    }
}
