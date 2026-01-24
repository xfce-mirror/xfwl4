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

use std::{
    io,
    ops::Not,
    sync::{Mutex, Once},
    time::{Duration, Instant},
};

use crate::{
    backend::udev::{UdevData, UdevOutputId},
    drawing::*,
    handlers::data_device::DndIcon,
    render::*,
    shell::WindowElement,
    state::{SurfaceDmabufFeedback, Xfwl4State, take_presentation_feedback, update_primary_scanout_output},
};

use anyhow::{Context, anyhow};
#[cfg(feature = "renderer_sync")]
use smithay::backend::drm::compositor::PrimaryPlaneElement;
use smithay::{
    backend::{
        SwapBuffersError,
        allocator::{Fourcc, gbm::GbmAllocator},
        drm::{
            DrmAccessError, DrmDeviceFd, DrmError, DrmEventMetadata, DrmEventTime, DrmNode, compositor::FrameFlags,
            exporter::gbm::GbmFramebufferExporter, output::DrmOutput,
        },
        renderer::{
            damage::Error as OutputDamageTrackerError,
            element::{AsRenderElements, RenderElementStates, memory::MemoryRenderBuffer},
            gles::GlesRenderer,
            multigpu::{MultiRenderer, gbm::GbmGlesBackend},
        },
    },
    desktop::{
        space::{Space, SurfaceTree},
        utils::OutputPresentationFeedback,
    },
    input::pointer::{CursorImageAttributes, CursorImageStatus},
    output::Output,
    reexports::{
        calloop::{
            RegistrationToken,
            timer::{TimeoutAction, Timer},
        },
        drm::control::crtc,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::{DisplayHandle, backend::GlobalId},
    },
    utils::{IsAlive, Logical, Monotonic, Point, Scale, Time, Transform},
    wayland::{compositor, presentation::Refresh},
};
use tracing::{error, trace, warn};

pub(super) type UdevRenderer<'a> =
    MultiRenderer<'a, 'a, GbmGlesBackend<GlesRenderer, DrmDeviceFd>, GbmGlesBackend<GlesRenderer, DrmDeviceFd>>;

#[derive(Debug, thiserror::Error)]
pub(super) enum RenderFailure {
    #[error("Failed to render surface: {0}")]
    Error(anyhow::Error),
    #[error("Render not needed for this output/device")]
    NotNeeded,
}

pub(super) struct SurfaceData {
    pub dh: DisplayHandle,
    pub device_id: DrmNode,
    pub render_node: Option<DrmNode>,
    pub output: Output,
    pub global: Option<GlobalId>,
    pub drm_output:
        DrmOutput<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, Option<OutputPresentationFeedback>, DrmDeviceFd>,
    pub disable_direct_scanout: bool,
    pub dmabuf_feedback: Option<SurfaceDmabufFeedback>,
    pub last_presentation_time: Option<Time<Monotonic>>,
    pub vblank_throttle_timer: Option<RegistrationToken>,
    #[cfg(feature = "debug")]
    pub debug: Option<crate::debug::RenderDebug<smithay::backend::renderer::multigpu::MultiTexture>>,
}

impl Drop for SurfaceData {
    fn drop(&mut self) {
        self.output.leave_all();
        if let Some(global) = self.global.take() {
            self.dh.remove_global::<Xfwl4State<UdevData>>(global);
        }
    }
}

