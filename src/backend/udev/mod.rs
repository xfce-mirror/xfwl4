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

use std::{collections::hash_map::HashMap, path::PathBuf};

use crate::{
    backend::{
        Backend,
        udev::{
            device::{BackendData, DeviceAddError, UdevOutputId, get_surface_dmabuf_feedback},
            handlers::wlr_gamma_control::UdevGammaControlData,
        },
    },
    core::{
        config::{OutputConfigChange, PointerConfig},
        drawing::*,
        input_handler::KeyAction,
        state::Xfwl4State,
    },
    ui::{FromUiMessage, ToUiMessage},
};

use anyhow::{Context, anyhow};
use glib::Sender;
#[cfg(feature = "egl")]
use smithay::backend::renderer::ImportEgl;
use smithay::{
    backend::{
        allocator::{Fourcc, Modifier, dmabuf::Dmabuf},
        drm::{DrmDeviceFd, DrmNode, NodeType},
        egl::{self, EGLContext, context::ContextPriority},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            Bind, DebugFlags, ImportDma, ImportMemWl,
            element::memory::MemoryRenderBuffer,
            gles::{Capability, GlesRenderer},
            multigpu::{GpuManager, MultiTexture, gbm::GbmGlesBackend},
        },
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, all_gpus, primary_gpu},
    },
    input::keyboard::LedState,
    output::Output,
    reexports::{
        calloop::{EventLoop, channel},
        input::Libinput,
        wayland_server::{Display, DisplayHandle, backend::GlobalId, protocol::wl_surface},
    },
    wayland::{
        dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufState},
        drm_syncobj::{DrmSyncobjState, supports_syncobj_eventfd},
        image_copy_capture::DmabufConstraints,
    },
};
use tracing::{error, info, warn};

pub mod device;
mod handlers;
pub mod input_handler;
pub mod render;

pub struct UdevConfig {
    pub drm_device: Option<PathBuf>,
    pub disable_gles_instancing: bool,
    pub disable_10bit_color: bool,
    pub disable_direct_scanout: bool,
}

pub struct UdevData {
    pub session: LibSeatSession,
    dh: DisplayHandle,
    dmabuf_state: Option<(DmabufState, DmabufGlobal)>,
    syncobj_state: Option<DrmSyncobjState>,
    primary_gpu: DrmNode,
    gpus: GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>,
    backends: HashMap<DrmNode, BackendData>,
    pointer_images: Vec<(xcursor::parser::Image, MemoryRenderBuffer)>,
    pointer_element: PointerElement,
    #[cfg(feature = "debug")]
    debug: Option<crate::core::debug::BackendDebug<smithay::backend::renderer::multigpu::MultiTexture>>,
    pointer_image: crate::core::cursor::Cursor,
    debug_flags: DebugFlags,
    keyboards: Vec<smithay::reexports::input::Device>,
    pointers: Vec<(smithay::reexports::input::Device, PointerConfig)>,
    disable_10bit_color: bool,
    disable_direct_scanout: bool,
}

impl UdevData {
    pub fn set_debug_flags(&mut self, flags: DebugFlags) {
        if self.debug_flags != flags {
            self.debug_flags = flags;

            for (_, backend) in self.backends.iter_mut() {
                for (_, surface) in backend.surfaces.iter_mut() {
                    surface.drm_output.set_debug_flags(flags);
                }
            }
        }
    }

    pub fn debug_flags(&self) -> DebugFlags {
        self.debug_flags
    }
}

impl Backend for UdevData {
    const HAS_RELATIVE_MOTION: bool = true;
    const HAS_GESTURES: bool = true;

    type RendererError = render::UdevRendererError;
    type RendererTextureId = MultiTexture;
    type Renderer<'a>
        = render::UdevRenderer<'a>
    where
        Self: 'a;

    type GammaControlData = UdevGammaControlData;

    fn backend_type(&self) -> super::BackendType {
        super::BackendType::Tty
    }

    fn seat_name(&self) -> String {
        self.session.seat()
    }

    fn reset_buffers(&mut self, output: &Output) {
        if let Some(id) = output.user_data().get::<UdevOutputId>()
            && let Some(gpu) = self.backends.get_mut(&id.device_id)
            && let Some(surface) = gpu.surfaces.get_mut(&id.crtc)
        {
            surface.drm_output.reset_buffers();
        }
    }

