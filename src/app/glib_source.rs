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

//! Drives a [`glib::MainContext`] from within calloop's event loop.
//!
//! GLib has no single pollable fd: a context is a *set* of fds plus a timeout, either of which can
//! change per-iteration.  To integrate it without busy-polling we let calloop own the blocking
//! poll and mirror glib's fds + next timeout into it, so calloop wakes exactly when glib has work.
//! The actual dispatch is delegated to a single non-blocking [`glib::MainContext::iteration()`],
//! which runs glib's own `check`/`dispatch` and so spares us from mapping calloop readiness back
//! onto `GIOCondition` revents.
//!
//! ## Two sources
//!
//! The integration is split across two cooperating calloop sources, both created by [`sources()`]:
//!
//! - [`GMainContextSource`] (the *worker*) registers glib's fds + next timeout and does the
//!   dispatching.  It re-queries glib after every dispatch via `PostAction::Reregister`.
//! - [`GMainContextPinger`] (the *pinger*) exists only to close a gap: a glib source attached from
//!   *this* thread outside our dispatch does **not** signal the context's wakeup fd (glib's
//!   `g_source_attach` only wakes when the context is owned by another thread, and we own it — see
//!   `g_source_attach_unlocked`), so the worker would never learn of it.  The pinger hooks
//!   calloop's `before_sleep`, cheaply re-checks glib before every sleep, and pings the worker
//!   whenever glib's fd set or next deadline changed, forcing a re-query.  This keeps glib correct
//!   regardless of the loop's poll cadence and without periodic wakeups.
//!
//! The pinger can't update calloop's poll itself: a source that opts into `before_sleep` must
//! never `Reregister` (calloop 0.14 appends it to its extra-lifecycle set without dedup on every
//! reregister, leaking unboundedly), so the work of (un)registering fds stays in the
//! non-extra-lifecycle worker, woken via the ping.

use std::{
    io::{Error as IoError, ErrorKind, Result as IoResult},
    os::{fd::BorrowedFd, raw::c_int},
    time::{Duration, Instant},
};

use glib::{ffi as gffi, translate::ToGlibPtr};
use smithay::reexports::calloop::{
    self, EventSource, Interest, Mode, Poll, PostAction, Readiness, Token, TokenFactory,
    ping::{Ping, PingSource, make_ping},
    timer::Timer,
};

// GIOCondition bits mapped onto calloop read- vs write-readiness. ERR/HUP are
// surfaced by the poller regardless, but glib requests them via the read set, so
// they are folded in there.
const READABLE_EVENTS: u16 = (gffi::G_IO_IN | gffi::G_IO_PRI | gffi::G_IO_ERR | gffi::G_IO_HUP) as u16;
const WRITABLE_EVENTS: u16 = gffi::G_IO_OUT as u16;

// Slack when comparing successive glib deadlines, to absorb sub-millisecond jitter
// in `Instant::now()` so a stable timeout doesn't read as "moved earlier" each turn.
const DEADLINE_SLOP: Duration = Duration::from_millis(2);

/// The main event source for the main context
///
/// This source handles iterating the main context and managing the list of fds and the timeout.
pub struct GMainContextSource {
    context: glib::MainContext,
    fds: Vec<gffi::GPollFD>,
    timer: Option<Timer>,
    ping_source: PingSource,
    _ownership: ContextOwnership,
}

/// Wakes the main context when new sources are added on the same thread
///
/// Watches glib for sources attached from this thread (which glib does not wake us for) and pings
/// the worker to re-query when its view would be stale.  Registers no fds of its own; it works
/// purely through `before_sleep`.
pub struct GMainContextPinger {
    context: glib::MainContext,
    fds: Vec<gffi::GPollFD>,
    prev_fds: Vec<(c_int, u16)>,
    prev_deadline: Option<Instant>,
    prev_ready: bool,
    ping: Ping,
}

// glib's acquire RAII mechanism takes a reference to the context, so we can't store the guard
// somewhere in order to keep ownership.  This works just as well, and avoids the need to drop to
// FFI in the main part of the code.
//
// We need to acquire ownership of the context for the lifetime of the event souce, as this is the
// only way the context gets properly notified to wake up when code from another thread adds a
// source.
struct ContextOwnership(glib::MainContext);

