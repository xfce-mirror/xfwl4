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

use std::{collections::HashSet, time::Duration};

use crate::{
    backend::{Backend, KeyboardInputEvent, PointerInputEvent, TranslatedInput, build_axis_frame},
    core::{
        config::OutputConfigChange,
        render::*,
        state::{Xfwl4Core, Xfwl4State},
    },
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
        input::{AbsolutePositionEvent, Event, InputEvent, KeyboardKeyEvent, PointerButtonEvent},
        renderer::{
            Bind, ImportDma, ImportMemWl,
            damage::OutputDamageTracker,
            element::RenderElementStates,
            gles::{GlesError, GlesRenderer, GlesTexture},
        },
        vulkan::{Instance, PhysicalDevice, version::Version},
        x11::{Window, WindowBuilder, X11Backend, X11Event, X11Handle, X11Input, X11Surface},
    },
    delegate_dmabuf,
    input::keyboard::LedState,
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        ash::ext,
        calloop::{EventLoop, channel},
        gbm,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::{Display, protocol::wl_surface},
    },
    utils::{DeviceFd, Monotonic, Size, Time},
    wayland::{
        dmabuf::{DmabufFeedback, DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        presentation::Refresh,
    },
};
use tracing::{info, trace, warn};
use x11rb::{
    connection::Connection,
    protocol::{
        dri3::ConnectionExt as _,
        xproto::{ConfigureWindowAux, ConnectionExt as _},
    },
};

mod renderer;

pub use renderer::X11Renderer;

pub const OUTPUT_NAME: &str = "x11";

pub struct X11Config {
    pub disable_vulkan: bool,
}

pub struct X11Data {
    backend_handle: X11Handle,
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
    dmabuf_state: DmabufState,
    _dmabuf_global: DmabufGlobal,
    _dmabuf_default_feedback: DmabufFeedback,
}

impl DmabufHandler for Xfwl4State<X11Data> {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.backend.dmabuf_state
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        if self.backend.renderer.import_dmabuf(&dmabuf, None).is_ok() {
            let _ = notifier.successful::<Xfwl4State<X11Data>>();
        } else {
            notifier.failed();
        }
    }
}
delegate_dmabuf!(Xfwl4State<X11Data>);

impl Backend for X11Data {
    type RendererError = GlesError;
    type RendererTextureId = GlesTexture;
    type Renderer<'a>
        = X11Renderer<'a>
    where
        Self: 'a;

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

    fn renderer(&mut self, #[cfg(feature = "udev")] _node: Option<smithay::backend::drm::DrmNode>) -> anyhow::Result<Self::Renderer<'_>> {
        Ok(X11Renderer(&mut self.renderer))
    }

    fn renderer_for_output(&mut self, output: &Output) -> anyhow::Result<Self::Renderer<'_>> {
        assert!(output == &self.output);
        Ok(X11Renderer(&mut self.renderer))
    }

    #[cfg(any(feature = "udev", feature = "winit"))]
    fn dmabuf_constraints(
        &mut self,
        _node: Option<smithay::backend::drm::DrmNode>,
    ) -> Option<smithay::wayland::image_copy_capture::DmabufConstraints> {
        None
    }

    fn apply_output_config_change(&mut self, _output: &Output, config: OutputConfigChange) -> anyhow::Result<()> {
        let new_mode = if let Some(Some(new_mode)) = config.current_mode {
            let params = ConfigureWindowAux {
                width: Some(new_mode.size.w as u32),
                height: Some(new_mode.size.h as u32),
                x: None,
                y: None,
                border_width: None,
                sibling: None,
                stack_mode: None,
            };

            let conn = self.backend_handle.connection();
            let cookie = conn.configure_window(self.window.id(), &params)?;
            cookie.check().map(|_| {
                let window_size = self.window.size();
                Some(Mode {
                    size: Size::new(window_size.w as i32, window_size.h as i32),
                    refresh: new_mode.refresh,
                })
            })
        } else {
            Ok(None)
        }?;

        self.output
            .change_current_state(new_mode, config.transform, config.scale, config.location);
        Ok(())
    }

    fn switch_vt(&mut self, _num: i32) {
        tracing::info!("VT switching not supported on this backend");
    }
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
    output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
    output.set_preferred(mode);

    let damage_tracker = OutputDamageTracker::from_output(&output);

    let (tx, rx) = channel::channel();

    let data = X11Data {
        backend_handle: handle,
        render: true,
        render_trigger: tx,
        output: output.clone(),
        mode,
        window,
        surface,
        renderer,
        damage_tracker,
        dmabuf_state,
        _dmabuf_global: dmabuf_global,
        _dmabuf_default_feedback: dmabuf_default_feedback,
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
    state.core.update_shm_formats(state.backend.renderer.shm_formats());

    state.output_created(&output);

    event_loop
        .handle()
        .insert_source(rx, |_, _, state| {
            let frame_target = state.core.now()
                + state
                    .backend
                    .output
                    .current_mode()
                    .map(|mode| Duration::from_secs_f64(1_000f64 / mode.refresh as f64))
                    .unwrap_or_default();

            state.render(&state.backend.output.clone(), frame_target, |backend, core| {
                backend.render(core, frame_target)
            });
        })
        .map_err(|err| anyhow!("Failed to register rendering channel into event loop: {err}"))?;

    let output = state.backend.output.clone();
    event_loop
        .handle()
        .insert_source(backend, move |event, _, data| match event {
            X11Event::CloseRequested { .. } => {
                data.shutdown();
            }
            X11Event::Resized { new_size, .. } => {
                let size = { (new_size.w as i32, new_size.h as i32).into() };

                data.backend.mode = Mode { size, refresh: 60_000 };
                output.delete_mode(output.current_mode().unwrap());
                output.change_current_state(Some(data.backend.mode), None, None, None);
                output.set_preferred(data.backend.mode);
                data.output_changed(&output);

                data.backend.render = true;
                data.backend.render_trigger.send(()).unwrap();
            }
            X11Event::PresentCompleted { .. } | X11Event::Refresh { .. } => {
                data.backend.render = true;
                data.backend.render_trigger.send(()).unwrap();
            }
            X11Event::Input { event, .. } => {
                let input = match event {
                    InputEvent::Keyboard { event } => Some(TranslatedInput::Keyboard(KeyboardInputEvent::Key {
                        keycode: event.key_code().into(),
                        state: event.state(),
                        time: event.time_msec(),
                    })),
                    InputEvent::PointerMotionAbsolute { event } => Some(TranslatedInput::Pointer(PointerInputEvent::MotionAbsolute {
                        position: event.position_transformed(Size::from((1, 1))),
                        time: event.time_msec(),
                    })),
                    InputEvent::PointerButton { event } => Some(TranslatedInput::Pointer(PointerInputEvent::Button {
                        button: event.button_code(),
                        state: event.state(),
                        time: event.time_msec(),
                    })),
                    InputEvent::PointerAxis { event } => Some(TranslatedInput::Pointer(PointerInputEvent::Axis {
                        frame: build_axis_frame::<X11Input>(&event),
                    })),
                    _ => None,
                };
                if let Some(input) = input {
                    data.dispatch_translated_input(input);
                }
            }
            X11Event::Focus { focused: false, .. } => {
                data.release_all_keys();
            }
            _ => {}
        })
        .map_err(|err| anyhow!("Failed to insert X11 Backend into event loop: {err}"))?;

    Ok((event_loop, state))
}