    fn early_import(&mut self, surface: &wl_surface::WlSurface) {
        if let Err(err) = self.gpus.early_import(self.primary_gpu, surface) {
            warn!("Early buffer import failed: {}", err);
        }
    }

    fn update_led_state(&mut self, led_state: LedState) {
        for keyboard in self.keyboards.iter_mut() {
            keyboard.led_update(led_state.into());
        }
    }

    fn renderer(&mut self, node: Option<smithay::backend::drm::DrmNode>) -> anyhow::Result<Self::Renderer<'_>> {
        let node = node.as_ref().unwrap_or(&self.primary_gpu);
        Ok(self.gpus.single_renderer(node)?)
    }

    fn dmabuf_constraints(&mut self, node: Option<DrmNode>) -> Option<DmabufConstraints> {
        let node = node.unwrap_or(self.primary_gpu);
        let renderer = self.gpus.single_renderer(&node).ok()?;
        let formats = Bind::<Dmabuf>::supported_formats(&renderer)?
            .iter()
            .fold(HashMap::<Fourcc, Vec<Modifier>>::new(), |mut map, fmt| {
                map.entry(fmt.code).or_default().push(fmt.modifier);
                map
            })
            .into_iter()
            .collect();
        Some(DmabufConstraints { node, formats })
    }

    fn set_cursor(&mut self, cursor: crate::core::cursor::Cursor) {
        self.pointer_image = cursor;
    }

    fn outputs(&self) -> Vec<(GlobalId, Output)> {
        self.backends
            .values()
            .flat_map(|backend_data| {
                backend_data.surfaces.values().flat_map(|surface_data| {
                    surface_data
                        .global
                        .as_ref()
                        .map(|global| (global.clone(), surface_data.output.clone()))
                })
            })
            .collect()
    }

    fn apply_output_config_change(&mut self, output: &Output, config: OutputConfigChange) -> anyhow::Result<()> {
        self.do_apply_output_config_change(output, config)
    }

    fn set_output_gamma(&mut self, output: Output, data: &Self::GammaControlData, red: &[u16], green: &[u16], blue: &[u16]) -> bool {
        self.set_output_gamma_real(output, data, red, green, blue)
    }
}

