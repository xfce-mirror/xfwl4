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

use smithay::{
    delegate_image_capture_source, delegate_output_capture_source, delegate_toplevel_capture_source,
    output::{Output, WeakOutput},
    reexports::wayland_server::DisplayHandle,
    wayland::{
        foreign_toplevel_list::ForeignToplevelHandle,
        image_capture_source::{
            ImageCaptureSource, ImageCaptureSourceHandler, OutputCaptureSourceHandler, OutputCaptureSourceState,
            ToplevelCaptureSourceHandler, ToplevelCaptureSourceState,
        },
    },
};

use crate::{
    backend::Backend,
    core::{shell::WindowElement, state::Xfwl4State, util::ClientExt},
};

pub enum CaptureSource {
    Output(WeakOutput),
    Toplevel(WindowElement),
}

pub struct ExtImageCaptureSourceState {
    output_capture_source_state: OutputCaptureSourceState,
    toplevel_capture_source_state: ToplevelCaptureSourceState,
}

impl ExtImageCaptureSourceState {
    pub fn new<BackendData: Backend + 'static>(dh: &DisplayHandle) -> Self {
        Self {
            output_capture_source_state: OutputCaptureSourceState::new_with_filter::<Xfwl4State<BackendData>, _>(dh, |client| {
                !client.has_security_context()
            }),
            toplevel_capture_source_state: ToplevelCaptureSourceState::new_with_filter::<Xfwl4State<BackendData>, _>(dh, |client| {
                !client.has_security_context()
            }),
        }
    }
}

impl<BackendData: Backend + 'static> ImageCaptureSourceHandler for Xfwl4State<BackendData> {}

impl<BackendData: Backend + 'static> OutputCaptureSourceHandler for Xfwl4State<BackendData> {
    fn output_capture_source_state(&mut self) -> &mut OutputCaptureSourceState {
        &mut self
            .core
            .protocol_delegates
            .ext_image_capture_source_state
            .output_capture_source_state
    }

    fn output_source_created(&mut self, source: ImageCaptureSource, output: &Output) {
        source.user_data().insert_if_missing(|| CaptureSource::Output(output.downgrade()));
    }
}

impl<BackendData: Backend + 'static> ToplevelCaptureSourceHandler for Xfwl4State<BackendData> {
    fn toplevel_capture_source_state(&mut self) -> &mut ToplevelCaptureSourceState {
        &mut self
            .core
            .protocol_delegates
            .ext_image_capture_source_state
            .toplevel_capture_source_state
    }

    fn toplevel_source_created(&mut self, source: ImageCaptureSource, toplevel: ForeignToplevelHandle) {
        if let Some(window) = self.core.protocol_delegates.foreign_toplevel_state.window_for_handle(&toplevel) {
            source.user_data().insert_if_missing(|| CaptureSource::Toplevel(window));
        }
    }
}

delegate_image_capture_source!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
delegate_output_capture_source!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
delegate_toplevel_capture_source!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
