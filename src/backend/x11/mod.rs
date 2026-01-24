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

use std::{collections::HashSet, sync::Mutex, time::Duration};

use crate::{
    backend::Backend,
    drawing::*,
    render::*,
    state::{Xfwl4State, take_presentation_feedback},
    ui::{FromUiMessage, ToUiMessage},
};
use anyhow::{Context, anyhow};
use glib::Sender;
#[cfg(feature = "egl")]
use smithay::backend::renderer::ImportEgl;

use smithay::{
    backend::{
        allocator::{
            Modifier,
            dmabuf::{Dmabuf, DmabufAllocator},
            gbm::{GbmAllocator, GbmBufferFlags},
            vulkan::{ImageUsageFlags, VulkanAllocator},
        },
        egl::{EGLContext, EGLDisplay},
        renderer::{Bind, ImportDma, ImportMemWl, damage::OutputDamageTracker, element::AsRenderElements, gles::GlesRenderer},
        vulkan::{Instance, PhysicalDevice, version::Version},
        x11::{Window, WindowBuilder, X11Backend, X11Event, X11Surface},
    },
    delegate_dmabuf,
    input::{
        keyboard::LedState,
        pointer::{CursorImageAttributes, CursorImageStatus},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        ash::ext,
        calloop::{EventLoop, channel},
        gbm,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::{Display, protocol::wl_surface},
    },
    utils::{DeviceFd, IsAlive, Scale},
    wayland::{
        compositor,
        dmabuf::{DmabufFeedback, DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        presentation::Refresh,
    },
};
use tracing::{error, info, trace, warn};
use x11rb::{connection::Connection, protocol::dri3::ConnectionExt};

pub const OUTPUT_NAME: &str = "x11";

pub struct X11Config {
    pub disable_vulkan: bool,
}

pub struct X11Data {
    render: bool,
    render_trigger: channel::Sender<()>,
    output: Output,
    mode: Mode,
    // FIXME: If GlesRenderer is dropped before X11Surface, then the MakeCurrent call inside Gles2Renderer will
    // fail because the X11Surface is keeping gbm alive.
    renderer: GlesRenderer,
    damage_tracker: OutputDamageTracker,
    window: Window,
    surface: X11Surface,
    pointer_element: PointerElement,
    dmabuf_state: DmabufState,
    _dmabuf_global: DmabufGlobal,
    _dmabuf_default_feedback: DmabufFeedback,
    #[cfg(feature = "debug")]
    debug: Option<crate::debug::RenderDebug<smithay::backend::renderer::gles::GlesTexture>>,
}

impl DmabufHandler for Xfwl4State<X11Data> {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.backend_data.dmabuf_state
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        if self.backend_data.renderer.import_dmabuf(&dmabuf, None).is_ok() {
            let _ = notifier.successful::<Xfwl4State<X11Data>>();
        } else {
            notifier.failed();
        }
    }
}
delegate_dmabuf!(Xfwl4State<X11Data>);

impl Backend for X11Data {
    fn backend_type(&self) -> super::BackendType {
        super::BackendType::X11
    }
    fn seat_name(&self) -> String {
        "x11".to_owned()
    }
    fn reset_buffers(&mut self, _output: &Output) {
        self.surface.reset_buffers();
    }
    fn early_import(&mut self, _surface: &wl_surface::WlSurface) {}
    fn update_led_state(&mut self, _led_state: LedState) {}
}

