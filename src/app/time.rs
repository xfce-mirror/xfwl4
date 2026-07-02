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
    mem::MaybeUninit,
    time::{SystemTime, UNIX_EPOCH},
};

use libc::{localtime_r, time_t, tm};
use tracing_subscriber::fmt::{
    format::Writer,
    time::{FormatTime, SystemTime as SystemTimeFormatter},
};

pub struct LocalSystemTime;

impl FormatTime for LocalSystemTime {
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        iso8601_micros()
            .map(|s| w.write_str(&s))
            .unwrap_or_else(|| SystemTimeFormatter.format_time(w))
    }
}

/// Returns the current local time as e.g. "2026-07-01T14:23:07.123456" (ISO 8601 date-time,
/// microsecond precision, but with no timezone designator).
///
/// This is here because `tracing-subscriber` can't format local timestamps without adding a
/// dependency on either `time` or `chrono`.  We already depend on `libc`.
fn iso8601_micros() -> Option<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch");

    let secs: time_t = now.as_secs() as time_t;
    let micros = now.subsec_micros();

    let mut tm = MaybeUninit::<tm>::uninit();
    // SAFETY: `secs` is a valid time_t, `tm` points to writable memory of
    // the correct size, and we only assume_init after checking for success.
    let tm = unsafe {
        if localtime_r(&secs, tm.as_mut_ptr()).is_null() {
            None
        } else {
            Some(tm.assume_init())
        }
    };

    tm.map(|tm| {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec,
            micros
        )
    })
}
