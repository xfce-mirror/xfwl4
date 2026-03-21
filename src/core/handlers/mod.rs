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
//
// Portions of this file are based on "anvil", an example compositor
// based on the smithay crate, and are licensed under the MIT license
// with the following terms:
//
// Copyright (C) Victor Berger <victor.berger@m4x.org>
// Copyright (C) Drakulix (Victoria Brekenfeld)
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use std::collections::HashSet;

use crate::{
    backend::Backend,
    core::{
        shell::{WindowElement, WindowState},
        state::{Xfwl4Core, Xfwl4State},
    },
    protocols::{wlr_foreign_toplevel_management::WlrForeignToplevelHandler, wlr_screencopy::WlrScreencopyState},
};

use smithay::{
    input::{Seat, SeatState},
    output::Output,
    reexports::wayland_server::protocol::{wl_shm, wl_surface::WlSurface},
    wayland::{
        commit_timing::CommitTimingManagerState,
        fifo::FifoManagerState,
        fractional_scale::FractionalScaleManagerState,
        idle_notify::IdleNotifierState,
        image_copy_capture::ImageCopyCaptureState,
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitState,
        output::OutputManagerState,
        presentation::PresentationState,
        selection::{data_device::DataDeviceState, primary_selection::PrimarySelectionState, wlr_data_control::DataControlState},
        shm::ShmState,
        single_pixel_buffer::SinglePixelBufferState,
        viewporter::ViewporterState,
        xdg_activation::XdgActivationState,
        xdg_foreign::XdgForeignState,
        xdg_toplevel_icon::XdgToplevelIconManager,
    },
};

pub mod data_device;
mod decoration;
mod ext_idle_notify;
mod ext_session_lock;
mod foreign_toplevel;
mod fractional_scale;
mod image_capture_source;
mod image_copy_capture;
mod input;
mod output;
mod seat;
mod security_context;
mod shm;
mod wlr_screencopy;
mod wp_idle_inhibit;
mod xdg_activation;
mod xdg_foreign;
mod xdg_toplevel_icon;
pub mod xfwl4_compositor_ui;

pub(super) use decoration::DecorationState;
pub(super) use ext_session_lock::ExtSessionLockState;
pub(crate) use foreign_toplevel::ForeignToplevelState;
pub(super) use image_capture_source::ExtImageCaptureSourceState;

pub struct ProtocolDelegates<BackendData: Backend + 'static> {
    _commit_timing_manager_state: CommitTimingManagerState,
    data_control_state: DataControlState,
    data_device_state: DataDeviceState,
    decoration_state: DecorationState,
    ext_idle_notifier_state: IdleNotifierState<Xfwl4State<BackendData>>,
    ext_image_capture_source_state: ExtImageCaptureSourceState,
    ext_session_lock_state: ExtSessionLockState,
    _fifo_manager_state: FifoManagerState,
    foreign_toplevel_state: ForeignToplevelState<BackendData>,
    _fractional_scale_manager_state: FractionalScaleManagerState,
    idle_inhibit_surfaces: HashSet<WlSurface>,
    image_copy_capture_state: ImageCopyCaptureState,
    keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
    _output_manager_state: OutputManagerState,
    _presentation_state: PresentationState,
    primary_selection_state: PrimarySelectionState,
    seat_state: SeatState<Xfwl4State<BackendData>>,
    shm_state: ShmState,
    _single_pixel_buffer_state: SinglePixelBufferState,
    _viewporter_state: ViewporterState,
    wlr_screencopy_state: WlrScreencopyState,
    xdg_activation_state: XdgActivationState,
    xdg_foreign_state: XdgForeignState,
    xdg_toplevel_icon_manager: XdgToplevelIconManager,
}

