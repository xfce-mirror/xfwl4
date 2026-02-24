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

use std::sync::Mutex;

use smithay::{
    desktop::Window,
    output::Output,
    wayland::image_copy_capture::{Frame, Session, SessionRef},
};

#[derive(Default)]
struct ImageCopySessions {
    sessions: Vec<Session>,
}

#[derive(Default)]
struct ImageCopyFrameQueue {
    frames: Vec<(SessionRef, Frame)>,
}

pub trait OutputImageCopyExt {
    fn add_image_copy_session(&self, session: Session);
    fn remove_image_copy_session(&self, session: &SessionRef);

    fn queue_image_copy_frame(&self, session: &SessionRef, frame: Frame);
    fn take_image_copy_frames(&self) -> Option<Vec<(SessionRef, Frame)>>;
}

impl OutputImageCopyExt for Output {
    fn add_image_copy_session(&self, session: Session) {
        self.user_data()
            .get_or_insert(|| Mutex::new(ImageCopySessions::default()))
            .lock()
            .unwrap()
            .sessions
            .push(session);
    }

    fn remove_image_copy_session(&self, session: &SessionRef) {
        if let Some(sessions) = self.user_data().get::<Mutex<ImageCopySessions>>() {
            sessions.lock().unwrap().sessions.retain(|s| s != session);
        }
    }

    fn queue_image_copy_frame(&self, session: &SessionRef, frame: Frame) {
        self.user_data()
            .get_or_insert(|| Mutex::new(ImageCopyFrameQueue::default()))
            .lock()
            .unwrap()
            .frames
            .push((session.clone(), frame));
    }

    fn take_image_copy_frames(&self) -> Option<Vec<(SessionRef, Frame)>> {
        self.user_data()
            .get::<Mutex<ImageCopyFrameQueue>>()
            .map(|queue| std::mem::take(&mut queue.lock().unwrap().frames))
            .filter(|frames| !frames.is_empty())
            .map(Some)
            .unwrap_or_default()
    }
}

pub trait WindowImageCopyExt {
    fn add_image_copy_session(&self, session: Session);
    fn remove_image_copy_session(&self, session: &SessionRef);
}

impl WindowImageCopyExt for Window {
    fn add_image_copy_session(&self, session: Session) {
        self.user_data()
            .get_or_insert(|| Mutex::new(ImageCopySessions::default()))
            .lock()
            .unwrap()
            .sessions
            .push(session);
    }

    fn remove_image_copy_session(&self, session: &SessionRef) {
        if let Some(sessions) = self.user_data().get::<Mutex<ImageCopySessions>>() {
            sessions.lock().unwrap().sessions.retain(|s| s != session);
        }
    }
}