impl GMainContextSource {
    /// Mirror glib's current fds + timeout into `poll`, rolling back any partial registration if a
    /// later step fails so calloop's poll is never left holding a subset of our fds.
    fn install(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> calloop::Result<()> {
        fn install_inner(source: &mut GMainContextSource, poll: &mut Poll, token_factory: &mut TokenFactory) -> calloop::Result<()> {
            let timeout = query(&source.context, &mut source.fds);

            for gfd in &source.fds {
                // SAFETY: gfd.fd was just returned by g_main_context_query and is open at
                // this point; the BorrowedFd is used only for this register() call.
                unsafe {
                    poll.register(
                        BorrowedFd::borrow_raw(gfd.fd),
                        interest_of(gfd.events),
                        Mode::Level,
                        token_factory.token(),
                    )?;
                }
            }
            source.ping_source.register(poll, token_factory)?;

            source.timer = match timeout {
                t if t < 0 => None,
                0 => Some(Timer::immediate()),
                ms => Some(Timer::from_duration(Duration::from_millis(ms as u64))),
            };
            if let Some(timer) = source.timer.as_mut() {
                timer.register(poll, token_factory)?;
            }

            Ok(())
        }

        let outcome = install_inner(self, poll, token_factory);
        if outcome.is_err() {
            self.uninstall(poll);
        }
        outcome
    }

    fn uninstall(&mut self, poll: &mut Poll) {
        for gfd in &self.fds {
            // SAFETY: each fd came from a prior query().  The just-finished dispatch may have
            // closed one (e.g. a GDBus connection dropping), and the GDBus worker thread may even
            // have reopened that number concurrently, so the BorrowedFd may not refer to glib's fd
            // anymore.  It is used only for an epoll_ctl DEL: a closed fd is already gone from the
            // epoll (EBADF, ignored), and a concurrently-reused number is not in our epoll
            // (ENOENT, ignored), so no unrelated registration is ever removed.
            let _ = poll.unregister(unsafe { BorrowedFd::borrow_raw(gfd.fd) });
        }
        let _ = self.ping_source.unregister(poll);
        if let Some(mut timer) = self.timer.take() {
            let _ = timer.unregister(poll);
        }
    }
}

impl EventSource for GMainContextSource {
    type Event = ();
    type Metadata = ();
    type Ret = ();
    type Error = calloop::Error;

    fn process_events<F>(&mut self, readiness: Readiness, token: Token, _: F) -> Result<PostAction, Self::Error>
    where
        F: FnMut((), &mut ()),
    {
        // Drain the pinger's ping if that's what fired (no-op for any other token), so its pipe
        // doesn't keep re-waking us.
        self.ping_source
            .process_events(readiness, token, |(), &mut ()| {})
            .map_err(|err| calloop::Error::OtherError(err.into()))?;

        // A glib fd is ready, the timeout elapsed, or the pinger pinged: one non-blocking turn
        // runs glib's prepare/query/poll(0)/check/dispatch and fires every ready source. A single
        // iteration (rather than draining to quiescence) keeps a busy glib source from starving
        // the rest of the loop; if more remains, query() arms an immediate timer and we re-enter
        // next turn. Reregister afterwards since dispatch (or a ping) may have changed the fd set
        // or the next timeout.
        self.context.iteration(false);
        Ok(PostAction::Reregister)
    }