impl<BackendData: Backend + 'static> ProtocolDelegates<BackendData> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        commit_timing_manager_state: CommitTimingManagerState,
        data_control_state: DataControlState,
        data_device_state: DataDeviceState,
        decoration_state: DecorationState,
        ext_idle_notifier_state: IdleNotifierState<Xfwl4State<BackendData>>,
        ext_image_capture_source_state: ExtImageCaptureSourceState,
        ext_session_lock_state: ExtSessionLockState,
        fifo_manager_state: FifoManagerState,
        foreign_toplevel_state: ForeignToplevelState<BackendData>,
        fractional_scale_manager_state: FractionalScaleManagerState,
        image_copy_capture_state: ImageCopyCaptureState,
        keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
        output_manager_state: OutputManagerState,
        presentation_state: PresentationState,
        primary_selection_state: PrimarySelectionState,
        seat_state: SeatState<Xfwl4State<BackendData>>,
        shm_state: ShmState,
        single_pixel_buffer_state: SinglePixelBufferState,
        viewporter_state: ViewporterState,
        wlr_screencopy_state: WlrScreencopyState,
        xdg_activation_state: XdgActivationState,
        xdg_foreign_state: XdgForeignState,
        xdg_toplevel_icon_manager: XdgToplevelIconManager,
    ) -> Self {
        Self {
            _commit_timing_manager_state: commit_timing_manager_state,
            data_control_state,
            data_device_state,
            decoration_state,
            ext_idle_notifier_state,
            ext_image_capture_source_state,
            ext_session_lock_state,
            _fifo_manager_state: fifo_manager_state,
            foreign_toplevel_state,
            _fractional_scale_manager_state: fractional_scale_manager_state,
            idle_inhibit_surfaces: HashSet::new(),
            image_copy_capture_state,
            keyboard_shortcuts_inhibit_state,
            _output_manager_state: output_manager_state,
            _presentation_state: presentation_state,
            primary_selection_state,
            seat_state,
            shm_state,
            _single_pixel_buffer_state: single_pixel_buffer_state,
            _viewporter_state: viewporter_state,
            wlr_screencopy_state,
            xdg_activation_state,
            xdg_foreign_state,
            xdg_toplevel_icon_manager,
        }
    }
}

impl<BackendData: Backend + 'static> Xfwl4Core<BackendData> {
    #[inline]
    pub(super) fn notify_activity(&mut self, seat: &Seat<Xfwl4State<BackendData>>) {
        self.protocol_delegates.ext_idle_notifier_state.notify_activity(seat);
    }

    #[inline]
    pub(super) fn session_lock_surface_for_output(&self, output: &Output) -> Option<WlSurface> {
        self.protocol_delegates
            .ext_session_lock_state
            .lock_surface_for_output(output)
            .map(|lock_surface| lock_surface.wl_surface().clone())
    }

    #[inline]
    pub(crate) fn update_shm_formats(&mut self, formats: impl IntoIterator<Item = wl_shm::Format>) {
        self.protocol_delegates.shm_state.update_formats(formats);
    }

    #[inline]
    pub(super) fn add_toplevel_icon_size(&mut self, size: i32) {
        self.protocol_delegates.xdg_toplevel_icon_manager.add_icon_size(size);
    }

    #[inline]
    pub fn replace_toplevel_icon_sizes<I: IntoIterator<Item = i32>>(&mut self, icon_sizes: I) {
        self.protocol_delegates.xdg_toplevel_icon_manager.replace_icon_sizes(icon_sizes);
    }

    #[inline]
    pub(super) fn toplevel_created<H: WlrForeignToplevelHandler>(
        &mut self,
        window: &WindowElement,
        outputs: Vec<Output>,
        parent: Option<&WindowElement>,
    ) {
        self.protocol_delegates
            .foreign_toplevel_state
            .toplevel_created::<H>(window, outputs, parent);
    }

    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub(super) fn toplevel_changed(
        &mut self,
        window: &WindowElement,
        title: Option<&str>,
        app_id: Option<&str>,
        states_added: WindowState,
        states_removed: WindowState,
        outputs_added: Vec<Output>,
        outputs_removed: Vec<Output>,
        parent: Option<Option<&WindowElement>>,
    ) {
        self.protocol_delegates.foreign_toplevel_state.toplevel_changed(
            window,
            title,
            app_id,
            states_added,
            states_removed,
            outputs_added,
            outputs_removed,
            parent,
        );
    }

    pub(super) fn toplevel_destroyed(&mut self, window: &WindowElement) {
        self.protocol_delegates.foreign_toplevel_state.toplevel_destroyed(window);
    }
}

smithay::delegate_viewporter!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
smithay::delegate_presentation!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
smithay::delegate_single_pixel_buffer!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
smithay::delegate_fifo!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
smithay::delegate_commit_timing!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
smithay::delegate_fixes!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
smithay::delegate_alpha_modifier!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