pub fn init(
    config: X11Config,
    from_ui_channel_rx: channel::Channel<FromUiMessage>,
    to_ui_channel_tx: Sender<ToUiMessage>,
) -> anyhow::Result<(EventLoop<'static, Xfwl4State<X11Data>>, Xfwl4State<X11Data>)> {
    let event_loop = EventLoop::try_new().context("Failed to create event loop")?;
    let display = Display::new().context("Failed to create Wayland display")?;

    let backend = X11Backend::new().context("Failed to initilize X11 backend")?;
    let handle = backend.handle();

    // Obtain the DRM node the X server uses for direct rendering.
    let (node, fd) = handle.drm_node().context("Could not get DRM node used by X server")?;

    // Create the gbm device for buffer allocation.
    let device = gbm::Device::new(DeviceFd::from(fd)).context("Failed to create gbm device")?;
    // Initialize EGL using the GBM device.
    let egl = unsafe { EGLDisplay::new(device.clone()).context("Failed to create EGLDisplay") }?;
    // Create the OpenGL context
    let context = EGLContext::new(&egl).context("Failed to create EGLContext")?;

    let conn = handle.connection();
    let screen = &conn.setup().roots[handle.screen()];
    let window_size = (
        (screen.width_in_pixels as f64 * 0.8) as u16,
        (screen.height_in_pixels as f64 * 0.8) as u16,
    );

    let window = WindowBuilder::new()
        .title("Xfwl4")
        .size(window_size.into())
        .build(&handle)
        .context("Failed to create first window")?;

    let vulkan_allocator = if !config.disable_vulkan {
        Instance::new(Version::VERSION_1_2, None)
            .ok()
            .and_then(|instance| {
                PhysicalDevice::enumerate(&instance).ok().and_then(|devices| {
                    devices
                        .filter(|phd| phd.has_device_extension(ext::physical_device_drm::NAME))
                        .find(|phd| phd.primary_node().unwrap() == Some(node) || phd.render_node().unwrap() == Some(node))
                })
            })
            .and_then(|physical_device| {
                VulkanAllocator::new(&physical_device, ImageUsageFlags::COLOR_ATTACHMENT | ImageUsageFlags::SAMPLED).ok()
            })
    } else {
        None
    };

    let modifiers = conn
        .dri3_get_supported_modifiers(window.id(), window.depth(), 32)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| {
            if !reply.window_modifiers.is_empty() {
                Some(reply.window_modifiers)
            } else if !reply.screen_modifiers.is_empty() {
                Some(reply.screen_modifiers)
            } else {
                None
            }
        })
        .and_then(|dri3_modifiers| {
            let dri3_modifiers = dri3_modifiers.into_iter().collect::<HashSet<_>>();

            let modifiers = context
                .dmabuf_render_formats()
                .iter()
                .filter_map(|format| {
                    let modifier_value = u64::from(format.modifier);
                    dri3_modifiers.contains(&modifier_value).then_some(format.modifier)
                })
                .collect::<Vec<_>>();

            (!modifiers.is_empty()).then_some(modifiers)
        })
        .unwrap_or_else(|| {
            // Fall back to something safe
            std::iter::once(Modifier::Linear).collect::<Vec<_>>()
        });

    let surface = match vulkan_allocator {
        // Create the surface for the window.
        Some(vulkan_allocator) => handle
            .create_surface(&window, DmabufAllocator(vulkan_allocator), modifiers.into_iter())
            .context("Failed to create X11 surface")?,
        None => handle
            .create_surface(
                &window,
                DmabufAllocator(GbmAllocator::new(device, GbmBufferFlags::RENDERING)),
                modifiers.into_iter(),
            )
            .context("Failed to create X11 surface")?,
    };

    #[cfg_attr(not(feature = "egl"), allow(unused_mut))]
    let mut renderer = unsafe { GlesRenderer::new(context) }.context("Failed to initialize renderer")?;

    #[cfg(feature = "egl")]
    if renderer.bind_wl_display(&display.handle()).is_ok() {
        info!("EGL hardware-acceleration enabled");
    }

    let dmabuf_formats = renderer.dmabuf_formats();
    let dmabuf_default_feedback = DmabufFeedbackBuilder::new(node.dev_id(), dmabuf_formats).build().unwrap();
    let mut dmabuf_state = DmabufState::new();
    let dmabuf_global =
        dmabuf_state.create_global_with_default_feedback::<Xfwl4State<X11Data>>(&display.handle(), &dmabuf_default_feedback);

    let size = {
        let s = window.size();

        (s.w as i32, s.h as i32).into()
    };

    let mode = Mode { size, refresh: 60_000 };

    #[cfg(feature = "debug")]
    let debug = crate::debug::BackendDebug::new(&mut renderer).map(|bd| crate::debug::RenderDebug::new(&bd));

    let output = Output::new(
        OUTPUT_NAME.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Smithay".into(),
            model: "X11".into(),
            serial_number: "Unknown".into(),
        },
    );
    let _global = output.create_global::<Xfwl4State<X11Data>>(&display.handle());
    output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
    output.set_preferred(mode);

    let damage_tracker = OutputDamageTracker::from_output(&output);

    let (tx, rx) = channel::channel();

    let data = X11Data {
        render: true,
        render_trigger: tx,
        output,
        mode,
        window,
        surface,
        renderer,
        damage_tracker,
        pointer_element: PointerElement::default(),
        dmabuf_state,
        _dmabuf_global: dmabuf_global,
        _dmabuf_default_feedback: dmabuf_default_feedback,
        #[cfg(feature = "debug")]
        debug,
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
    state.shm_state.update_formats(state.backend_data.renderer.shm_formats());

    for workspace in state.workspace_manager.workspaces_mut() {
        workspace.map_output(&state.backend_data.output, (0, 0));
    }

    event_loop
        .handle()
        .insert_source(rx, |_, _, state| {
            if let Err(err) = state.render() {
                error!("Rendering failed: {err}");
            }
        })
        .map_err(|err| anyhow!("Failed to register rendering channel into event loop: {err}"))?;

    let output = state.backend_data.output.clone();
    event_loop
        .handle()
        .insert_source(backend, move |event, _, data| match event {
            X11Event::CloseRequested { .. } => {
                data.shutdown();
            }
            X11Event::Resized { new_size, .. } => {
                let size = { (new_size.w as i32, new_size.h as i32).into() };

                data.backend_data.mode = Mode { size, refresh: 60_000 };
                output.delete_mode(output.current_mode().unwrap());
                output.change_current_state(Some(data.backend_data.mode), None, None, None);
                output.set_preferred(data.backend_data.mode);
                crate::shell::fixup_positions(&mut data.workspace_manager, data.pointer.current_location());

                data.backend_data.render = true;
                data.backend_data.render_trigger.send(()).unwrap();
            }
            X11Event::PresentCompleted { .. } | X11Event::Refresh { .. } => {
                data.backend_data.render = true;
                data.backend_data.render_trigger.send(()).unwrap();
            }
            X11Event::Input { event, .. } => data.process_input_event_windowed(event, OUTPUT_NAME),
            X11Event::Focus { focused: false, .. } => {
                data.release_all_keys();
            }
            _ => {}
        })
        .map_err(|err| anyhow!("Failed to insert X11 Backend into event loop: {err}"))?;

    Ok((event_loop, state))
}

