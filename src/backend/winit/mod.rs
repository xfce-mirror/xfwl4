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

use std::{collections::HashMap, sync::Mutex, time::Duration};

use anyhow::{Context, anyhow};
use glib::Sender;
#[cfg(feature = "egl")]
use smithay::backend::renderer::ImportEgl;
#[cfg(feature = "debug")]
use smithay::reexports::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use smithay::{
    backend::{
        SwapBuffersError,
        allocator::{Fourcc, Modifier, dmabuf::Dmabuf},
        drm::DrmNode,
        egl::EGLDevice,
        renderer::{
            Bind, ImportDma, ImportMemWl,
            damage::{Error as OutputDamageTrackerError, OutputDamageTracker},
            element::AsRenderElements,
            gles::{GlesError, GlesRenderer, GlesTexture},
        },
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    delegate_dmabuf,
    input::{
        keyboard::LedState,
        pointer::{CursorImageAttributes, CursorImageStatus},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{EventLoop, channel},
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::{Display, backend::GlobalId, protocol::wl_surface},
        winit::dpi::LogicalSize,
    },
    utils::{IsAlive, Scale, Size, Transform},
    wayland::{
        compositor,
        dmabuf::{DmabufFeedback, DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        image_copy_capture::DmabufConstraints,
        presentation::Refresh,
    },
};
use tracing::{error, info, warn};

use crate::{
    backend::Backend,
    config::OutputConfigChange,
    drawing::*,
    render::*,
    state::{Xfwl4State, take_presentation_feedback},
    ui::{FromUiMessage, ToUiMessage},
    util::OutputImageCopyExt,
};

mod renderer;

pub use renderer::WinitRenderer;

pub const OUTPUT_NAME: &str = "winit";

pub struct WinitData {
    backend: WinitGraphicsBackend<GlesRenderer>,
    damage_tracker: OutputDamageTracker,
    dmabuf_state: (DmabufState, DmabufGlobal, Option<DmabufFeedback>),
    full_redraw: u8,
    output_global: GlobalId,
    output: Output,
    pointer_element: PointerElement,
    #[cfg(feature = "debug")]
    debug: Option<crate::debug::RenderDebug<smithay::backend::renderer::gles::GlesTexture>>,
}

impl DmabufHandler for Xfwl4State<WinitData> {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.backend_data.dmabuf_state.0
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        if self.backend_data.backend.renderer().import_dmabuf(&dmabuf, None).is_ok() {
            let _ = notifier.successful::<Xfwl4State<WinitData>>();
        } else {
            notifier.failed();
        }
    }
}
delegate_dmabuf!(Xfwl4State<WinitData>);

impl Backend for WinitData {
    type RendererError = GlesError;
    type RendererTextureId = GlesTexture;
    type Renderer<'a>
        = WinitRenderer<'a>
    where
        Self: 'a;

    type GammaControlData = ();

    fn backend_type(&self) -> super::BackendType {
        super::BackendType::Winit
    }
    fn seat_name(&self) -> String {
        String::from("winit")
    }
    fn reset_buffers(&mut self, _output: &Output) {
        self.full_redraw = 4;
    }
    fn early_import(&mut self, _surface: &wl_surface::WlSurface) {}
    fn update_led_state(&mut self, _led_state: LedState) {}

    fn renderer(&mut self, #[cfg(feature = "udev")] _node: Option<smithay::backend::drm::DrmNode>) -> anyhow::Result<Self::Renderer<'_>> {
        Ok(WinitRenderer(self.backend.renderer()))
    }

    fn dmabuf_constraints(&mut self, node: Option<DrmNode>) -> Option<DmabufConstraints> {
        #[cfg(feature = "egl")]
        {
            let node = node.or_else(|| {
                EGLDevice::device_for_display(self.backend.renderer().egl_context().display())
                    .ok()
                    .and_then(|dev| dev.try_get_render_node().ok().flatten())
            })?;
            let formats = Bind::<Dmabuf>::supported_formats(self.backend.renderer())?
                .iter()
                .fold(HashMap::<Fourcc, Vec<Modifier>>::new(), |mut map, fmt| {
                    map.entry(fmt.code).or_default().push(fmt.modifier);
                    map
                })
                .into_iter()
                .collect();
            Some(DmabufConstraints { node, formats })
        }
        #[cfg(not(feature = "egl"))]
        {
            let _ = node;
            None
        }
    }

    fn set_cursor(&mut self, _cursor: crate::cursor::Cursor) {
        // TODO
    }

    fn outputs(&self) -> Vec<(GlobalId, Output)> {
        vec![(self.output_global.clone(), self.output.clone())]
    }

    fn apply_output_config_change(&mut self, _output: &Output, config: OutputConfigChange) -> anyhow::Result<()> {
        let new_mode = if let Some(Some(new_mode)) = config.current_mode {
            if let Some(new_size) = self
                .backend
                .window()
                .request_inner_size(LogicalSize::new(new_mode.size.w, new_mode.size.h))
            {
                let new_mode = Mode {
                    size: Size::new(new_size.width as i32, new_size.height as i32),
                    refresh: new_mode.refresh,
                };
                self.output.set_preferred(new_mode);
                Some(new_mode)
            } else {
                // New size will arrive in a Resize event; our handler will take care of it.
                None
            }
        } else {
            None
        };

        self.output
            .change_current_state(new_mode, config.transform, config.scale, config.location);
        Ok(())
    }

    fn set_output_gamma(&mut self, _output: Output, _data: &Self::GammaControlData, _red: &[u16], _green: &[u16], _blue: &[u16]) -> bool {
        // not supported
        false
    }
}

pub fn init(
    from_ui_channel_rx: channel::Channel<FromUiMessage>,
    to_ui_channel_tx: Sender<ToUiMessage>,
) -> anyhow::Result<(EventLoop<'static, Xfwl4State<WinitData>>, Xfwl4State<WinitData>)> {
    let event_loop = EventLoop::try_new().context("Failed to create event loop")?;
    let display = Display::new().context("Failed to create Wayland display")?;

    let (mut backend, winit) = winit::init::<GlesRenderer>().map_err(|err| anyhow!("Failed to initialize Winit backend: {err}"))?;
    let size = backend.window_size();

    let mode = Mode { size, refresh: 60_000 };
    let output = Output::new(
        OUTPUT_NAME.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Xfce".into(),
            model: "Winit".into(),
            serial_number: "Unknown".into(),
        },
    );
    let global = output.create_global::<Xfwl4State<WinitData>>(&display.handle());
    output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some((0, 0).into()));
    output.set_preferred(mode);

    #[cfg(feature = "debug")]
    let debug = crate::debug::BackendDebug::new(backend.renderer()).map(|bd| crate::debug::RenderDebug::new(&bd));

    let render_node =
        EGLDevice::device_for_display(backend.renderer().egl_context().display()).and_then(|device| device.try_get_render_node());

    let dmabuf_default_feedback = match render_node {
        Ok(Some(node)) => {
            let dmabuf_formats = backend.renderer().dmabuf_formats();
            let dmabuf_default_feedback = DmabufFeedbackBuilder::new(node.dev_id(), dmabuf_formats)
                .build()
                .context("Failed to build default DMABUF feedback")?;
            Some(dmabuf_default_feedback)
        }
        Ok(None) => {
            warn!("failed to query render node, dmabuf will use v3");
            None
        }
        Err(err) => {
            warn!(?err, "failed to egl device for display, dmabuf will use v3");
            None
        }
    };

    // if we failed to build dmabuf feedback we fall back to dmabuf v3
    // Note: egl on Mesa requires either v4 or wl_drm (initialized with bind_wl_display)
    let dmabuf_state = if let Some(default_feedback) = dmabuf_default_feedback {
        let mut dmabuf_state = DmabufState::new();
        let dmabuf_global = dmabuf_state.create_global_with_default_feedback::<Xfwl4State<WinitData>>(&display.handle(), &default_feedback);
        (dmabuf_state, dmabuf_global, Some(default_feedback))
    } else {
        let dmabuf_formats = backend.renderer().dmabuf_formats();
        let mut dmabuf_state = DmabufState::new();
        let dmabuf_global = dmabuf_state.create_global::<Xfwl4State<WinitData>>(&display.handle(), dmabuf_formats);
        (dmabuf_state, dmabuf_global, None)
    };

    #[cfg(feature = "egl")]
    if backend.renderer().bind_wl_display(&display.handle()).is_ok() {
        info!("EGL hardware-acceleration enabled");
    };

    let data = {
        let damage_tracker = OutputDamageTracker::from_output(&output);

        WinitData {
            backend,
            damage_tracker,
            dmabuf_state,
            full_redraw: 0,
            output_global: global.clone(),
            output: output.clone(),
            pointer_element: PointerElement::default(),
            #[cfg(feature = "debug")]
            debug,
        }
    };
    let mut state = Xfwl4State::init(
        display,
        event_loop.handle(),
        event_loop.get_signal(),
        data,
        from_ui_channel_rx,
        to_ui_channel_tx,
        true,
    );
    state.shm_state.update_formats(state.backend_data.backend.renderer().shm_formats());

    state.output_created(global, &output);

    event_loop
        .handle()
        .insert_source(winit, |event, _, state| match event {
            WinitEvent::Resized { size, .. } => {
                let mode = Mode { size, refresh: 60_000 };
                let output = state.backend_data.output.clone();
                output.change_current_state(Some(mode), None, None, None);
                output.set_preferred(mode);
                state.output_changed(&output);
            }
            WinitEvent::Input(event) => state.process_input_event_windowed(event, OUTPUT_NAME),
            WinitEvent::Redraw => state.render(),
            WinitEvent::Focus(false) => state.release_all_keys(),
            WinitEvent::CloseRequested => state.shutdown(),
            _ => (),
        })
        .map_err(|err| anyhow!("Failed to register winit event source: {err}"))?;

    Ok((event_loop, state))
}