impl Xfwl4State<UdevData> {
    pub(super) fn frame_finish(&mut self, dev_id: DrmNode, crtc: crtc::Handle, metadata: &mut Option<DrmEventMetadata>) {
        profiling::scope!("frame_finish", &format!("{crtc:?}"));

        let device_backend = match self.backend_data.backends.get_mut(&dev_id) {
            Some(backend) => backend,
            None => {
                error!("Trying to finish frame on non-existent backend {}", dev_id);
                return;
            }
        };

        let surface = match device_backend.surfaces.get_mut(&crtc) {
            Some(surface) => surface,
            None => {
                error!("Trying to finish frame on non-existent crtc {:?}", crtc);
                return;
            }
        };

        if let Some(timer_token) = surface.vblank_throttle_timer.take() {
            self.handle.remove(timer_token);
        }

        let output = if let Some(output) = self.workspace_manager.active_workspace().outputs().find(|o| {
            o.user_data().get::<UdevOutputId>()
                == Some(&UdevOutputId {
                    device_id: surface.device_id,
                    crtc,
                })
        }) {
            output.clone()
        } else {
            // somehow we got called with an invalid output
            return;
        };

        let Some(frame_duration) = output
            .current_mode()
            .map(|mode| Duration::from_secs_f64(1_000f64 / mode.refresh as f64))
        else {
            return;
        };

        let tp = metadata.as_ref().and_then(|metadata| match metadata.time {
            smithay::backend::drm::DrmEventTime::Monotonic(tp) => tp.is_zero().not().then_some(tp),
            smithay::backend::drm::DrmEventTime::Realtime(_) => None,
        });

        let seq = metadata.as_ref().map(|metadata| metadata.sequence).unwrap_or(0);

        let (clock, flags) = if let Some(tp) = tp {
            (
                tp.into(),
                wp_presentation_feedback::Kind::Vsync
                    | wp_presentation_feedback::Kind::HwClock
                    | wp_presentation_feedback::Kind::HwCompletion,
            )
        } else {
            (self.clock.now(), wp_presentation_feedback::Kind::Vsync)
        };

        let vblank_remaining_time = surface
            .last_presentation_time
            .map(|last_presentation_time| frame_duration.saturating_sub(Time::elapsed(&last_presentation_time, clock)));

        if let Some(vblank_remaining_time) = vblank_remaining_time
            && vblank_remaining_time > frame_duration / 2
        {
            static WARN_ONCE: Once = Once::new();
            WARN_ONCE.call_once(|| warn!("display running faster than expected, throttling vblanks and disabling HwClock"));
            let throttled_time = tp.map(|tp| tp.saturating_add(vblank_remaining_time)).unwrap_or(Duration::ZERO);
            let throttled_metadata = DrmEventMetadata {
                sequence: seq,
                time: DrmEventTime::Monotonic(throttled_time),
            };
            let timer_token = self
                .handle
                .insert_source(Timer::from_duration(vblank_remaining_time), move |_, _, state| {
                    state.frame_finish(dev_id, crtc, &mut Some(throttled_metadata));
                    TimeoutAction::Drop
                })
                .expect("failed to register vblank throttle timer");
            surface.vblank_throttle_timer = Some(timer_token);
            return;
        }
        surface.last_presentation_time = Some(clock);

        let submit_result = surface.drm_output.frame_submitted().map_err(Into::<SwapBuffersError>::into);

        let schedule_render = match submit_result {
            Ok(user_data) => {
                if let Some(mut feedback) = user_data.flatten() {
                    feedback.presented(clock, Refresh::fixed(frame_duration), seq as u64, flags);
                }

                true
            }
            Err(err) => {
                warn!("Error during rendering: {:?}", err);
                match err {
                    SwapBuffersError::AlreadySwapped => true,
                    // If the device has been deactivated do not reschedule, this will be done
                    // by session resume
                    SwapBuffersError::TemporaryFailure(err)
                        if matches!(err.downcast_ref::<DrmError>(), Some(&DrmError::DeviceInactive)) =>
                    {
                        false
                    }
                    SwapBuffersError::TemporaryFailure(err) => matches!(
                        err.downcast_ref::<DrmError>(),
                        Some(DrmError::Access(DrmAccessError {
                            source,
                            ..
                        })) if source.kind() == io::ErrorKind::PermissionDenied
                    ),
                    SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {err}"),
                }
            }
        };

        if schedule_render {
            let next_frame_target = clock + frame_duration;

            // What are we trying to solve by introducing a delay here:
            //
            // Basically it is all about latency of client provided buffers.
            // A client driven by frame callbacks will wait for a frame callback
            // to repaint and submit a new buffer. As we send frame callbacks
            // as part of the repaint in the compositor the latency would always
            // be approx. 2 frames. By introducing a delay before we repaint in
            // the compositor we can reduce the latency to approx. 1 frame + the
            // remaining duration from the repaint to the next VBlank.
            //
            // With the delay it is also possible to further reduce latency if
            // the client is driven by presentation feedback. As the presentation
            // feedback is directly sent after a VBlank the client can submit a
            // new buffer during the repaint delay that can hit the very next
            // VBlank, thus reducing the potential latency to below one frame.
            //
            // Choosing a good delay is a topic on its own so we just implement
            // a simple strategy here. We just split the duration between two
            // VBlanks into two steps, one for the client repaint and one for the
            // compositor repaint. Theoretically the repaint in the compositor should
            // be faster so we give the client a bit more time to repaint. On a typical
            // modern system the repaint in the compositor should not take more than 2ms
            // so this should be safe for refresh rates up to at least 120 Hz. For 120 Hz
            // this results in approx. 3.33ms time for repainting in the compositor.
            // A too big delay could result in missing the next VBlank in the compositor.
            //
            // A more complete solution could work on a sliding window analyzing past repaints
            // and do some prediction for the next repaint.
            let repaint_delay = Duration::from_secs_f64(frame_duration.as_secs_f64() * 0.6f64);

            let timer = if surface
                .render_node
                .map(|render_node| render_node != self.backend_data.primary_gpu)
                .unwrap_or(true)
            {
                // However, if we need to do a copy, that might not be enough.
                // (And without actual comparision to previous frames we cannot really know.)
                // So lets ignore that in those cases to avoid thrashing performance.
                trace!("scheduling repaint timer immediately on {:?}", crtc);
                Timer::immediate()
            } else {
                trace!("scheduling repaint timer with delay {:?} on {:?}", repaint_delay, crtc);
                Timer::from_duration(repaint_delay)
            };

            self.handle
                .insert_source(timer, move |_, _, state| {
                    state.render(dev_id, Some(crtc), next_frame_target);
                    TimeoutAction::Drop
                })
                .expect("failed to schedule frame timer");
        }
    }

