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

use std::{cell::RefCell, collections::HashMap, sync::Mutex, time::Duration};

use anyhow::anyhow;
use smithay::{
    backend::{
        allocator::{Fourcc, dmabuf::Dmabuf},
        renderer::{
            Bind, Color32F, ExportMem, ImportAll, ImportMem, Offscreen, Renderer, RendererSuper, Texture, TextureMapping,
            damage::OutputDamageTracker,
            element::{
                AsRenderElements, Element, Id, Kind, RenderElement, RenderElementStates, Wrap, default_primary_scanout_output_compare,
                memory::MemoryRenderBuffer,
                surface::WaylandSurfaceRenderElement,
                texture::TextureRenderElement,
                utils::{Relocate, RelocateRenderElement, select_dmabuf_feedback},
            },
            gles::{GlesRenderbuffer, GlesRenderer, GlesTarget, GlesTexture},
        },
    },
    desktop::{
        LayerMap, LayerSurface, layer_map_for_output,
        space::{SpaceRenderElements, SurfaceTree},
        utils::{
            OutputPresentationFeedback, surface_presentation_feedback_flags_from_states, surface_primary_scanout_output,
            update_surface_primary_scanout_output, with_surfaces_surface_tree,
        },
    },
    input::pointer::{CursorImageAttributes, CursorImageStatus},
    output::Output,
    reexports::wayland_server::{Client, Resource, backend::ClientId, protocol::wl_buffer::WlBuffer},
    render_elements,
    utils::{Buffer, IsAlive, Logical, Monotonic, Point, Rectangle, Scale, Size, Time, Transform},
    wayland::{
        commit_timing::CommitTimerBarrierStateUserData,
        compositor::{self, CompositorHandler},
        dmabuf::{DmabufFeedback, get_dmabuf},
        fifo::FifoBarrierCachedState,
        fractional_scale::with_fractional_scale,
        image_copy_capture::{CaptureFailureReason, Frame as ImageCopyFrame, SessionRef},
        shell::wlr_layer::Layer,
        shm,
    },
};

use crate::{
    backend::{AsGlesRenderer, Backend, FromGlesError},
    core::{
        drawing::{
            CLEAR_COLOR, CLEAR_COLOR_FULLSCREEN, PointerRenderElement,
            shadows::{self, ShadowParams},
            zoom::ZoomedRenderElement,
        },
        handlers::data_device::DndIcon,
        shell::{ShadowKey, WindowRenderElement, WindowShadow, WindowShadowTexture},
        state::{Xfwl4Core, Xfwl4State},
        util::OutputImageCopyExt,
        workspaces::Workspace,
    },
    protocols::wlr_screencopy::WlrFrame,
};

render_elements! {
    pub CustomRenderElements<R> where
        R: ImportAll + ImportMem;
    Pointer=PointerRenderElement<R>,
    Surface=WaylandSurfaceRenderElement<R>,
    #[cfg(feature = "debug")]
    Fps=smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement<R>,
}

impl<R: Renderer> std::fmt::Debug for CustomRenderElements<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pointer(arg0) => f.debug_tuple("Pointer").field(arg0).finish(),
            Self::Surface(arg0) => f.debug_tuple("Surface").field(arg0).finish(),
            #[cfg(feature = "debug")]
            Self::Fps(arg0) => f.debug_tuple("Fps").field(arg0).finish(),
            Self::_GenericCatcher(arg0) => f.debug_tuple("_GenericCatcher").field(arg0).finish(),
        }
    }
}

render_elements! {
    pub BaseOutputRenderElements<R, E> where
        R: ImportAll + ImportMem + AsGlesRenderer,
        <R as RendererSuper>::Error: FromGlesError;
    Space=SpaceRenderElements<R, E>,
    Window=Wrap<E>,
    Custom=CustomRenderElements<R>,
}

impl<R, E> std::fmt::Debug for BaseOutputRenderElements<R, E>
where
    R: Renderer + ImportAll + ImportMem + AsGlesRenderer,
    <R as RendererSuper>::Error: FromGlesError,
    E: RenderElement<R> + Element,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BaseOutputRenderElements::Space(_) => f.debug_tuple("Space").finish_non_exhaustive(),
            BaseOutputRenderElements::Window(_) => f.debug_tuple("Window").finish_non_exhaustive(),
            BaseOutputRenderElements::Custom(_) => f.debug_tuple("Custom").finish_non_exhaustive(),
            BaseOutputRenderElements::_GenericCatcher(_) => unreachable!(),
        }
    }
}

render_elements! {
    pub OutputRenderElements<R, E> where
        R: ImportAll + ImportMem + AsGlesRenderer,
        <R as RendererSuper>::Error: FromGlesError;
    Base=BaseOutputRenderElements<R, E>,
    Zoomed=ZoomedRenderElement<BaseOutputRenderElements<R, E>>,
}