impl Xfwl4State<WinitData> {
    fn render(&mut self) {
        let now = self.clock.now();
        let frame_target = now
            + self
                .backend_data
                .output
                .current_mode()
                .map(|mode| Duration::from_secs_f64(1_000f64 / mode.refresh as f64))
                .unwrap_or_default();

        let output = self.backend_data.output.clone();
        self.pre_repaint(&output, frame_target);

        #[cfg(feature = "debug")]
        let fps_element = self.backend_data.debug.as_mut().map(|d| d.update());

        let backend = &mut self.backend_data.backend;

        // draw the cursor as relevant
        // reset the cursor if the surface is no longer alive
        let mut reset = false;
        if let CursorImageStatus::Surface(ref surface) = self.cursor_status {
            reset = !surface.alive();
        }
        if reset {
            self.cursor_status = CursorImageStatus::default_named();
        }
        let cursor_visible = !matches!(self.cursor_status, CursorImageStatus::Surface(_));

        self.backend_data.pointer_element.set_status(self.cursor_status.clone());

        let full_redraw = &mut self.backend_data.full_redraw;
        *full_redraw = full_redraw.saturating_sub(1);
        let workspace = self.workspace_manager.active_workspace_mut();
        let damage_tracker = &mut self.backend_data.damage_tracker;

        let dnd_icon = self.dnd_icon.as_ref();

        let scale = Scale::from(self.backend_data.output.current_scale().fractional_scale());
        let cursor_hotspot = if let CursorImageStatus::Surface(ref surface) = self.cursor_status {
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
        let cursor_pos = self.pointer.current_location();

        #[cfg(feature = "debug")]
        let mut renderdoc = self.renderdoc.as_mut();

        let age = if *full_redraw > 0 { 0 } else { backend.buffer_age().unwrap_or(0) };
        #[cfg(feature = "debug")]
        let window_handle = backend
            .window()
            .window_handle()
            .map(|handle| {
                if let RawWindowHandle::Wayland(handle) = handle.as_raw() {
                    handle.surface.as_ptr()
                } else {
                    std::ptr::null_mut()
                }
            })
            .unwrap_or_else(|_| std::ptr::null_mut());
        let render_res = backend.bind().and_then(|(renderer, mut fb)| {
            #[cfg(feature = "debug")]
            if let Some(renderdoc) = renderdoc.as_mut() {
                renderdoc.start_frame_capture(renderer.egl_context().get_context_handle(), window_handle);
            }

            let mut elements = Vec::<CustomRenderElements<GlesRenderer>>::new();

            elements.extend(self.backend_data.pointer_element.render_elements(
                renderer,
                (cursor_pos - cursor_hotspot.to_f64()).to_physical(scale).to_i32_round(),
                scale,
                1.0,
            ));

            // draw the dnd icon if any
            if let Some(icon) = dnd_icon {
                let dnd_icon_pos = (cursor_pos + icon.offset.to_f64()).to_physical(scale).to_i32_round();
                if icon.surface.alive() {
                    elements.extend(AsRenderElements::<GlesRenderer>::render_elements(
                        &smithay::desktop::space::SurfaceTree::from_surface(&icon.surface),
                        renderer,
                        dnd_icon_pos,
                        scale,
                        1.0,
                    ));
                }
            }

            #[cfg(feature = "debug")]
            elements.extend(fps_element);

            let mut render_view = RenderView {
                ext_session_lock_state: &self.ext_session_lock_state,
                renderer,
            };
            let result = render_view
                .render_output(&self.backend_data.output, workspace, elements, &mut fb, damage_tracker, age)
                .map_err(|err| match err {
                    OutputDamageTrackerError::Rendering(err) => err.into(),
                    _ => unreachable!(),
                });
            if let Some(frames) = output.take_image_copy_frames() {
                render_view.render_image_copy_frames(frames, &self.backend_data.output, workspace, frame_target);
            }
            if let Some(frames) = output.take_wlr_screencopy_frames() {
                render_view.render_wlr_screencopy_frames(frames, &self.backend_data.output, workspace, frame_target);
            }
            result
        });

        match render_res {
            Ok(render_output_result) => {
                let has_rendered = render_output_result.damage.is_some();
                if let Some(damage) = render_output_result.damage
                    && let Err(err) = backend.submit(Some(damage))
                {
                    warn!("Failed to submit buffer: {}", err);
                }

                #[cfg(feature = "debug")]
                if let Some(renderdoc) = renderdoc.as_mut() {
                    renderdoc.end_frame_capture(
                        backend.renderer().egl_context().get_context_handle(),
                        backend
                            .window()
                            .window_handle()
                            .map(|handle| {
                                if let RawWindowHandle::Wayland(handle) = handle.as_raw() {
                                    handle.surface.as_ptr()
                                } else {
                                    std::ptr::null_mut()
                                }
                            })
                            .unwrap_or_else(|_| std::ptr::null_mut()),
                    );
                }

                backend.window().set_cursor_visible(cursor_visible);

                let states = render_output_result.states;
                if has_rendered {
                    let mut output_presentation_feedback =
                        take_presentation_feedback(&self.backend_data.output, self.workspace_manager.active_workspace(), &states);
                    output_presentation_feedback.presented(
                        frame_target,
                        self.backend_data
                            .output
                            .current_mode()
                            .map(|mode| Refresh::fixed(Duration::from_secs_f64(1_000f64 / mode.refresh as f64)))
                            .unwrap_or(Refresh::Unknown),
                        0,
                        wp_presentation_feedback::Kind::Vsync,
                    )
                }

                // Send frame events so that client start drawing their next frame
                self.post_repaint(&output, frame_target, None, &states);
            }
            Err(SwapBuffersError::ContextLost(err)) => {
                #[cfg(feature = "debug")]
                if let Some(renderdoc) = renderdoc.as_mut() {
                    renderdoc.discard_frame_capture(
                        backend.renderer().egl_context().get_context_handle(),
                        backend
                            .window()
                            .window_handle()
                            .map(|handle| {
                                if let RawWindowHandle::Wayland(handle) = handle.as_raw() {
                                    handle.surface.as_ptr()
                                } else {
                                    std::ptr::null_mut()
                                }
                            })
                            .unwrap_or_else(|_| std::ptr::null_mut()),
                    );
                }

                error!("Critical Rendering Error: {}", err);
                self.shutdown();
            }
            Err(err) => warn!("Rendering error: {}", err),
        }

        self.backend_data.backend.window().request_redraw();
    }
}