    // If crtc is `Some()`, render it, else render all crtcs
    pub(super) fn render(&mut self, node: DrmNode, crtc: Option<crtc::Handle>, frame_target: Time<Monotonic>) {
        let device_backend = match self.backend_data.backends.get_mut(&node) {
            Some(backend) => backend,
            None => {
                error!("Trying to render on non-existent backend {}", node);
                return;
            }
        };

        if let Some(crtc) = crtc {
            if let Err(RenderFailure::Error(err)) = self.render_surface(node, crtc, frame_target) {
                error!("Failed to render surface: {err}");
            }
        } else {
            let crtcs: Vec<_> = device_backend.surfaces.keys().copied().collect();
            for crtc in crtcs {
                if let Err(RenderFailure::Error(err)) = self.render_surface(node, crtc, frame_target) {
                    error!("Failed to render surface to crtc {crtc:?}: {err}");
                }
            }
        };
    }

    pub(super) fn render_surface(&mut self, node: DrmNode, crtc: crtc::Handle, frame_target: Time<Monotonic>) -> Result<(), RenderFailure> {
        profiling::scope!("render_surface", &format!("{crtc:?}"));

        let output = self
            .workspace_manager
            .active_workspace()
            .outputs()
            .find(|o| o.user_data().get::<UdevOutputId>() == Some(&UdevOutputId { device_id: node, crtc }))
            .cloned()
            .ok_or(RenderFailure::NotNeeded)?;

        self.pre_repaint(&output, frame_target);

        let device = self.backend_data.backends.get_mut(&node).ok_or(RenderFailure::NotNeeded)?;
        let surface = device.surfaces.get_mut(&crtc).ok_or(RenderFailure::NotNeeded)?;

        let start = Instant::now();

        // TODO get scale from the rendersurface when supporting HiDPI
        let frame = self.backend_data.pointer_image.get_image(1 /*scale*/, self.clock.now().into());

        let primary_gpu = self.backend_data.primary_gpu;
        let render_node = surface.render_node.unwrap_or(primary_gpu);
        let mut renderer = if primary_gpu == render_node {
            self.backend_data.gpus.single_renderer(&render_node)
        } else {
            let format = surface.drm_output.format();
            self.backend_data.gpus.renderer(&primary_gpu, &render_node, format)
        }
        .context("Failed to find renderer for surface")
        .map_err(RenderFailure::Error)?;

        let pointer_images = &mut self.backend_data.pointer_images;
        let pointer_image = pointer_images
            .iter()
            .find_map(|(image, texture)| if image == &frame { Some(texture.clone()) } else { None })
            .unwrap_or_else(|| {
                let buffer = MemoryRenderBuffer::from_slice(
                    &frame.pixels_rgba,
                    Fourcc::Argb8888,
                    (frame.width as i32, frame.height as i32),
                    1,
                    Transform::Normal,
                    None,
                );
                pointer_images.push((frame, buffer.clone()));
                buffer
            });

        let result = render_surface(
            surface,
            &mut renderer,
            self.workspace_manager.active_workspace(),
            &output,
            self.pointer.current_location(),
            &pointer_image,
            &mut self.backend_data.pointer_element,
            &self.dnd_icon,
            &mut self.cursor_status,
            self.show_window_preview,
        );
        let reschedule = match result {
            Ok((has_rendered, states)) => {
                let dmabuf_feedback = surface.dmabuf_feedback.clone();
                self.post_repaint(&output, frame_target, dmabuf_feedback, &states);
                !has_rendered
            }
            Err(err) => {
                warn!("Error during rendering: {:#?}", err);
                match err {
                    SwapBuffersError::AlreadySwapped => false,
                    SwapBuffersError::TemporaryFailure(err) => match err.downcast_ref::<DrmError>() {
                        Some(DrmError::DeviceInactive) => true,
                        Some(DrmError::Access(DrmAccessError { source, .. })) => source.kind() == io::ErrorKind::PermissionDenied,
                        _ => false,
                    },
                    SwapBuffersError::ContextLost(err) => match err.downcast_ref::<DrmError>() {
                        Some(DrmError::TestFailed(_)) => {
                            // reset the complete state, disabling all connectors and planes in case we hit a test failed
                            // most likely we hit this after a tty switch when a foreign master changed CRTC <-> connector bindings
                            // and we run in a mismatch
                            device
                                .drm_output_manager
                                .device_mut()
                                .reset_state()
                                .expect("failed to reset drm device");
                            true
                        }
                        _ => panic!("Rendering loop lost: {err}"),
                    },
                }
            }
        };

        if reschedule {
            if let Some(output_refresh) = output.current_mode().map(|mode| mode.refresh) {
                // If reschedule is true we either hit a temporary failure or more likely rendering
                // did not cause any damage on the output. In this case we just re-schedule a repaint
                // after approx. one frame to re-test for damage.
                let next_frame_target = frame_target + Duration::from_millis(1_000_000 / output_refresh as u64);
                let reschedule_timeout = Duration::from(next_frame_target).saturating_sub(self.clock.now().into());
                trace!("reschedule repaint timer with delay {:?} on {:?}", reschedule_timeout, crtc,);
                let timer = Timer::from_duration(reschedule_timeout);
                self.handle
                    .insert_source(timer, move |_, _, state| {
                        state.render(node, Some(crtc), next_frame_target);
                        TimeoutAction::Drop
                    })
                    .map_err(|err| RenderFailure::Error(anyhow!("Failed to schedule frame timer: {err}")))?;
            }
        } else {
            let elapsed = start.elapsed();
            tracing::trace!(?elapsed, "rendered surface");
        }

        profiling::finish_frame!();

        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
#[profiling::function]
fn render_surface<'a>(
    surface: &'a mut SurfaceData,
    renderer: &mut UdevRenderer<'a>,
    space: &Space<WindowElement>,
    output: &Output,
    pointer_location: Point<f64, Logical>,
    pointer_image: &MemoryRenderBuffer,
    pointer_element: &mut PointerElement,
    dnd_icon: &Option<DndIcon>,
    cursor_status: &mut CursorImageStatus,
    show_window_preview: bool,
) -> Result<(bool, RenderElementStates), SwapBuffersError> {
    let output_geometry = space.output_geometry(output).unwrap();
    let scale = Scale::from(output.current_scale().fractional_scale());

    let mut custom_elements: Vec<CustomRenderElements<_>> = Vec::new();

    if output_geometry.to_f64().contains(pointer_location) {
        let cursor_hotspot = if let &mut CursorImageStatus::Surface(ref surface) = cursor_status {
            compositor::with_states(surface, |states| {
                states
                    .data_map
                    .get::<Mutex<CursorImageAttributes>>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .hotspot
            })
        } else {
            (0, 0).into()
        };
        let cursor_pos = pointer_location - output_geometry.loc.to_f64();

        // set cursor
        pointer_element.set_buffer(pointer_image.clone());

        // draw the cursor as relevant
        {
            // reset the cursor if the surface is no longer alive
            let mut reset = false;
            if let CursorImageStatus::Surface(ref surface) = *cursor_status {
                reset = !surface.alive();
            }
            if reset {
                *cursor_status = CursorImageStatus::default_named();
            }

            pointer_element.set_status(cursor_status.clone());
        }

        custom_elements.extend(pointer_element.render_elements(
            renderer,
            (cursor_pos - cursor_hotspot.to_f64()).to_physical(scale).to_i32_round(),
            scale,
            1.0,
        ));

        // draw the dnd icon if applicable
        {
            if let Some(icon) = dnd_icon.as_ref() {
                let dnd_icon_pos = (cursor_pos + icon.offset.to_f64()).to_physical(scale).to_i32_round();
                if icon.surface.alive() {
                    custom_elements.extend(AsRenderElements::<UdevRenderer<'a>>::render_elements(
                        &SurfaceTree::from_surface(&icon.surface),
                        renderer,
                        dnd_icon_pos,
                        scale,
                        1.0,
                    ));
                }
            }
        }
    }