    fn register(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> calloop::Result<()> {
        self.install(poll, token_factory)
    }

    fn reregister(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> calloop::Result<()> {
        self.uninstall(poll);
        self.install(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.uninstall(poll);
        Ok(())
    }
}

impl EventSource for GMainContextPinger {
    type Event = ();
    type Metadata = ();
    type Ret = ();
    type Error = calloop::Error;

    const NEEDS_EXTRA_LIFECYCLE_EVENTS: bool = true;

    fn before_sleep(&mut self) -> calloop::Result<Option<(Readiness, Token)>> {
        // The worker owns the context on this thread; if for some reason it doesn't, prepare/query
        // would be illegal, so skip.
        if self.context.is_owner() {
            let timeout = query(&self.context, &mut self.fds);
            let deadline = deadline_of(timeout, Instant::now());
            let ready = timeout == 0;
            let mut fds: Vec<(c_int, u16)> = self.fds.iter().map(|gfd| (gfd.fd, gfd.events)).collect();
            fds.sort_unstable();

            // Ping the worker when glib's poll requirements have changed in a way it wouldn't
            // otherwise notice: a new/removed fd, a freshly-ready source, or a deadline that moved
            // earlier (a new sooner timeout). Re-arming an existing source already wakes us via
            // g_source_set_ready_time, and stable state reads identically each turn, so neither
            // pings.
            let sooner = match (deadline, self.prev_deadline) {
                (Some(now_deadline), Some(prev)) => now_deadline + DEADLINE_SLOP < prev,
                (Some(_), None) => true,
                (None, _) => false,
            };
            if fds != self.prev_fds || (ready && !self.prev_ready) || sooner {
                self.ping.ping();
            }

            self.prev_fds = fds;
            self.prev_deadline = deadline;
            self.prev_ready = ready;
        }
        Ok(None)
    }

    fn process_events<F>(&mut self, _: Readiness, _: Token, _: F) -> Result<PostAction, Self::Error>
    where
        F: FnMut((), &mut ()),
    {
        Ok(PostAction::Continue)
    }

    fn register(&mut self, _: &mut Poll, _: &mut TokenFactory) -> calloop::Result<()> {
        Ok(())
    }

    fn reregister(&mut self, _: &mut Poll, _: &mut TokenFactory) -> calloop::Result<()> {
        Ok(())
    }

    fn unregister(&mut self, _: &mut Poll) -> calloop::Result<()> {
        Ok(())
    }
}

impl ContextOwnership {
    fn acquire(context: glib::MainContext) -> Option<Self> {
        // SAFETY: to_glib_none yields a valid GMainContext pointer for the call.
        let acquired = unsafe { gffi::g_main_context_acquire(context.to_glib_none().0) } != 0;
        acquired.then(|| Self(context))
    }
}

impl Drop for ContextOwnership {
    fn drop(&mut self) {
        // SAFETY: balances the acquire above. The event loop and its sources are
        // dropped on the same thread that created them, which is the owning thread.
        unsafe { gffi::g_main_context_release(self.0.to_glib_none().0) };
    }
}

/// `prepare` + `query`: refresh `fds`, returning glib's requested timeout in
/// milliseconds (negative meaning "block indefinitely"). The caller must own the
/// context on the current thread (glib requires it for prepare/query).
fn query(context: &glib::MainContext, fds: &mut Vec<gffi::GPollFD>) -> i32 {
    let (_ready, max_priority) = context.prepare();
    let raw = context.to_glib_none().0;
    let mut timeout: c_int = 0;
    let mut len = fds.len();
    loop {
        fds.resize(
            len,
            gffi::GPollFD {
                fd: 0,
                events: 0,
                revents: 0,
            },
        );
        // SAFETY: `raw` is a valid GMainContext owned by this thread (see caller);
        // glib fills up to `len` entries and `fds` has that many writable elements.
        let n = unsafe { gffi::g_main_context_query(raw, max_priority, &mut timeout, fds.as_mut_ptr(), len as c_int) } as usize;
        if n <= len {
            fds.truncate(n);
            break timeout;
        } else {
            len = n;
        }
    }
}

fn interest_of(events: u16) -> Interest {
    Interest {
        readable: events & READABLE_EVENTS != 0,
        writable: events & WRITABLE_EVENTS != 0,
    }
}

/// Absolute deadline glib wants to wake at, given a `query` timeout (ms).
fn deadline_of(timeout: i32, now: Instant) -> Option<Instant> {
    (timeout >= 0).then(|| now + Duration::from_millis(timeout as u64))
}

/// Create the worker + pinger pair that together drive `context`.
///
/// Must be called on the thread that will run the event loop, since it takes ownership of
/// `context` for that thread.
///
/// Two sources are needed in order to address mismatches in how glib and calloop work.  Insert
/// both sources into the event loop.  Their loop callbacks are never invoked, so you should pass
/// an empty closure. to [`LoopHandle::insert_source()`](calloop::LoopHandle::insert_source).
///
/// Fails if `context` is already owned by another thread, or if underlying OS resources cannot be
/// allocated.
pub fn sources(context: glib::MainContext) -> IoResult<(GMainContextSource, GMainContextPinger)> {
    let ownership = ContextOwnership::acquire(context.clone())
        .ok_or_else(|| IoError::new(ErrorKind::ResourceBusy, "GMainContext is already owned by another thread"))?;
    let (ping, ping_source) = make_ping()?;
    let worker = GMainContextSource {
        context: context.clone(),
        fds: Vec::with_capacity(8),
        timer: None,
        ping_source,
        _ownership: ownership,
    };
    let pinger = GMainContextPinger {
        context,
        fds: Vec::with_capacity(8),
        prev_fds: Vec::new(),
        prev_deadline: None,
        prev_ready: false,
        ping,
    };
    Ok((worker, pinger))
}