impl Xfwl4State<X11Data> {
    fn render(&mut self) -> anyhow::Result<()> {
        if self.backend_data.render {
            profiling::scope!("render_frame");

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

            let backend_data = &mut self.backend_data;
            let (mut buffer, age) = backend_data.surface.buffer().context("gbm device was destroyed")?;
            let mut fb = backend_data.renderer.bind(&mut buffer).context("Failed to bind buffer")?;

            #[cfg(feature = "debug")]
            if let Some(renderdoc) = self.renderdoc.as_mut() {
                renderdoc.start_frame_capture(backend_data.renderer.egl_context().get_context_handle(), std::ptr::null());
            }

            let mut elements: Vec<CustomRenderElements<GlesRenderer>> = Vec::new();

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

            let scale = Scale::from(output.current_scale().fractional_scale());
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

            backend_data.pointer_element.set_status(self.cursor_status.clone());
            elements.extend(backend_data.pointer_element.render_elements(
                &mut backend_data.renderer,
                (cursor_pos - cursor_hotspot.to_f64()).to_physical(scale).to_i32_round(),
                scale,
                1.0,
            ));

            // draw the dnd icon if any
            if let Some(icon) = self.dnd_icon.as_ref() {
                let dnd_icon_pos = (cursor_pos + icon.offset.to_f64()).to_physical(scale).to_i32_round();
                if icon.surface.alive() {
                    elements.extend(AsRenderElements::<GlesRenderer>::render_elements(
                        &smithay::desktop::space::SurfaceTree::from_surface(&icon.surface),
                        &mut backend_data.renderer,
                        dnd_icon_pos,
                        scale,
                        1.0,
                    ));
                }
            }

            #[cfg(feature = "debug")]
            elements.extend(fps_element);

            let render_res = render_output(
                &output,
                self.workspace_manager.active_workspace(),
                elements,
                &mut backend_data.renderer,
                &mut fb,
                &mut backend_data.damage_tracker,
                age.into(),
                self.show_window_preview,
            );

            match render_res {
                Ok(render_output_result) => {
                    trace!("Finished rendering");
                    let submitted = if let Err(err) = backend_data.surface.submit() {
                        backend_data.surface.reset_buffers();
                        warn!("Failed to submit buffer: {}. Retrying", err);
                        false
                    } else {
                        true
                    };

                    let states = render_output_result.states;
                    #[cfg(feature = "debug")]
                    let rendered = render_output_result.damage.is_some();
                    if render_output_result.damage.is_some() {
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

                    #[cfg(feature = "debug")]
                    if rendered {
                        if let Some(renderdoc) = self.renderdoc.as_mut() {
                            renderdoc.end_frame_capture(self.backend_data.renderer.egl_context().get_context_handle(), std::ptr::null());
                        }
                    } else if let Some(renderdoc) = self.renderdoc.as_mut() {
                        renderdoc.discard_frame_capture(self.backend_data.renderer.egl_context().get_context_handle(), std::ptr::null());
                    }

                    self.backend_data.render = !submitted;

                    // Send frame events so that client start drawing their next frame
                    self.post_repaint(&output, frame_target, None, &states);
                }
                Err(err) => {
                    #[cfg(feature = "debug")]
                    if let Some(renderdoc) = self.renderdoc.as_mut() {
                        renderdoc.discard_frame_capture(backend_data.renderer.egl_context().get_context_handle(), std::ptr::null());
                    }

                    backend_data.surface.reset_buffers();
                    error!("Rendering error: {}", err);
                    // TODO: convert RenderError into SwapBuffersError and skip temporary (will retry) and panic on ContextLost or recreate
                }
            }

            self.backend_data.window.set_cursor_visible(cursor_visible);
            profiling::finish_frame!();
        }

        Ok(())
    }
}