    #[cfg(feature = "debug")]
    if let Some(debug) = &mut surface.debug {
        custom_elements.push(debug.update());
    }

    let (elements, clear_color) = output_elements(output, space, custom_elements, renderer, show_window_preview);

    let frame_mode = if surface.disable_direct_scanout {
        FrameFlags::empty()
    } else {
        FrameFlags::DEFAULT
    };
    let (rendered, states) = surface
        .drm_output
        .render_frame(renderer, &elements, clear_color, frame_mode)
        .map(|render_frame_result| {
            #[cfg(feature = "renderer_sync")]
            if let PrimaryPlaneElement::Swapchain(element) = render_frame_result.primary_element {
                element.sync.wait();
            }
            (!render_frame_result.is_empty, render_frame_result.states)
        })
        .map_err(|err| match err {
            smithay::backend::drm::compositor::RenderFrameError::PrepareFrame(err) => SwapBuffersError::from(err),
            smithay::backend::drm::compositor::RenderFrameError::RenderFrame(OutputDamageTrackerError::Rendering(err)) => {
                SwapBuffersError::from(err)
            }
            _ => unreachable!(),
        })?;

    update_primary_scanout_output(space, output, dnd_icon, cursor_status, &states);

    if rendered {
        let output_presentation_feedback = take_presentation_feedback(output, space, &states);
        surface
            .drm_output
            .queue_frame(Some(output_presentation_feedback))
            .map_err(Into::<SwapBuffersError>::into)?;
    }

    Ok((rendered, states))
}