impl X11Data {
    fn render(
        &mut self,
        core: &mut Xfwl4Core<X11Data>,
        frame_target: Time<Monotonic>,
    ) -> Result<(Option<SurfaceDmabufFeedback>, Option<RenderElementStates>), RenderFailure> {
        if self.render {
            profiling::scope!("render_frame");

            #[cfg(feature = "debug")]
            if let Some(renderdoc) = core.renderdoc.as_mut() {
                renderdoc.start_frame_capture(self.renderer.egl_context().get_context_handle(), std::ptr::null());
            }

            let output = self.output.clone();
            let (elements, clear_color) = core.prepare_render(&output, frame_target, &mut self.renderer);

            let (mut buffer, age) = self
                .surface
                .buffer()
                .context("gbm device was destroyed")
                .map_err(RenderFailure::Error)?;
            let mut fb = self
                .renderer
                .bind(&mut buffer)
                .context("Failed to bind buffer")
                .map_err(RenderFailure::Error)?;
            let render_res = self
                .damage_tracker
                .render_output(&mut self.renderer, &mut fb, age.into(), &elements, clear_color);

            match render_res {
                Ok(render_output_result) => {
                    trace!("Finished rendering");
                    let submitted = if let Err(err) = self.surface.submit() {
                        self.surface.reset_buffers();
                        warn!("Failed to submit buffer: {}. Retrying", err);
                        false
                    } else {
                        true
                    };

                    let states = render_output_result.states;
                    #[cfg(feature = "debug")]
                    let rendered = render_output_result.damage.is_some();
                    if render_output_result.damage.is_some() {
                        let mut output_presentation_feedback = core.take_presentation_feedback(&output, &states);
                        output_presentation_feedback.presented(
                            frame_target,
                            output
                                .current_mode()
                                .map(|mode| Refresh::fixed(Duration::from_secs_f64(1_000f64 / mode.refresh as f64)))
                                .unwrap_or(Refresh::Unknown),
                            0,
                            wp_presentation_feedback::Kind::Vsync,
                        )
                    }

                    #[cfg(feature = "debug")]
                    if rendered {
                        if let Some(renderdoc) = core.renderdoc.as_mut() {
                            renderdoc.end_frame_capture(self.renderer.egl_context().get_context_handle(), std::ptr::null());
                        }
                    } else if let Some(renderdoc) = core.renderdoc.as_mut() {
                        renderdoc.discard_frame_capture(self.renderer.egl_context().get_context_handle(), std::ptr::null());
                    }

                    self.window.set_cursor_visible(false);
                    self.render = !submitted;
                    profiling::finish_frame!();

                    Ok((None, Some(states)))
                }

                Err(err) => {
                    #[cfg(feature = "debug")]
                    if let Some(renderdoc) = core.renderdoc.as_mut() {
                        renderdoc.discard_frame_capture(self.renderer.egl_context().get_context_handle(), std::ptr::null());
                    }

                    self.surface.reset_buffers();
                    profiling::finish_frame!();
                    Err(RenderFailure::Error(err.into()))
                }
            }
        } else {
            Err(RenderFailure::NotNeeded)
        }
    }
}
