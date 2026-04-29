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

use std::{sync::Mutex, time::Duration};

use smithay::{
    backend::renderer::{
        Color32F, ImportAll, ImportMem, Renderer, Texture,
        element::{AsRenderElements, Kind, memory::MemoryRenderBufferRenderElement, surface::WaylandSurfaceRenderElement},
    },
    input::pointer::{CursorImageAttributes, CursorImageStatus},
    reexports::wayland_server::Resource,
    render_elements,
    utils::{Logical, Physical, Point, Scale},
    wayland::compositor,
};

use crate::core::cursor::{CursorFrame, CursorTheme};

pub static CLEAR_COLOR: Color32F = Color32F::new(0.1, 0.1, 0.1, 1.0);
pub static CLEAR_COLOR_FULLSCREEN: Color32F = Color32F::new(0.0, 0.0, 0.0, 0.0);

pub struct PointerElement {
    status: CursorImageStatus,
    cursor: Option<CursorFrame>,
    hotspot: Option<Point<i32, Logical>>,
}

impl Default for PointerElement {
    fn default() -> Self {
        Self {
            status: CursorImageStatus::default_named(),
            cursor: None,
            hotspot: None,
        }
    }
}

impl PointerElement {
    pub fn set_status(&mut self, status: CursorImageStatus) {
        self.status = status;
        self.cursor = None;
        self.hotspot = None;
    }

    pub fn prepare(&mut self, theme: &mut CursorTheme, output_scale: f64, time: Duration) {
        if let CursorImageStatus::Surface(surface) = &self.status
            && !surface.is_alive()
        {
            self.status = CursorImageStatus::default_named();
        }

        match &self.status {
            CursorImageStatus::Hidden => {
                self.cursor = None;
                self.hotspot = None;
            }

            CursorImageStatus::Named(cursor_icon) => {
                let cursor = theme.get_frame(*cursor_icon, output_scale, time);
                self.hotspot = Some(cursor.hotspot);
                self.cursor = Some(cursor);
            }

            CursorImageStatus::Surface(surface) => {
                self.cursor = None;
                self.hotspot = compositor::with_states(surface, |states| {
                    states
                        .data_map
                        .get::<Mutex<CursorImageAttributes>>()
                        .map(|attrs| attrs.lock().unwrap().hotspot)
                });
            }
        }
    }

    pub fn status(&self) -> &CursorImageStatus {
        &self.status
    }

    pub fn hotspot(&self) -> Option<Point<i32, Logical>> {
        self.hotspot
    }
}

render_elements! {
    pub PointerRenderElement<R> where R: ImportAll + ImportMem;
    Surface=WaylandSurfaceRenderElement<R>,
    Memory=MemoryRenderBufferRenderElement<R>,
}

impl<R: Renderer> std::fmt::Debug for PointerRenderElement<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Surface(arg0) => f.debug_tuple("Surface").field(arg0).finish(),
            Self::Memory(arg0) => f.debug_tuple("Memory").field(arg0).finish(),
            Self::_GenericCatcher(arg0) => f.debug_tuple("_GenericCatcher").field(arg0).finish(),
        }
    }
}

impl<T: Texture + Clone + Send + 'static, R> AsRenderElements<R> for PointerElement
where
    R: Renderer<TextureId = T> + ImportAll + ImportMem,
{
    type RenderElement = PointerRenderElement<R>;
    fn render_elements<E>(&self, renderer: &mut R, location: Point<i32, Physical>, scale: Scale<f64>, alpha: f32) -> Vec<E>
    where
        E: From<PointerRenderElement<R>>,
    {
        match (&self.status, &self.cursor) {
            (CursorImageStatus::Hidden, _) => vec![],

            (CursorImageStatus::Named(_), Some(cursor)) => MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                location.to_f64(),
                &cursor.buffer,
                None,
                Some(cursor.src),
                Some(cursor.size),
                Kind::Cursor,
            )
            .inspect_err(|err| tracing::warn!("Failed to build render buffer for pointer: {err}"))
            .map(|buffer| vec![PointerRenderElement::<R>::from(buffer).into()])
            .unwrap_or_else(|_| Vec::default()),

            (CursorImageStatus::Surface(surface), _) if surface.is_alive() => {
                let elements: Vec<PointerRenderElement<R>> =
                    smithay::backend::renderer::element::surface::render_elements_from_surface_tree(
                        renderer,
                        surface,
                        location,
                        scale,
                        alpha,
                        Kind::Cursor,
                    );
                elements.into_iter().map(E::from).collect()
            }

            _ => {
                tracing::warn!("Bad CursorImageStatus; pointer will not display");
                vec![]
            }
        }
    }
}
