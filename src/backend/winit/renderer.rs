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

#[cfg(feature = "egl")]
use smithay::backend::renderer::ImportEgl;
use smithay::{
    backend::{
        allocator::{Fourcc, dmabuf::Dmabuf, format::FormatSet},
        renderer::{
            Bind, ContextId, DebugFlags, ExportMem, ImportDma, ImportDmaWl, ImportMem, ImportMemWl, Offscreen, Renderer, RendererSuper,
            TextureFilter,
            gles::{GlesError, GlesFrame, GlesMapping, GlesRenderbuffer, GlesRenderer, GlesTarget, GlesTexture},
        },
    },
    reexports::wayland_server::protocol::wl_buffer,
    utils::{Buffer, Physical, Rectangle, Size, Transform},
    wayland::compositor::SurfaceData,
};

pub struct WinitRenderer<'a>(pub &'a mut GlesRenderer);

impl std::fmt::Debug for WinitRenderer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("WinitRenderer").field(&self.0).finish()
    }
}

impl RendererSuper for WinitRenderer<'_> {
    type Error = GlesError;
    type TextureId = GlesTexture;
    type Framebuffer<'buffer> = GlesTarget<'buffer>;
    type Frame<'frame, 'buffer>
        = GlesFrame<'frame, 'buffer>
    where
        'buffer: 'frame,
        Self: 'frame;
}

impl Renderer for WinitRenderer<'_> {
    fn context_id(&self) -> ContextId<Self::TextureId> {
        self.0.context_id()
    }

    fn downscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.0.downscale_filter(filter)
    }

    fn upscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.0.upscale_filter(filter)
    }

    fn set_debug_flags(&mut self, flags: DebugFlags) {
        self.0.set_debug_flags(flags)
    }

    fn debug_flags(&self) -> DebugFlags {
        self.0.debug_flags()
    }

    fn render<'frame, 'buffer>(
        &'frame mut self,
        framebuffer: &'frame mut Self::Framebuffer<'buffer>,
        output_size: Size<i32, Physical>,
        dst_transform: Transform,
    ) -> Result<Self::Frame<'frame, 'buffer>, Self::Error>
    where
        'buffer: 'frame,
    {
        self.0.render(framebuffer, output_size, dst_transform)
    }

    fn cleanup_texture_cache(&mut self) -> Result<(), Self::Error> {
        self.0.cleanup_texture_cache()
    }

    fn wait(&mut self, sync: &smithay::backend::renderer::sync::SyncPoint) -> Result<(), Self::Error> {
        self.0.wait(sync)
    }
}

impl ImportDma for WinitRenderer<'_> {
    fn dmabuf_formats(&self) -> FormatSet {
        self.0.dmabuf_formats()
    }

    fn import_dmabuf(&mut self, dmabuf: &Dmabuf, damage: Option<&[Rectangle<i32, Buffer>]>) -> Result<Self::TextureId, Self::Error> {
        self.0.import_dmabuf(dmabuf, damage)
    }
}

impl ExportMem for WinitRenderer<'_> {
    type TextureMapping = GlesMapping;

    fn copy_framebuffer(
        &mut self,
        target: &Self::Framebuffer<'_>,
        region: Rectangle<i32, Buffer>,
        format: Fourcc,
    ) -> Result<Self::TextureMapping, Self::Error> {
        self.0.copy_framebuffer(target, region, format)
    }

    fn copy_texture(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, Buffer>,
        format: Fourcc,
    ) -> Result<Self::TextureMapping, Self::Error> {
        self.0.copy_texture(texture, region, format)
    }

    fn can_read_texture(&mut self, texture: &Self::TextureId) -> Result<bool, Self::Error> {
        self.0.can_read_texture(texture)
    }

    fn map_texture<'a>(&mut self, texture_mapping: &'a Self::TextureMapping) -> Result<&'a [u8], Self::Error> {
        self.0.map_texture(texture_mapping)
    }
}

impl Offscreen<GlesRenderbuffer> for WinitRenderer<'_> {
    fn create_buffer(&mut self, format: Fourcc, size: Size<i32, Buffer>) -> Result<GlesRenderbuffer, Self::Error> {
        self.0.create_buffer(format, size)
    }
}

impl Bind<GlesRenderbuffer> for WinitRenderer<'_> {
    fn bind<'a>(&mut self, target: &'a mut GlesRenderbuffer) -> Result<Self::Framebuffer<'a>, Self::Error> {
        self.0.bind(target)
    }

    fn supported_formats(&self) -> Option<FormatSet> {
        Bind::<GlesRenderbuffer>::supported_formats(&*self.0)
    }
}

impl ImportMem for WinitRenderer<'_> {
    fn import_memory(
        &mut self,
        data: &[u8],
        format: Fourcc,
        size: Size<i32, Buffer>,
        flipped: bool,
    ) -> Result<Self::TextureId, Self::Error> {
        self.0.import_memory(data, format, size, flipped)
    }

    fn update_memory(&mut self, texture: &Self::TextureId, data: &[u8], region: Rectangle<i32, Buffer>) -> Result<(), Self::Error> {
        self.0.update_memory(texture, data, region)
    }

    fn mem_formats(&self) -> Box<dyn Iterator<Item = Fourcc>> {
        self.0.mem_formats()
    }
}

impl ImportMemWl for WinitRenderer<'_> {
    fn import_shm_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&SurfaceData>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Result<Self::TextureId, Self::Error> {
        self.0.import_shm_buffer(buffer, surface, damage)
    }
}

impl ImportDmaWl for WinitRenderer<'_> {}

#[cfg(feature = "egl")]
impl ImportEgl for WinitRenderer<'_> {
    fn bind_wl_display(&mut self, display: &smithay::reexports::wayland_server::DisplayHandle) -> Result<(), smithay::backend::egl::Error> {
        self.0.bind_wl_display(display)
    }

    fn unbind_wl_display(&mut self) {
        self.0.unbind_wl_display()
    }

    fn egl_reader(&self) -> Option<&smithay::backend::egl::display::EGLBufferReader> {
        self.0.egl_reader()
    }

    fn import_egl_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&SurfaceData>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Result<Self::TextureId, Self::Error> {
        self.0.import_egl_buffer(buffer, surface, damage)
    }
}
