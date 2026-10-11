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

use anyhow::anyhow;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, ExportMem, Frame, Offscreen, Renderer,
            element::{AsRenderElements, Element, RenderElement},
            gles::GlesRenderbuffer,
            sync::SyncPoint,
        },
    },
    desktop::Window,
    reexports::wayland_server::protocol::{wl_buffer::WlBuffer, wl_shm},
    utils::{Logical, Physical, Rectangle, Scale, Size, Transform},
    wayland::shm::with_buffer_contents,
};

use crate::{
    backend::Backend,
    core::{shell::WindowElement, state::Xfwl4State},
    util::icon::Argb32Pixels,
};

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    /// Returns a thumbnail/preview of the current window contents for each of the given windows.
    ///
    /// In order to avoid blocking on GPU<->CPU round-trips, we do all windows in a batch, in three
    /// phases:
    ///
    /// 1. First create renderbuffers and submit frames for each window.
    /// 2. Then ask the GPU to copy each framebuffer.
    /// 3. Finally, read back the pixels for each window.
    ///
    /// See [`Argb32Pixels`] for returned pixel format.
    pub(in crate::core) fn windows_to_image_data(
        &mut self,
        windows: &[WindowElement],
        max_size: u32,
        output_scale: impl Into<Scale<f64>>,
    ) -> anyhow::Result<Vec<(WindowElement, Argb32Pixels)>> {
        struct Pending {
            window: WindowElement,
            renderbuffer: GlesRenderbuffer,
            sync: SyncPoint,
            physical_size: Size<i32, Physical>,
            #[cfg(feature = "debug-rendering")]
            logical_size: Size<i32, Logical>,
            scale: u32,
        }

        #[cfg(feature = "debug-rendering")]
        let start = std::time::Instant::now();

        let output_scale = output_scale.into();
        let mut renderer = self.backend.renderer(
            #[cfg(feature = "udev")]
            None,
        )?;

        let mut submit_window =
            |window: &WindowElement| -> anyhow::Result<Pending> {
                #[cfg(feature = "debug-rendering")]
                let submit_start = std::time::Instant::now();

                let logical_size = window.0.bbox().size;
                if logical_size.w <= 0 || logical_size.h <= 0 {
                    Err(anyhow!("invalid window size"))
                } else {
                    let aspect_ratio = logical_size.w as f64 / logical_size.h as f64;
                    let (thumbnail_logical_w, thumbnail_logical_h) = if logical_size.w >= logical_size.h {
                        (max_size as i32, (max_size as f64 / aspect_ratio).round() as i32)
                    } else {
                        ((max_size as f64 * aspect_ratio).round() as i32, max_size as i32)
                    };

                    let (thumbnail_logical_w, thumbnail_logical_h) =
                        if thumbnail_logical_w > logical_size.w || thumbnail_logical_h > logical_size.h {
                            (logical_size.w, logical_size.h)
                        } else {
                            (thumbnail_logical_w, thumbnail_logical_h)
                        };

                    let thumbnail_scale_factor =
                        (thumbnail_logical_w as f64 / logical_size.w as f64).min(thumbnail_logical_h as f64 / logical_size.h as f64);
                    let render_scale = Scale::from(thumbnail_scale_factor * output_scale.x);

                    let elements = AsRenderElements::render_elements::<
                        <Window as AsRenderElements<BackendData::Renderer<'_>>>::RenderElement,
                    >(&window.0, &mut renderer, (0, 0).into(), render_scale, 1.0);

                    let thumbnail_logical_size: Size<i32, Logical> = (thumbnail_logical_w, thumbnail_logical_h).into();
                    let thumbnail_physical_size = thumbnail_logical_size.to_physical_precise_round(output_scale);
                    let buffer_size = (thumbnail_physical_size.w, thumbnail_physical_size.h).into();

                    let mut renderbuffer = renderer.create_buffer(Fourcc::Argb8888, buffer_size)?;

                    let sync = {
                        let mut framebuffer = renderer.bind(&mut renderbuffer)?;
                        let mut frame = renderer.render(&mut framebuffer, thumbnail_physical_size, Transform::Normal)?;

                        frame.clear([0., 0., 0., 0.].into(), &[Rectangle::from_size(thumbnail_physical_size)])?;

                        for element in &elements {
                            let element_geometry = element.geometry(render_scale);
                            let damage = [element_geometry];
                            let opaque_regions = element.opaque_regions(render_scale);
                            if let Err(err) = element.draw(&mut frame, element.src(), element_geometry, &damage, &opaque_regions, None) {
                                tracing::debug!("Failed to draw element: {}", err);
                            }
                        }

                        frame.finish()?
                    };

                    #[cfg(feature = "debug-rendering")]
                    tracing::trace!(
                        "Submitted window thumbnail render ({}x{} -> {}x{}) in {:.2}ms",
                        logical_size.w,
                        logical_size.h,
                        thumbnail_physical_size.w,
                        thumbnail_physical_size.h,
                        submit_start.elapsed().as_secs_f64() * 1000.0
                    );

                    Ok(Pending {
                        window: window.clone(),
                        renderbuffer,
                        sync,
                        physical_size: thumbnail_physical_size,
                        #[cfg(feature = "debug-rendering")]
                        logical_size,
                        scale: output_scale.x.ceil().max(1.) as u32,
                    })
                }
            };

        let pending = windows
            .iter()
            .filter_map(|window| {
                submit_window(window)
                    .inspect_err(|err| tracing::info!("Failed to submit window preview render: {err}"))
                    .ok()
            })
            .collect::<Vec<_>>();

        let mappings = pending
            .into_iter()
            .filter_map(|mut pending_render| {
                #[cfg(feature = "debug-rendering")]
                let copy_start = std::time::Instant::now();

                renderer
                    .wait(&pending_render.sync)
                    .inspect_err(|err| tracing::info!("Failed to wait for window preview render: {err}"))
                    .ok()?;

                let framebuffer = renderer
                    .bind(&mut pending_render.renderbuffer)
                    .inspect_err(|err| tracing::info!("Failed to bind window preview framebuffer: {err}"))
                    .ok()?;

                let buffer_size = (pending_render.physical_size.w, pending_render.physical_size.h).into();
                let region = Rectangle::from_size(buffer_size);
                let mapping = renderer
                    .copy_framebuffer(&framebuffer, region, Fourcc::Argb8888)
                    .inspect_err(|err| tracing::info!("Failed to copy window preview framebuffer: {err}"))
                    .ok()?;

                #[cfg(feature = "debug-rendering")]
                tracing::trace!(
                    "Copied window thumbnail framebuffer ({}x{} -> {}x{}) in {:.2}ms",
                    pending_render.logical_size.w,
                    pending_render.logical_size.h,
                    pending_render.physical_size.w,
                    pending_render.physical_size.h,
                    copy_start.elapsed().as_secs_f64() * 1000.0
                );

                Some((pending_render.window, mapping, pending_render.physical_size, pending_render.scale))
            })
            .collect::<Vec<_>>();

        let preview_images = mappings
            .into_iter()
            .filter_map(|(window, mapping, physical_size, scale)| {
                #[cfg(feature = "debug-rendering")]
                let map_start = std::time::Instant::now();

                let bytes = renderer
                    .map_texture(&mapping)
                    .inspect_err(|err| tracing::info!("Failed to map window preview framebuffer: {err}"))
                    .ok()?
                    .to_vec();

                #[cfg(feature = "debug-rendering")]
                tracing::trace!(
                    "Mapped window thumbnail ({}x{}) in {:.2}ms",
                    physical_size.w,
                    physical_size.h,
                    map_start.elapsed().as_secs_f64() * 1000.0
                );

                Some((
                    window,
                    Argb32Pixels {
                        bytes,
                        size: (physical_size.w as u32, physical_size.h as u32).into(),
                        scale,
                    },
                ))
            })
            .collect::<Vec<_>>();

        #[cfg(feature = "debug-rendering")]
        tracing::trace!(
            "Finished generating {} window thumbnails in {:.2}ms",
            preview_images.len(),
            start.elapsed().as_secs_f64() * 1000.0,
        );

        Ok(preview_images)
    }
}

