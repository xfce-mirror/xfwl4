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
    backend::renderer::{
        ImportAll, ImportMem, Renderer, RendererSuper,
        element::{Element, Id, Kind, RenderElement, UnderlyingStorage, surface::WaylandSurfaceRenderElement},
        gles::GlesRenderer,
        utils::{CommitCounter, DamageSet, OpaqueRegions},
    },
    utils::{Buffer, Physical, Rectangle, Scale, Transform, user_data::UserDataMap},
};

use crate::{
    backend::{AsGlesRenderer, FromGlesError},
    core::shell::{WindowRenderElement, ssd::DecorationRenderElement},
};

impl<R: Renderer> From<WaylandSurfaceRenderElement<R>> for WindowRenderElement<R> {
    fn from(elem: WaylandSurfaceRenderElement<R>) -> Self {
        WindowRenderElement::Window(elem)
    }
}

impl<R: Renderer> From<DecorationRenderElement> for WindowRenderElement<R> {
    fn from(elem: DecorationRenderElement) -> Self {
        WindowRenderElement::Decoration(elem)
    }
}

impl<R> Element for WindowRenderElement<R>
where
    R: Renderer + ImportAll + ImportMem,
{
    fn id(&self) -> &Id {
        match self {
            WindowRenderElement::Window(elem) => elem.id(),
            WindowRenderElement::Decoration(elem) => elem.id(),
            WindowRenderElement::Shadow(elem) => elem.id(),
            WindowRenderElement::Wireframe(elem) => elem.id(),
        }
    }

    fn current_commit(&self) -> CommitCounter {
        match self {
            WindowRenderElement::Window(elem) => elem.current_commit(),
            WindowRenderElement::Decoration(elem) => elem.current_commit(),
            WindowRenderElement::Shadow(elem) => elem.current_commit(),
            WindowRenderElement::Wireframe(elem) => elem.current_commit(),
        }
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        match self {
            WindowRenderElement::Window(elem) => elem.geometry(scale),
            WindowRenderElement::Decoration(elem) => elem.geometry(scale),
            WindowRenderElement::Shadow(elem) => elem.geometry(scale),
            WindowRenderElement::Wireframe(elem) => elem.geometry(scale),
        }
    }

    fn transform(&self) -> Transform {
        match self {
            WindowRenderElement::Window(elem) => elem.transform(),
            WindowRenderElement::Decoration(elem) => elem.transform(),
            WindowRenderElement::Shadow(elem) => elem.transform(),
            WindowRenderElement::Wireframe(elem) => elem.transform(),
        }
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        match self {
            WindowRenderElement::Window(elem) => elem.src(),
            WindowRenderElement::Decoration(elem) => elem.src(),
            WindowRenderElement::Shadow(elem) => elem.src(),
            WindowRenderElement::Wireframe(elem) => elem.src(),
        }
    }

    fn damage_since(&self, scale: Scale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        match self {
            WindowRenderElement::Window(elem) => elem.damage_since(scale, commit),
            WindowRenderElement::Decoration(elem) => elem.damage_since(scale, commit),
            WindowRenderElement::Shadow(elem) => elem.damage_since(scale, commit),
            WindowRenderElement::Wireframe(elem) => elem.damage_since(scale, commit),
        }
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        match self {
            WindowRenderElement::Window(elem) => elem.opaque_regions(scale),
            WindowRenderElement::Decoration(elem) => elem.opaque_regions(scale),
            WindowRenderElement::Shadow(elem) => elem.opaque_regions(scale),
            WindowRenderElement::Wireframe(elem) => elem.opaque_regions(scale),
        }
    }

    fn alpha(&self) -> f32 {
        match self {
            WindowRenderElement::Window(elem) => elem.alpha(),
            WindowRenderElement::Decoration(elem) => elem.alpha(),
            WindowRenderElement::Shadow(elem) => elem.alpha(),
            WindowRenderElement::Wireframe(elem) => elem.alpha(),
        }
    }

    fn kind(&self) -> Kind {
        match self {
            WindowRenderElement::Window(elem) => elem.kind(),
            WindowRenderElement::Decoration(elem) => elem.kind(),
            WindowRenderElement::Shadow(elem) => elem.kind(),
            WindowRenderElement::Wireframe(elem) => elem.kind(),
        }
    }
}

impl<R> RenderElement<R> for WindowRenderElement<R>
where
    R: Renderer + ImportAll + ImportMem + AsGlesRenderer,
    R::TextureId: 'static,
    <R as RendererSuper>::Error: FromGlesError,
{
    fn draw(
        &self,
        frame: &mut <R as RendererSuper>::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), <R as RendererSuper>::Error> {
        match self {
            WindowRenderElement::Window(elem) => elem.draw(frame, src, dst, damage, opaque_regions, cache),
            WindowRenderElement::Decoration(elem) => {
                RenderElement::<GlesRenderer>::draw(elem, R::gles_frame_mut(frame), src, dst, damage, opaque_regions, cache)
                    .map_err(FromGlesError::from_gles_error)
            }
            WindowRenderElement::Shadow(elem) => {
                RenderElement::<GlesRenderer>::draw(elem, R::gles_frame_mut(frame), src, dst, damage, opaque_regions, cache)
                    .map_err(FromGlesError::from_gles_error)
            }
            WindowRenderElement::Wireframe(elem) => {
                RenderElement::<GlesRenderer>::draw(elem, R::gles_frame_mut(frame), src, dst, damage, opaque_regions, cache)
                    .map_err(FromGlesError::from_gles_error)
            }
        }
    }

    fn underlying_storage(&self, renderer: &mut R) -> Option<UnderlyingStorage<'_>> {
        match self {
            WindowRenderElement::Window(elem) => elem.underlying_storage(renderer),
            WindowRenderElement::Decoration(elem) => elem.underlying_storage(renderer.gles_renderer_mut()),
            WindowRenderElement::Shadow(elem) => elem.underlying_storage(renderer.gles_renderer_mut()),
            WindowRenderElement::Wireframe(elem) => elem.underlying_storage(renderer.gles_renderer_mut()),
        }
    }
}