impl<R, E> std::fmt::Debug for OutputRenderElements<R, E>
where
    R: Renderer + ImportAll + ImportMem + AsGlesRenderer,
    <R as RendererSuper>::Error: FromGlesError,
    E: RenderElement<R> + Element,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputRenderElements::Base(_) => f.debug_tuple("Base").finish_non_exhaustive(),
            OutputRenderElements::Zoomed(_) => f.debug_tuple("Zoomed").finish_non_exhaustive(),
            OutputRenderElements::_GenericCatcher(_) => unreachable!(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SurfaceDmabufFeedback {
    pub render_feedback: DmabufFeedback,
    pub scanout_feedback: DmabufFeedback,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderFailure {
    #[error("Render not needed for this output/device")]
    NotNeeded,
    #[error("Failed to render surface: {0}")]
    Error(anyhow::Error),
    #[error("Unrecoverable render error: {0}")]
    FatalError(anyhow::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ImageCopyError {
    #[error("no buffer constraints")]
    MissingBufferConstraints,
    #[error("{0}")]
    Unknown(#[from] anyhow::Error),
}

struct BuiltOutputElements<R>
where
    R: ImportAll + ImportMem + AsGlesRenderer,
    R::TextureId: Clone + Send + 'static,
    <R as RendererSuper>::Error: FromGlesError,
{
    pointer_elements: Vec<BaseOutputRenderElements<R, WindowRenderElement<R>>>,
    elements: Vec<BaseOutputRenderElements<R, WindowRenderElement<R>>>,
    clear_color: Color32F,
}

impl<BackendData: Backend + 'static> Xfwl4Core<BackendData> {
    fn create_shadow_element(
        renderer: &GlesRenderer,
        shadow_tex: &WindowShadowTexture,
        location: Point<i32, Logical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> TextureRenderElement<GlesTexture> {
        let buffer_scale = 1;
        let shadow_location = (location + shadow_tex.offset).to_f64().to_physical(scale);
        let tex_src: Rectangle<f64, Logical> =
            Rectangle::from_size(shadow_tex.tex.size().to_logical(buffer_scale, Transform::Normal)).to_f64();
        TextureRenderElement::from_static_texture(
            shadow_tex.tex_id.clone(),
            renderer.context_id(),
            shadow_location,
            shadow_tex.tex.clone(),
            buffer_scale,
            Transform::Normal,
            Some(alpha),
            Some(tex_src),
            Some(shadow_tex.render_size),
            None,
            Kind::Unspecified,
        )
    }

    fn render_shadow_texture(renderer: &mut GlesRenderer, key: ShadowKey, surface_size: Size<i32, Logical>) -> Option<WindowShadowTexture> {
        let params = ShadowParams::new(key.delta_loc, key.delta_width, key.delta_height, surface_size);
        let opacity = (key.opacity as f64 / 100.).clamp(0., 1.);

        match shadows::make_shadow_texture(renderer, opacity, surface_size, key.delta_loc, key.delta_width, key.delta_height) {
            Ok(Some((tex, _w, _h))) => Some(WindowShadowTexture {
                key,
                offset: params.offset,
                render_size: params.size,
                tex_id: Id::new(),
                tex,
            }),
            Ok(None) => None,
            Err(err) => {
                tracing::warn!("Failed to create shadow texture: {err}");
                None
            }
        }
    }

    fn shadow_element_for_layer_surface(
        &mut self,
        renderer: &mut GlesRenderer,
        surface: &LayerSurface,
        layer_geometry: Rectangle<i32, Logical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Option<TextureRenderElement<GlesTexture>> {
        let key = ShadowKey {
            opacity: self.config.shadow_opacity(),
            frame_size: layer_geometry.size,
            delta_loc: (self.config.shadow_delta_x(), self.config.shadow_delta_y()).into(),
            delta_width: self.config.shadow_delta_width(),
            delta_height: self.config.shadow_delta_height(),
        };

        if let Some(shadow_tex) = surface
            .user_data()
            .get::<WindowShadow>()
            .and_then(|shadow| shadow.texture_if_key(key))
        {
            Some(Self::create_shadow_element(
                renderer.gles_renderer(),
                &shadow_tex,
                layer_geometry.loc,
                scale,
                alpha,
            ))
        } else if let Some(shadow_tex) = Self::render_shadow_texture(renderer.gles_renderer_mut(), key, layer_geometry.size) {
            let elem = Self::create_shadow_element(renderer.gles_renderer(), &shadow_tex, layer_geometry.loc, scale, alpha);
            surface
                .user_data()
                .get_or_insert(|| WindowShadow(RefCell::new(None)))
                .0
                .borrow_mut()
                .replace(shadow_tex);
            Some(elem)
        } else {
            None
        }
    }

    fn render_active_workspace_elements<R>(
        &mut self,
        renderer: &mut R,
        output: &Output,
        alpha: f32,
    ) -> Vec<SpaceRenderElements<R, WindowRenderElement<R>>>
    where
        R: Renderer + ImportAll + ImportMem + AsGlesRenderer,
        R::TextureId: Clone + Send + 'static,
        <R as smithay::backend::renderer::RendererSuper>::Error: FromGlesError,
    {
        let mut render_elements = Vec::new();
        let output_scale = output.current_scale().fractional_scale();
        let scale = Scale::from(output_scale);

        let layer_map = layer_map_for_output(output);
        let (background, bottom, top, overlay) = {
            let (lower, upper) = layer_map
                .layers()
                .rev()
                .partition::<Vec<_>, _>(|surface| matches!(surface.layer(), Layer::Background | Layer::Bottom));
            let (background, bottom) = lower
                .into_iter()
                .partition::<Vec<_>, _>(|surface| surface.layer() == Layer::Background);
            let (top, overlay) = upper.into_iter().partition::<Vec<_>, _>(|surface| surface.layer() == Layer::Top);

            (background, bottom, top, overlay)
        };

        let render_elements_for_layer =
            |core: &mut Xfwl4Core<BackendData>, surfaces: Vec<&LayerSurface>, renderer: &mut R, layer_map: &LayerMap, draw_shadow: bool| {
                surfaces
                    .into_iter()
                    .filter_map(|surface| layer_map.layer_geometry(surface).map(|geo| (geo, surface)))
                    .flat_map(|(geo, surface)| {
                        let surface_elements = AsRenderElements::<R>::render_elements::<WaylandSurfaceRenderElement<R>>(
                            surface,
                            renderer,
                            geo.loc.to_physical_precise_round(output_scale),
                            scale,
                            alpha,
                        )
                        .into_iter()
                        .map(SpaceRenderElements::Surface);

                        if draw_shadow {
                            let shadow_elem = core
                                .shadow_element_for_layer_surface(renderer.gles_renderer_mut(), surface, geo, scale, alpha)
                                .map(|shadow_elem| SpaceRenderElements::Element(Wrap::from(WindowRenderElement::Shadow(shadow_elem))));

                            surface_elements.chain(shadow_elem).collect::<Vec<_>>()
                        } else {
                            surface_elements.collect::<Vec<_>>()
                        }
                    })
                    .collect::<Vec<_>>()
            };

        render_elements.extend(render_elements_for_layer(self, overlay, renderer, &layer_map, false));
        render_elements.extend(render_elements_for_layer(
            self,
            top,
            renderer,
            &layer_map,
            self.config.show_dock_shadow(),
        ));
        {
            let workspace = self.workspace_manager.active_workspace();
            if let Some(output_geo) = workspace.output_geometry(output) {
                render_elements.extend(
                    workspace
                        .render_elements_for_region(renderer, &output_geo, output_scale, alpha)
                        .into_iter()
                        .map(|elem| SpaceRenderElements::Element(Wrap::from(elem))),
                );
            }
        }
        render_elements.extend(render_elements_for_layer(self, bottom, renderer, &layer_map, false));
        render_elements.extend(render_elements_for_layer(self, background, renderer, &layer_map, false));

        render_elements
    }

    fn build_output_elements<R>(&mut self, output: &Output, renderer: &mut R) -> BuiltOutputElements<R>
    where
        R: Renderer + ImportAll + ImportMem + AsGlesRenderer,
        R::TextureId: Clone + Send + 'static,
        <R as smithay::backend::renderer::RendererSuper>::Error: FromGlesError,
    {
        profiling::scope!("prepare_render");
        let scale = Scale::from(output.current_scale().fractional_scale());
        let integer_scale = output.current_scale().integer_scale();

        #[cfg_attr(not(feature = "debug"), allow(unused_mut))]
        let mut custom_elements: Vec<CustomRenderElements<_>> = Vec::new();

        let (frame, buffer_scale) = self.pointer_image.get_image(integer_scale as u32, self.clock.now().into());
        // xhot/yhot are in buffer pixels; divide by buffer_scale to get logical coords
        let pointer_hotspot: Point<i32, Logical> =
            (frame.xhot as i32 / buffer_scale as i32, frame.yhot as i32 / buffer_scale as i32).into();

        let pointer_image_cache = &mut self.pointer_image_cache;
        let pointer_image = pointer_image_cache
            .iter()
            .find_map(|(image, texture)| if image == &frame { Some(texture.clone()) } else { None })
            .unwrap_or_else(|| {
                let buffer = MemoryRenderBuffer::from_slice(
                    &frame.pixels_rgba,
                    Fourcc::Argb8888,
                    (frame.width as i32, frame.height as i32),
                    buffer_scale as i32,
                    Transform::Normal,
                    None,
                );
                pointer_image_cache.push((frame, buffer.clone()));
                buffer
            });

        let output_geometry = self.workspace_manager.active_workspace().output_geometry(output).unwrap();
        let pointer_location = self.pointer.current_location();
        let pointer_elements = if output_geometry.to_f64().contains(pointer_location) {
            let mut pointer_elements = Vec::<CustomRenderElements<R>>::new();

            let cursor_hotspot = if let &mut CursorImageStatus::Surface(ref surface) = &mut self.cursor_status {
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
                pointer_hotspot
            };
            let cursor_pos = pointer_location - output_geometry.loc.to_f64();

            self.pointer_element.set_buffer(pointer_image.clone());
            let reset_cursor = if let CursorImageStatus::Surface(ref surface) = self.cursor_status {
                !surface.alive()
            } else {
                false
            };
            if reset_cursor {
                self.cursor_status = CursorImageStatus::default_named();
            }
            self.pointer_element.set_status(self.cursor_status.clone());

            pointer_elements.extend(self.pointer_element.render_elements(
                renderer,
                (cursor_pos - cursor_hotspot.to_f64()).to_physical(scale).to_i32_round(),
                scale,
                1.0,
            ));

            if let Some(icon) = self.dnd_icon.as_ref() {
                let dnd_icon_pos = (cursor_pos + icon.offset.to_f64()).to_physical(scale).to_i32_round();
                if icon.surface.alive() {
                    pointer_elements.extend(AsRenderElements::<R>::render_elements(
                        &SurfaceTree::from_surface(&icon.surface),
                        renderer,
                        dnd_icon_pos,
                        scale,
                        1.0,
                    ));
                }
            }

            pointer_elements
        } else {
            vec![]
        };

        #[cfg(feature = "debug")]
        if let Some(debug) = output.user_data().get::<std::cell::RefCell<crate::core::debug::RenderDebug>>() {
            // FIXME: don't update() when calling for screencopy
            debug.borrow_mut().update();
            custom_elements.extend(debug.borrow().fps_element().render_elements(renderer, (0, 0).into(), scale, 1.0));
        }

        let (elements, clear_color) = if let Some(lock_surface) = self.session_lock_surface_for_output(output) {
            match compositor::with_states(&lock_surface, |states| {
                WaylandSurfaceRenderElement::from_surface(
                    renderer,
                    &lock_surface,
                    states,
                    output
                        .current_location()
                        .to_f64()
                        .to_physical(output.current_scale().fractional_scale()),
                    1.,
                    Kind::Unspecified,
                )
            }) {
                Ok(Some(elem)) => (
                    vec![BaseOutputRenderElements::Custom(CustomRenderElements::Surface(elem))],
                    CLEAR_COLOR_FULLSCREEN,
                ),
                Ok(None) => {
                    tracing::warn!("Failed to create render element from lockscreen surface");
                    (vec![], CLEAR_COLOR_FULLSCREEN)
                }
                Err(err) => {
                    tracing::warn!("Failed to create render element from lockscreen surface: {err}");
                    (vec![], CLEAR_COLOR_FULLSCREEN)
                }
            }
        } else if let Some(window) = self.workspace_manager.active_workspace().fullscreen_window_for_output(output) {
            let scale = output.current_scale().fractional_scale().into();
            let elements = AsRenderElements::<R>::render_elements(&window, renderer, (0, 0).into(), scale, 1.0)
                .into_iter()
                .map(|elem: WindowRenderElement<R>| BaseOutputRenderElements::Window(Wrap::from(elem)))
                .collect::<Vec<_>>();
            (elements, CLEAR_COLOR_FULLSCREEN)
        } else {
            let elements = self
                .render_active_workspace_elements(renderer, output, 1.)
                .into_iter()
                .map(BaseOutputRenderElements::Space)
                .collect::<Vec<_>>();
            (elements, CLEAR_COLOR)
        };

        let pointer_elements = pointer_elements.into_iter().map(BaseOutputRenderElements::from).collect();

        let elements = custom_elements
            .into_iter()
            .map(BaseOutputRenderElements::from)
            .chain(elements)
            .collect();

        BuiltOutputElements {
            pointer_elements,
            elements,
            clear_color,
        }
    }

    pub fn prepare_render<R>(
        &mut self,
        output: &Output,
        _frame_target: Time<Monotonic>,
        renderer: &mut R,
    ) -> (Vec<OutputRenderElements<R, WindowRenderElement<R>>>, Color32F)
    where
        R: Renderer + ImportAll + ImportMem + AsGlesRenderer,
        R::TextureId: Clone + Send + 'static,
        <R as smithay::backend::renderer::RendererSuper>::Error: FromGlesError,
    {
        let BuiltOutputElements {
            pointer_elements,
            elements,
            clear_color,
        } = self.build_output_elements(output, renderer);

        if let Some(zoom_state) = self.outputs_config.zoom_state_for_output_mut(output)
            && zoom_state.is_zoomed()
            && let Some(output_mode) = output.current_mode()
            && let Some(output_geom) = self.workspace_manager.active_workspace().output_geometry(output)
        {
            let (unzoomed_pointer_elements, zoomed_elements) = if self.config.zoom_pointer() {
                let zoomed = pointer_elements.into_iter().chain(elements).collect();
                (vec![], zoomed)
            } else {
                (pointer_elements, elements)
            };

            let unzoomed_pointer_elements = unzoomed_pointer_elements
                .into_iter()
                .map(OutputRenderElements::Base)
                .collect::<Vec<_>>();

            let output_scale = output.current_scale().fractional_scale();
            let pointer_location = (self.pointer.current_location() - output_geom.loc.to_f64()).to_physical(output_scale);
            let zoomed_elements = zoom_state
                .zoomed_render_elements(pointer_location, output_mode.size, output_scale, zoomed_elements)
                .into_iter()
                .map(OutputRenderElements::Zoomed)
                .collect::<Vec<_>>();

            (unzoomed_pointer_elements.into_iter().chain(zoomed_elements).collect(), clear_color)
        } else {
            (
                pointer_elements
                    .into_iter()
                    .chain(elements)
                    .map(OutputRenderElements::Base)
                    .collect(),
                clear_color,
            )
        }
    }

    #[profiling::function]
    pub fn take_presentation_feedback(&self, output: &Output, render_element_states: &RenderElementStates) -> OutputPresentationFeedback {
        let mut output_presentation_feedback = OutputPresentationFeedback::new(output);

        let workspace = self.workspace_manager.active_workspace();
        workspace.elements().for_each(|window| {
            if workspace.outputs_for_element(window).contains(output) {
                window.take_presentation_feedback(&mut output_presentation_feedback, surface_primary_scanout_output, |surface, _| {
                    surface_presentation_feedback_flags_from_states(surface, render_element_states)
                });
            }
        });
        let map = smithay::desktop::layer_map_for_output(output);
        for layer_surface in map.layers() {
            layer_surface.take_presentation_feedback(&mut output_presentation_feedback, surface_primary_scanout_output, |surface, _| {
                surface_presentation_feedback_flags_from_states(surface, render_element_states)
            });
        }

        output_presentation_feedback
    }

    fn finish_render<R>(
        &mut self,
        output: &Output,
        frame_target: Time<Monotonic>,
        renderer: &mut R,
        render_element_states: &RenderElementStates,
    ) where
        R: Renderer + ImportAll + ImportMem + AsGlesRenderer,
        R::TextureId: Clone + 'static,
    {
        profiling::scope!("finish_render");
        // NB: this used to be _before_ udev's surface.drm_output.queue_frame().  hopefully
        // it's ok to move it after.
        update_primary_scanout_output(
            self.workspace_manager.active_workspace(),
            output,
            &self.dnd_icon,
            &self.cursor_status,
            render_element_states,
        );

        let image_copy_frames = output.take_image_copy_frames();
        let wlr_screencopy_frames = output.take_wlr_screencopy_frames();
        if image_copy_frames.is_some() || wlr_screencopy_frames.is_some() {
            let renderer = renderer.gles_renderer_mut();
            let BuiltOutputElements {
                pointer_elements,
                elements,
                clear_color,
            } = self.build_output_elements(output, renderer);
            let elements = pointer_elements.into_iter().chain(elements).collect::<Vec<_>>();

            if let Some(frames) = image_copy_frames {
                self.render_image_copy_frames(renderer, frames, output, &elements, clear_color, frame_target);
            }
            if let Some(frames) = wlr_screencopy_frames {
                self.render_wlr_screencopy_frames(renderer, frames, output, &elements, clear_color, frame_target);
            }
        }
    }

    pub fn now(&self) -> Time<Monotonic> {
        self.clock.now()
    }

    fn render_image_copy_frame(
        session: SessionRef,
        frame: &ImageCopyFrame,
        gles: &mut GlesRenderer,
        output: &Output,
        elements: &[BaseOutputRenderElements<GlesRenderer, WindowRenderElement<GlesRenderer>>],
        clear_color: Color32F,
    ) -> Result<(), ImageCopyError> {
        if let Some(constraints) = session.current_constraints() {
            let size = constraints.size;
            let wl_buffer = frame.buffer();
            let dmabuf = get_dmabuf(&wl_buffer).ok().cloned();

            render_to_capture_buffer(
                gles,
                size,
                dmabuf,
                &wl_buffer,
                |gles: &mut GlesRenderer, target: &mut GlesTarget<'_>| {
                    let mut tracker = OutputDamageTracker::from_output(output);
                    let render_result = tracker
                        .render_output(gles, target, 0, elements, clear_color)
                        .map_err(|err| anyhow!("Render failed: {err}"))?;
                    render_result
                        .sync
                        .wait()
                        .map_err(|err| anyhow::anyhow!("Render interrupted: {err}"))
                },
            )?;

            Ok(())
        } else {
            Err(ImageCopyError::MissingBufferConstraints)
        }
    }

    fn render_image_copy_frames(
        &mut self,
        gles: &mut GlesRenderer,
        frames: Vec<(SessionRef, ImageCopyFrame)>,
        output: &Output,
        elements: &[BaseOutputRenderElements<GlesRenderer, WindowRenderElement<GlesRenderer>>],
        clear_color: Color32F,
        presented: Time<Monotonic>,
    ) {
        for (session, frame) in frames {
            if let Err(err) = Self::render_image_copy_frame(session, &frame, gles, output, elements, clear_color) {
                tracing::warn!("Failed to render output image copy frame: {err}");
                let reason = match err {
                    ImageCopyError::MissingBufferConstraints => CaptureFailureReason::BufferConstraints,
                    _ => CaptureFailureReason::Unknown,
                };
                frame.fail(reason);
            } else {
                frame.success(output.current_transform(), None, presented);
            }
        }
    }

    fn render_wlr_screencopy_frame(
        frame: &WlrFrame,
        wl_buffer: WlBuffer,
        gles: &mut GlesRenderer,
        output: &Output,
        elements: &[BaseOutputRenderElements<GlesRenderer, WindowRenderElement<GlesRenderer>>],
        clear_color: Color32F,
    ) -> anyhow::Result<()> {
        let size = frame.buffer_size();
        let output_rect = frame.output_rect();
        let dmabuf = get_dmabuf(&wl_buffer).ok().cloned();

        render_to_capture_buffer(
            gles,
            size,
            dmabuf,
            &wl_buffer,
            |gles: &mut GlesRenderer, target: &mut GlesTarget<'_>| {
                let scale = output.current_scale().fractional_scale();
                let region_offset = output.current_location() - output_rect.loc;
                let physical_offset = region_offset.to_f64().to_physical(scale).to_i32_round::<i32>();
                let region_physical_size = output_rect.size.to_f64().to_physical(scale).to_i32_round();

                let relocated = elements
                    .iter()
                    .map(|e| RelocateRenderElement::from_element(e, physical_offset, Relocate::Relative))
                    .collect::<Vec<_>>();

                let mut tracker = OutputDamageTracker::new(region_physical_size, scale, output.current_transform());
                let render_result = tracker
                    .render_output(gles, target, 0, &relocated, clear_color)
                    .map_err(|err| anyhow!("Render failed: {err}"))?;
                render_result
                    .sync
                    .wait()
                    .map_err(|err| anyhow::anyhow!("Render interrupted: {err}"))
            },
        )?;

        Ok(())
    }

    fn render_wlr_screencopy_frames(
        &mut self,
        gles: &mut GlesRenderer,
        frames: Vec<(WlrFrame, WlBuffer)>,
        output: &Output,
        elements: &[BaseOutputRenderElements<GlesRenderer, WindowRenderElement<GlesRenderer>>],
        clear_color: Color32F,
        presented: Time<Monotonic>,
    ) {
        for (frame, buffer) in frames {
            if let Err(err) = Self::render_wlr_screencopy_frame(&frame, buffer, gles, output, elements, clear_color) {
                tracing::warn!("Failed to render wlr screencopy frame: {err}");
                frame.send_failed();
            } else {
                frame.send_ready(presented);
            }
        }
    }
}

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub fn render<F>(&mut self, output: &Output, frame_target: Time<Monotonic>, render_fn: F)
    where
        F: FnOnce(
            &mut BackendData,
            &mut Xfwl4Core<BackendData>,
        ) -> Result<(Option<SurfaceDmabufFeedback>, Option<RenderElementStates>), RenderFailure>,
    {
        profiling::scope!("render");
        self.pre_repaint(output, frame_target);

        match render_fn(&mut self.backend, &mut self.core) {
            Ok((dmabuf_feedback, Some(render_element_states))) => {
                if let Ok(mut renderer) = self.backend.renderer_for_output(output) {
                    self.core
                        .finish_render(output, frame_target, renderer.as_mut(), &render_element_states);
                }
                self.post_repaint(output, frame_target, dmabuf_feedback, &render_element_states);
            }
            Ok((_, None)) => tracing::trace!("Didn't render for some reason (no render_element_states)"),
            Err(RenderFailure::NotNeeded) => (),
            Err(RenderFailure::Error(err)) => tracing::error!("Failed to render to output {}: {err}", output.name()),
            Err(RenderFailure::FatalError(err)) => {
                tracing::error!("Unrecoverable rendering error: {err}");
                self.shutdown();
            }
        }
    }

    fn pre_repaint(&mut self, output: &Output, frame_target: impl Into<Time<Monotonic>>) {
        profiling::scope!("pre_repaint");
        let frame_target = frame_target.into();

        #[allow(clippy::mutable_key_type)]
        let mut clients: HashMap<ClientId, Client> = HashMap::new();
        let workspace = self.core.workspace_manager.active_workspace();
        workspace.space().elements().for_each(|window| {
            window.with_surfaces(|surface, states| {
                if let Some(mut commit_timer_state) = states
                    .data_map
                    .get::<CommitTimerBarrierStateUserData>()
                    .map(|commit_timer| commit_timer.lock().unwrap())
                {
                    commit_timer_state.signal_until(frame_target);
                    let client = surface.client().unwrap();
                    clients.insert(client.id(), client);
                }
            });
        });

        let map = smithay::desktop::layer_map_for_output(output);
        for layer_surface in map.layers() {
            layer_surface.with_surfaces(|surface, states| {
                if let Some(mut commit_timer_state) = states
                    .data_map
                    .get::<CommitTimerBarrierStateUserData>()
                    .map(|commit_timer| commit_timer.lock().unwrap())
                {
                    commit_timer_state.signal_until(frame_target);
                    let client = surface.client().unwrap();
                    clients.insert(client.id(), client);
                }
            });
        }
        // Drop the lock to the layer map before calling blocker_cleared, which might end up
        // calling the commit handler which in turn again could access the layer map.
        std::mem::drop(map);

        if let CursorImageStatus::Surface(ref surface) = self.core.cursor_status {
            with_surfaces_surface_tree(surface, |surface, states| {
                if let Some(mut commit_timer_state) = states
                    .data_map
                    .get::<CommitTimerBarrierStateUserData>()
                    .map(|commit_timer| commit_timer.lock().unwrap())
                {
                    commit_timer_state.signal_until(frame_target);
                    let client = surface.client().unwrap();
                    clients.insert(client.id(), client);
                }
            });
        }

        if let Some(surface) = self.core.dnd_icon.as_ref().map(|icon| &icon.surface) {
            with_surfaces_surface_tree(surface, |surface, states| {
                if let Some(mut commit_timer_state) = states
                    .data_map
                    .get::<CommitTimerBarrierStateUserData>()
                    .map(|commit_timer| commit_timer.lock().unwrap())
                {
                    commit_timer_state.signal_until(frame_target);
                    let client = surface.client().unwrap();
                    clients.insert(client.id(), client);
                }
            });
        }

        let dh = self.core.display_handle.clone();
        for client in clients.into_values() {
            self.client_compositor_state(&client).blocker_cleared(self, &dh);
        }
    }

    fn post_repaint(
        &mut self,
        output: &Output,
        time: impl Into<Duration>,
        dmabuf_feedback: Option<SurfaceDmabufFeedback>,
        render_element_states: &RenderElementStates,
    ) {
        profiling::scope!("post_repaint");
        let time = time.into();
        // XXX: this was originally set to 1 second, which caused stuttering and lagginess on the
        // winit and X11 backends (but not the udev backend).  Setting to 16ms seems to fix the
        // problem on winit and X11, and so far seems to show no ill effects for udev.
        let throttle = Some(Duration::from_millis(16));

        #[allow(clippy::mutable_key_type)]
        let mut clients: HashMap<ClientId, Client> = HashMap::new();

        let workspace = self.core.workspace_manager.active_workspace();
        let space = workspace.space();
        space.elements().for_each(|window| {
            window.with_surfaces(|surface, states| {
                let primary_scanout_output = surface_primary_scanout_output(surface, states);

                if let Some(output) = primary_scanout_output.as_ref() {
                    with_fractional_scale(states, |fraction_scale| {
                        fraction_scale.set_preferred_scale(output.current_scale().fractional_scale());
                    });
                }

                if primary_scanout_output.as_ref().map(|o| o == output).unwrap_or(true) {
                    let fifo_barrier = states.cached_state.get::<FifoBarrierCachedState>().current().barrier.take();

                    if let Some(fifo_barrier) = fifo_barrier {
                        fifo_barrier.signal();
                        let client = surface.client().unwrap();
                        clients.insert(client.id(), client);
                    }
                }
            });

            if space.outputs_for_element(window).contains(output) {
                window.send_frame(output, time, throttle, surface_primary_scanout_output);
                if let Some(dmabuf_feedback) = dmabuf_feedback.as_ref() {
                    window.send_dmabuf_feedback(output, surface_primary_scanout_output, |surface, _| {
                        select_dmabuf_feedback(
                            surface,
                            render_element_states,
                            &dmabuf_feedback.render_feedback,
                            &dmabuf_feedback.scanout_feedback,
                        )
                    });
                }
            }
        });
        let map = smithay::desktop::layer_map_for_output(output);
        for layer_surface in map.layers() {
            layer_surface.with_surfaces(|surface, states| {
                let primary_scanout_output = surface_primary_scanout_output(surface, states);

                if let Some(output) = primary_scanout_output.as_ref() {
                    with_fractional_scale(states, |fraction_scale| {
                        fraction_scale.set_preferred_scale(output.current_scale().fractional_scale());
                    });
                }

                if primary_scanout_output.as_ref().map(|o| o == output).unwrap_or(true) {
                    let fifo_barrier = states.cached_state.get::<FifoBarrierCachedState>().current().barrier.take();

                    if let Some(fifo_barrier) = fifo_barrier {
                        fifo_barrier.signal();
                        let client = surface.client().unwrap();
                        clients.insert(client.id(), client);
                    }
                }
            });

            layer_surface.send_frame(output, time, throttle, surface_primary_scanout_output);
            if let Some(dmabuf_feedback) = dmabuf_feedback.as_ref() {
                layer_surface.send_dmabuf_feedback(output, surface_primary_scanout_output, |surface, _| {
                    select_dmabuf_feedback(
                        surface,
                        render_element_states,
                        &dmabuf_feedback.render_feedback,
                        &dmabuf_feedback.scanout_feedback,
                    )
                });
            }
        }
        // Drop the lock to the layer map before calling blocker_cleared, which might end up
        // calling the commit handler which in turn again could access the layer map.
        std::mem::drop(map);

        if let CursorImageStatus::Surface(ref surface) = self.core.cursor_status {
            with_surfaces_surface_tree(surface, |surface, states| {
                let primary_scanout_output = surface_primary_scanout_output(surface, states);

                if let Some(output) = primary_scanout_output.as_ref() {
                    with_fractional_scale(states, |fraction_scale| {
                        fraction_scale.set_preferred_scale(output.current_scale().fractional_scale());
                    });
                }

                if primary_scanout_output.as_ref().map(|o| o == output).unwrap_or(true) {
                    let fifo_barrier = states.cached_state.get::<FifoBarrierCachedState>().current().barrier.take();

                    if let Some(fifo_barrier) = fifo_barrier {
                        fifo_barrier.signal();
                        let client = surface.client().unwrap();
                        clients.insert(client.id(), client);
                    }
                }
            });
        }

        if let Some(surface) = self.core.dnd_icon.as_ref().map(|icon| &icon.surface) {
            with_surfaces_surface_tree(surface, |surface, states| {
                let primary_scanout_output = surface_primary_scanout_output(surface, states);

                if let Some(output) = primary_scanout_output.as_ref() {
                    with_fractional_scale(states, |fraction_scale| {
                        fraction_scale.set_preferred_scale(output.current_scale().fractional_scale());
                    });
                }

                if primary_scanout_output.as_ref().map(|o| o == output).unwrap_or(true) {
                    let fifo_barrier = states.cached_state.get::<FifoBarrierCachedState>().current().barrier.take();

                    if let Some(fifo_barrier) = fifo_barrier {
                        fifo_barrier.signal();
                        let client = surface.client().unwrap();
                        clients.insert(client.id(), client);
                    }
                }
            });
        }

        let dh = self.core.display_handle.clone();
        for client in clients.into_values() {
            self.client_compositor_state(&client).blocker_cleared(self, &dh);
        }
    }
}

pub(in crate::core) fn render_to_capture_buffer<F>(
    gles: &mut GlesRenderer,
    size: Size<i32, Buffer>,
    dmabuf: Option<Dmabuf>,
    wl_buffer: &WlBuffer,
    render_fn: F,
) -> anyhow::Result<()>
where
    F: for<'fb> FnOnce(&mut GlesRenderer, &mut GlesTarget<'fb>) -> anyhow::Result<()>,
{
    if let Some(mut dmabuf) = dmabuf {
        let mut fb = gles.bind(&mut dmabuf)?;
        render_fn(gles, &mut fb)
    } else {
        let mut offscreen: GlesRenderbuffer = gles.create_buffer(Fourcc::Argb8888, size)?;
        let mut fb = gles.bind(&mut offscreen)?;
        render_fn(gles, &mut fb)?;

        let region = Rectangle::from_size(size);
        let mapping = gles.copy_framebuffer(&fb, region, Fourcc::Argb8888)?;
        let bytes = gles.map_texture(&mapping)?;

        let width = size.w as usize;
        let height = size.h as usize;
        let row_stride = width * 4;

        shm::with_buffer_contents_mut(wl_buffer, |ptr, _, data| {
            let dst_stride = data.stride as usize;
            for y in 0..height {
                let src_start = (if mapping.flipped() { y } else { height - 1 - y }) * row_stride;
                let dst_start = data.offset as usize + y * dst_stride;
                let dst = unsafe { std::slice::from_raw_parts_mut(ptr.add(dst_start), width * 4) };
                dst.copy_from_slice(&bytes[src_start..src_start + width * 4]);
            }
        })?;

        Ok(())
    }
}

fn update_primary_scanout_output(
    workspace: &Workspace,
    output: &Output,
    dnd_icon: &Option<DndIcon>,
    cursor_status: &CursorImageStatus,
    render_element_states: &RenderElementStates,
) {
    profiling::scope!("update_primary_scanout_output");
    workspace.elements().for_each(|window| {
        window.with_surfaces(|surface, states| {
            update_surface_primary_scanout_output(
                surface,
                output,
                states,
                render_element_states,
                default_primary_scanout_output_compare,
            );
        });
    });
    let map = smithay::desktop::layer_map_for_output(output);
    for layer_surface in map.layers() {
        layer_surface.with_surfaces(|surface, states| {
            update_surface_primary_scanout_output(
                surface,
                output,
                states,
                render_element_states,
                default_primary_scanout_output_compare,
            );
        });
    }

    if let CursorImageStatus::Surface(surface) = cursor_status {
        with_surfaces_surface_tree(surface, |surface, states| {
            update_surface_primary_scanout_output(
                surface,
                output,
                states,
                render_element_states,
                default_primary_scanout_output_compare,
            );
        });
    }

    if let Some(surface) = dnd_icon.as_ref().map(|icon| &icon.surface) {
        with_surfaces_surface_tree(surface, |surface, states| {
            update_surface_primary_scanout_output(
                surface,
                output,
                states,
                render_element_states,
                default_primary_scanout_output_compare,
            );
        });
    }
}
