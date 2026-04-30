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
    output::Output,
    reexports::wayland_server::protocol::{wl_buffer::WlBuffer, wl_shm},
    utils::{Logical, Rectangle, Transform},
};

use crate::{
    backend::Backend,
    core::{state::Xfwl4State, util::OutputImageCopyExt},
    protocols::wlr_screencopy::{WlrBufferConstraints, WlrFrame, WlrScreencopyHandler, WlrScreencopyState},
};

impl<BackendData: Backend + 'static> WlrScreencopyHandler for Xfwl4State<BackendData> {
    fn wlr_screencopy_state(&mut self) -> &mut WlrScreencopyState {
        &mut self.core.protocol_delegates.wlr_screencopy_state
    }

    fn buffer_constraints(&mut self, output: &Output, output_rect: Rectangle<i32, Logical>) -> Option<WlrBufferConstraints> {
        let size = output_rect
            .size
            .to_f64()
            .to_buffer(output.current_scale().fractional_scale(), Transform::Normal)
            .to_i32_round();

        #[cfg(any(feature = "udev", feature = "winit"))]
        let dmabuf_constraints = self
            .backend
            .dmabuf_constraints(None)
            .map(|constraints| constraints.formats.into_iter().map(|(format, _)| format).collect())
            .unwrap_or_default();

        Some(WlrBufferConstraints {
            size,
            shm: vec![(wl_shm::Format::Argb8888, (size.w * 4) as u32)],
            #[cfg(any(feature = "udev", feature = "winit"))]
            dma: dmabuf_constraints,
        })
    }

    fn on_copy(&mut self, frame: WlrFrame, output: Output, buffer: WlBuffer) {
        output.queue_wlr_screencopy_frame(frame, buffer);
    }
}
