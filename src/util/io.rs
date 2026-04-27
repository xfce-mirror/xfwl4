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

use std::os::fd::{AsFd, RawFd};

use rustix::io::Errno;

#[derive(Debug, Clone, thiserror::Error)]
pub enum IoError {
    #[error("not enough data or expected data not found")]
    DataNotFound,
    #[error("ran out of buffer space while reading")]
    BufferTooSmall,
    #[error("os error: {0}")]
    Errno(#[from] Errno),
}

/// Closes all file descriptors, except stdin, stdout, stderr, and those in `except`.
pub fn close_all_fds(except: &[RawFd]) {
    let mut keep = vec![libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO];
    keep.extend(except);

    for fd in 0..1024 {
        if !keep.contains(&fd) {
            // SAFETY: All our FDs are >= 0, and so are safe to call close() on, even if they
            // aren't open.
            unsafe { libc::close(fd) };
        }
    }
}

/// Reads from `fd` until it sees the bytes sequece passed in `ends_with`.
///
/// Blocks until `ends_with` is seen, or there is a read error or EOF.
///
/// Note that, to ensure it doesn't read past `ends_with`, this reads only a single byte at a time,
/// so it is inefficient if you need to write a lot of data before getting to `ends_with`.
pub fn read_until<FD: AsFd>(fd: FD, buf: &mut [u8], ends_with: &[u8]) -> Result<usize, IoError> {
    let mut total_len = 0;
    loop {
        match rustix::io::read(&fd, &mut buf[total_len..(total_len + 1)]) {
            Err(errno) if errno == Errno::INTR || errno == Errno::AGAIN => (),
            Err(errno) => break Err(errno.into()),
            Ok(len) if buf[0..(total_len + len)].ends_with(ends_with) => break Ok(total_len + len),
            Ok(0) => break Err(IoError::DataNotFound),
            Ok(len) if total_len + len == buf.len() => break Err(IoError::BufferTooSmall),
            Ok(len) => total_len += len,
        }
    }
}

/// Reads exactly the number of bytes available for storage in `buf`.
///
/// Blocks until enough bytes are read, or until there is a read error or EOF.
pub fn read_exact<FD: AsFd>(fd: FD, buf: &mut [u8]) -> Result<usize, IoError> {
    let mut total_len = 0;
    loop {
        match rustix::io::read(&fd, &mut buf[total_len..]) {
            Err(errno) if errno == Errno::INTR || errno == Errno::AGAIN => (),
            Err(errno) => break Err(errno.into()),
            Ok(len) if len == 0 && total_len < buf.len() => break Err(IoError::DataNotFound),
            Ok(len) if total_len + len == buf.len() => break Ok(total_len + len),
            Ok(len) => total_len += len,
        }
    }
}

/// Writes all bytes available in `buf`.
///
/// Blocks until all bytes are written, or until there is a write error.
pub fn write_all<FD: AsFd>(fd: FD, buf: &[u8]) -> Result<usize, Errno> {
    let mut total_len = 0;
    loop {
        match rustix::io::write(&fd, &buf[total_len..]) {
            Err(errno) if errno == Errno::INTR || errno == Errno::AGAIN => (),
            Err(errno) => break Err(errno),
            Ok(len) if total_len + len == buf.len() => break Ok(total_len + len),
            Ok(len) => total_len += len,
        }
    }
}