pub fn shm_buffer_to_image_data(buffer: &WlBuffer, scale: u32) -> anyhow::Result<Argb32Pixels> {
    with_buffer_contents(buffer, |ptr, _len, data| {
        let width = data.width as u32;
        let height = data.height as u32;
        let stride = data.stride as usize;

        let (has_alpha, bgr) = match data.format {
            wl_shm::Format::Argb8888 => Ok((true, false)),
            wl_shm::Format::Xrgb8888 => Ok((false, false)),
            wl_shm::Format::Abgr8888 => Ok((true, true)),
            wl_shm::Format::Xbgr8888 => Ok((false, true)),
            _ => Err(anyhow!("unsupported shm format {:?}", data.format)),
        }?;

        let mut bytes = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            let row_start = data.offset as usize + y as usize * stride;
            for x in 0..width {
                let pixel_offset = row_start + x as usize * 4;
                let pixel = unsafe { std::slice::from_raw_parts(ptr.add(pixel_offset), 4) };
                let (r, b) = if bgr { (pixel[0], pixel[2]) } else { (pixel[2], pixel[0]) };
                let a = if has_alpha { pixel[3] } else { 255 };
                bytes.extend_from_slice(&[r, pixel[1], b, a]);
            }
        }

        Ok(Argb32Pixels {
            bytes,
            size: (width, height).into(),
            scale,
        })
    })?
}