pub fn init(
    config: UdevConfig,
    from_ui_channel_rx: channel::Channel<FromUiMessage>,
    to_ui_channel_tx: Sender<ToUiMessage>,
) -> anyhow::Result<(EventLoop<'static, Xfwl4State<UdevData>>, Xfwl4State<UdevData>)> {
    let event_loop = EventLoop::try_new().context("Failed to create event loop")?;
    let display = Display::new().context("Failed to create Wayland display")?;
    let display_handle = display.handle();

    /*
     * Initialize session
     */
    let (session, notifier) = LibSeatSession::new().context("Failed to intialize libseat session")?;

    /*
     * Initialize the compositor
     */
    let primary_gpu = if let Some(var) = config.drm_device {
        DrmNode::from_path(var).context("Invalid DRM device path for GPU")
    } else {
        match primary_gpu(session.seat())
            .context("Failed to find primary GPU")?
            .and_then(|x| DrmNode::from_path(x).ok()?.node_with_type(NodeType::Render)?.ok())
        {
            Some(node) => Ok(node),
            None => all_gpus(session.seat())
                .context("Failed to query all GPUS")?
                .into_iter()
                .find_map(|x| DrmNode::from_path(x).ok())
                .ok_or_else(|| anyhow!("No usable GPU found")),
        }
    }?;
    info!("Using {primary_gpu} as primary GPU");

    let gpus = GpuManager::new(GbmGlesBackend::with_factory(move |display| {
        let context = EGLContext::new_with_priority(display, ContextPriority::High)?;
        let mut capabilities = unsafe { GlesRenderer::supported_capabilities(&context)? };
        if config.disable_gles_instancing {
            capabilities.retain(|capability| *capability != Capability::Instancing);
        }
        Ok(unsafe { GlesRenderer::with_capabilities(context, capabilities)? })
    }))
    .context("Failed to initialize GPU manager")?;

    let data = UdevData {
        dh: display_handle.clone(),
        dmabuf_state: None,
        syncobj_state: None,
        session,
        primary_gpu,
        gpus,
        backends: HashMap::new(),
        pointer_image: crate::core::cursor::Cursor::fallback(),
        pointer_images: Vec::new(),
        pointer_element: PointerElement::default(),
        #[cfg(feature = "debug")]
        debug: None,
        debug_flags: DebugFlags::empty(),
        keyboards: Vec::new(),
        pointers: Vec::new(),
        disable_10bit_color: config.disable_10bit_color,
        disable_direct_scanout: config.disable_direct_scanout,
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

    /*
     * Initialize the udev backend
     */
    let udev_backend = UdevBackend::new(&state.core.seat_name).context("Failed to intialize udev backend")?;

    /*
     * Initialize libinput backend
     */
    let mut libinput_context = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(state.backend.session.clone().into());
    libinput_context
        .udev_assign_seat(&state.core.seat_name)
        .map_err(|_| anyhow!("Failed to assign libinput context to seat"))?;
    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    /*
     * Bind all our objects that get driven by the event loop
     */
    event_loop
        .handle()
        .insert_source(libinput_backend, move |event, _, state| {
            if let Some(input) = state.backend.translate_input_event(event)
                && let KeyAction::VtSwitch(vt) = state.dispatch_translated_input(input)
            {
                use smithay::backend::session::Session;
                info!(to = vt, "Trying to switch vt");
                if let Err(err) = state.backend.session.change_vt(vt) {
                    error!(vt, "Error switching vt: {}", err);
                }
            }
        })
        .map_err(|err| anyhow!("Failed to register libinput event source: {err}"))?;

    event_loop
        .handle()
        .insert_source(notifier, move |event, &mut (), state| match event {
            SessionEvent::PauseSession => {
                libinput_context.suspend();
                info!("pausing session");

                for backend in state.backend.backends.values_mut() {
                    backend.drm_output_manager.pause();
                    backend.active_leases.clear();
                    if let Some(lease_global) = backend.leasing_global.as_mut() {
                        lease_global.suspend();
                    }
                }
            }
            SessionEvent::ActivateSession => {
                info!("resuming session");

                if let Err(err) = libinput_context.resume() {
                    error!("Failed to resume libinput context: {:?}", err);
                }
                for (node, backend) in state.backend.backends.iter_mut().map(|(handle, backend)| (*handle, backend)) {
                    // if we do not care about flicking (caused by modesetting) we could just
                    // pass true for disable connectors here. this would make sure our drm
                    // device is in a known state (all connectors and planes disabled).
                    // but for demonstration we choose a more optimistic path by leaving the
                    // state as is and assume it will just work. If this assumption fails
                    // we will try to reset the state when trying to queue a frame.
                    backend
                        .drm_output_manager
                        .lock()
                        .activate(false)
                        .expect("failed to activate drm backend");
                    if let Some(lease_global) = backend.leasing_global.as_mut() {
                        lease_global.resume::<Xfwl4State<UdevData>>();
                    }
                    state
                        .core
                        .handle
                        .insert_idle(move |data| data.render(node, None, data.core.clock.now()));
                }
            }
        })
        .map_err(|err| anyhow!("Failed to register session notifier event source: {err}"))?;

    // We try to initialize the primary node before others to make sure
    // any display only node can fall back to the primary node for rendering
    let primary_node = primary_gpu.node_with_type(NodeType::Primary).and_then(|node| node.ok());
    let primary_device = udev_backend.device_list().find(|(device_id, _)| {
        primary_node
            .map(|primary_node| *device_id == primary_node.dev_id())
            .unwrap_or(false)
            || *device_id == primary_gpu.dev_id()
    });

    if let Some((device_id, path)) = primary_device {
        let node = DrmNode::from_dev_id(device_id).context("Failed to get primary GPU node")?;
        state.device_added(node, path).context("Failed to initialize primary GPU node")?;
    }

    let primary_device_id = primary_device.map(|(device_id, _)| device_id);
    for (device_id, path) in udev_backend.device_list() {
        if Some(device_id) == primary_device_id {
            continue;
        }

        if let Err(err) = DrmNode::from_dev_id(device_id)
            .map_err(DeviceAddError::DrmNode)
            .and_then(|node| state.device_added(node, path))
        {
            error!("Skipping device {device_id}: {err}");
        }
    }

    #[cfg_attr(not(feature = "egl"), allow(unused_mut))]
    let mut renderer = state
        .backend
        .gpus
        .single_renderer(&primary_gpu)
        .context("Failed to get renderer for primary GPU")?;

    state.core.shm_state.update_formats(renderer.shm_formats());

    #[cfg(feature = "debug")]
    if let Some(backend_debug) = crate::core::debug::BackendDebug::new(&mut renderer) {
        for backend in state.backend.backends.values_mut() {
            for surface in backend.surfaces.values_mut() {
                surface.debug = Some(crate::core::debug::RenderDebug::new(&backend_debug));
            }
        }
        state.backend.debug = Some(backend_debug);
    }

    #[cfg(feature = "egl")]
    {
        info!(?primary_gpu, "Trying to initialize EGL Hardware Acceleration",);
        match renderer.bind_wl_display(&display_handle) {
            Ok(_) => info!("EGL hardware-acceleration enabled"),
            Err(egl::Error::EglExtensionNotSupported(exts)) if exts.iter().all(|ext| *ext == "EGL_WL_bind_wayland_display") => {
                info!("Failed to intialize EGL hardware-acceleration; this error is safe to ignore");
            }
            Err(err) => warn!(?err, "Failed to initialize EGL hardware-acceleration"),
        }
    }

    // init dmabuf support with format list from our primary gpu
    let dmabuf_formats = renderer.dmabuf_formats();
    let default_feedback = DmabufFeedbackBuilder::new(primary_gpu.dev_id(), dmabuf_formats)
        .build()
        .context("Failed to build default DMABUF feedback")?;
    let mut dmabuf_state = DmabufState::new();
    let global = dmabuf_state.create_global_with_default_feedback::<Xfwl4State<UdevData>>(&display_handle, &default_feedback);
    state.backend.dmabuf_state = Some((dmabuf_state, global));

    let gpus = &mut state.backend.gpus;
    state.backend.backends.iter_mut().for_each(|(node, backend_data)| {
        // Update the per drm surface dmabuf feedback
        backend_data.surfaces.values_mut().for_each(|surface_data| {
            surface_data.dmabuf_feedback = surface_data.dmabuf_feedback.take().or_else(|| {
                surface_data.drm_output.with_compositor(|compositor| {
                    get_surface_dmabuf_feedback(primary_gpu, surface_data.render_node, *node, gpus, compositor.surface())
                })
            });
        });
    });

    // Expose syncobj protocol if supported by primary GPU
    if let Some(primary_node) = state.backend.primary_gpu.node_with_type(NodeType::Primary).and_then(|x| x.ok())
        && let Some(backend) = state.backend.backends.get(&primary_node)
    {
        let import_device = backend.drm_output_manager.device().device_fd().clone();
        if supports_syncobj_eventfd(&import_device) {
            let syncobj_state = DrmSyncobjState::new::<Xfwl4State<UdevData>>(&display_handle, import_device);
            state.backend.syncobj_state = Some(syncobj_state);
        }
    }

    event_loop
        .handle()
        .insert_source(udev_backend, move |event, _, state| match event {
            UdevEvent::Added { device_id, path } => {
                if let Err(err) = DrmNode::from_dev_id(device_id)
                    .map_err(DeviceAddError::DrmNode)
                    .and_then(|node| state.device_added(node, &path))
                {
                    error!("Skipping device {device_id}: {err}");
                }
            }
            UdevEvent::Changed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    state.device_changed(node)
                }
            }
            UdevEvent::Removed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    state.device_removed(node)
                }
            }
        })
        .map_err(|err| anyhow!("Failed to register udev event source: {err}"))?;

    Ok((event_loop, state))
}

//pub type RenderSurface = GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, Option<OutputPresentationFeedback>>;

//pub type GbmDrmCompositor =
//    DrmCompositor<GbmAllocator<DrmDeviceFd>, GbmDevice<DrmDeviceFd>, Option<OutputPresentationFeedback>, DrmDeviceFd>;
